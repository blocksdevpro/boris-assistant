//! Execute a batch of tool calls from one assistant message.
//!
//! Dispatch rules:
//! - Length 1 → sequential path.
//! - Length > 1 → preflight with [`ToolRuntime::decide_only`]; if any call needs
//!   confirmation, fall back to sequential (HITL-safe). Otherwise:
//!   - **wave scheduling** (default): read-only wave (parallel, chunked) then
//!     write wave (sequential)
//!   - else: legacy unbounded `join_all` for all auto-allow calls
//!
//! Sequential batches can pause mid-batch for HITL and keep remaining siblings.

use std::time::Instant;

use futures::future::join_all;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    clamp_parallel, partition_read_write, InvokeOptions, InvokeResult, PendingTurn, PolicyDecision,
    RawToolCall,
};
use crate::tool::Tool;
use crate::types::{AgentLoopConfig, EmitFn};

use super::helpers::{
    build_tool_invocation, commit_tool_observation, emit_tool_start, find_tool, observation_looks_ok,
    parallel_batch_observation,
};
use super::LoopState;

/// Outcome of processing one model tool-call batch.
pub(super) enum ToolBatchResult {
    /// All calls finished; loop should request another LLM completion.
    Continue,
    /// HITL pause — host must confirm/reject before `resume_pending_tool`.
    Paused {
        outcome: AgentOutcome,
        pending_turn: PendingTurn,
    },
}

/// Process a batch of tool calls with the appropriate concurrency strategy.
pub(super) async fn process_tool_calls(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    tool_rounds: u32,
    confirms_used: &mut u32,
    user_text: &str,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
) -> Result<ToolBatchResult, AgentError> {
    if calls.len() > 1 {
        if !batch_needs_confirmation(state, &calls, *confirms_used)? {
            if config.features.wave_scheduling {
                tracing::debug!(
                    batch = calls.len(),
                    "tool batch: wave scheduling (parallel reads + sequential writes)"
                );
                return run_tool_batch_waves(
                    state,
                    calls,
                    tools_used,
                    *confirms_used,
                    config,
                    emit,
                    cancel,
                )
                .await;
            }
            tracing::debug!(
                batch = calls.len(),
                "tool batch: parallel dispatch (legacy join_all)"
            );
            return run_tool_batch_parallel(
                state,
                calls,
                tools_used,
                *confirms_used,
                config,
                emit,
                cancel,
            )
            .await;
        }
        tracing::debug!(
            batch = calls.len(),
            "tool batch: sequential (HITL or confirm risk in batch)"
        );
    }

    run_tool_batch_sequential(
        state,
        calls,
        tools_used,
        tool_rounds,
        confirms_used,
        user_text,
        config,
        emit,
        cancel,
    )
    .await
}

/// True if any call in the batch would require user confirmation under current policy.
fn batch_needs_confirmation(
    state: &LoopState<'_>,
    calls: &[RawToolCall],
    confirms_used: u32,
) -> Result<bool, AgentError> {
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used,
    };
    for call in calls {
        let tool = find_tool(state.tools, &call.name)?;
        if matches!(
            state.runtime.decide_only(tool, &call.args, opts),
            PolicyDecision::NeedsConfirmation { .. }
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Sequential path — HITL-safe; can pause mid-batch with remaining siblings.
async fn run_tool_batch_sequential(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    tool_rounds: u32,
    confirms_used: &mut u32,
    user_text: &str,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
) -> Result<ToolBatchResult, AgentError> {
    let mut iter = calls.into_iter();
    while let Some(call) = iter.next() {
        let tool = find_tool(state.tools, &call.name)?;
        let inv = build_tool_invocation(&call, config, cancel.clone(), emit);
        let opts = InvokeOptions {
            skip_confirmation: false,
            confirms_used: *confirms_used,
        };

        emit_tool_start(emit, &call);
        let started = Instant::now();

        match state.runtime.invoke(tool, inv, opts).await {
            InvokeResult::Observation(result) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                commit_tool_observation(
                    state.context,
                    &call,
                    result.clone(),
                    observation_looks_ok(&result),
                    duration_ms,
                    tools_used,
                    emit,
                );
            }
            InvokeResult::Denied { reason } => {
                let duration_ms = started.elapsed().as_millis() as u64;
                commit_tool_observation(
                    state.context,
                    &call,
                    format!("Error: {reason}"),
                    false,
                    duration_ms,
                    tools_used,
                    emit,
                );
            }
            InvokeResult::NeedsConfirmation {
                pending,
                speak_prompt,
            } => {
                *confirms_used = confirms_used.saturating_add(1);
                let remaining: Vec<RawToolCall> = iter.collect();
                let pending_turn = PendingTurn {
                    pending: pending.clone(),
                    remaining_calls: remaining,
                    tools_used: tools_used.clone(),
                    tool_rounds,
                    confirms_used: *confirms_used,
                    user_text: user_text.to_string(),
                };
                return Ok(ToolBatchResult::Paused {
                    outcome: AgentOutcome::NeedsConfirmation {
                        text: speak_prompt,
                        pending,
                    },
                    pending_turn,
                });
            }
        }
    }
    Ok(ToolBatchResult::Continue)
}

/// Parallel path for batches where every call is auto-allowed or denyable.
///
/// Invokes concurrently; only mutates context after all results are collected,
/// preserving original call order.
async fn run_tool_batch_parallel(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    confirms_used: u32,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
) -> Result<ToolBatchResult, AgentError> {
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used,
    };

    // Borrow tools/runtime only for the concurrent phase (no context mut).
    let results = {
        let tools = state.tools;
        let runtime = state.runtime;

        // Resolve tools + emit starts before concurrent work.
        let mut tools_for_calls: Vec<&dyn Tool> = Vec::with_capacity(calls.len());
        for call in &calls {
            let tool = find_tool(tools, &call.name)?;
            emit_tool_start(emit, call);
            tools_for_calls.push(tool);
        }

        let futs = calls.iter().zip(tools_for_calls).map(|(call, tool)| {
            let inv = build_tool_invocation(call, config, cancel.clone(), emit);
            let started = Instant::now();
            async move {
                let result = runtime.invoke(tool, inv, opts).await;
                (started.elapsed().as_millis() as u64, result)
            }
        });

        join_all(futs).await
    };

    commit_batch_results(state, &calls, results, tools_used, emit);
    Ok(ToolBatchResult::Continue)
}

/// Wave scheduling: parallel read-only wave, then sequential non-read-only.
async fn run_tool_batch_waves(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    confirms_used: u32,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
) -> Result<ToolBatchResult, AgentError> {
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used,
    };
    let (read_idx, write_idx) = partition_read_write(&calls, state.tools);
    let max_par = clamp_parallel(read_idx.len(), config.features.max_parallel_tools);

    // Pre-size results in original order.
    let mut ordered: Vec<Option<(u64, InvokeResult)>> = (0..calls.len()).map(|_| None).collect();

    // Read-only wave in chunks of max_par.
    for chunk in read_idx.chunks(max_par.max(1)) {
        let results = {
            let tools = state.tools;
            let runtime = state.runtime;
            let mut pairs: Vec<(&RawToolCall, &dyn Tool)> = Vec::new();
            for &i in chunk {
                let call = &calls[i];
                let tool = find_tool(tools, &call.name)?;
                emit_tool_start(emit, call);
                pairs.push((call, tool));
            }
            let futs = pairs.into_iter().map(|(call, tool)| {
                let inv = build_tool_invocation(call, config, cancel.clone(), emit);
                let started = Instant::now();
                async move {
                    let result = runtime.invoke(tool, inv, opts).await;
                    (started.elapsed().as_millis() as u64, result)
                }
            });
            join_all(futs).await
        };
        for (&i, res) in chunk.iter().zip(results) {
            ordered[i] = Some(res);
        }
    }

    // Write wave: sequential in original relative order.
    for i in write_idx {
        let call = &calls[i];
        let tool = find_tool(state.tools, &call.name)?;
        emit_tool_start(emit, call);
        let inv = build_tool_invocation(call, config, cancel.clone(), emit);
        let started = Instant::now();
        let result = state.runtime.invoke(tool, inv, opts).await;
        ordered[i] = Some((started.elapsed().as_millis() as u64, result));
    }

    let results: Vec<(u64, InvokeResult)> = ordered
        .into_iter()
        .map(|o| o.expect("every call should have a result"))
        .collect();
    commit_batch_results(state, &calls, results, tools_used, emit);
    Ok(ToolBatchResult::Continue)
}

/// Append batch outcomes to context in original call order.
fn commit_batch_results(
    state: &mut LoopState<'_>,
    calls: &[RawToolCall],
    results: Vec<(u64, InvokeResult)>,
    tools_used: &mut Vec<String>,
    emit: &EmitFn,
) {
    for (call, (duration_ms, result)) in calls.iter().zip(results) {
        let (content, ok) = parallel_batch_observation(result);
        commit_tool_observation(
            state.context,
            call,
            content,
            ok,
            duration_ms,
            tools_used,
            emit,
        );
    }
}
