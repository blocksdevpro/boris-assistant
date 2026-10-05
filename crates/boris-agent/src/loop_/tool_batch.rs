//! Execute a batch of tool calls from one assistant message.
//!
//! Dispatch rules:
//! - Length 1 → sequential path.
//! - Wave scheduling (default): contiguous independent read-only runs execute
//!   in a bounded rolling pool. Writes, input, and approval are ordering barriers.
//! - Legacy mode: preflight pause-free batches, then bounded parallel dispatch.
//!
//! Sequential batches can pause mid-batch for HITL and keep remaining siblings.
//! Contiguous confirm-needed calls that share risk **and shell-ness** are
//! batched into one HITL decision (`PendingTurn::batch_with`):
//! - multiple file writes → one yes
//! - multiple bash commands → one yes
//! - shell is never mixed with non-shell in the same prompt

use std::time::Instant;

use futures::{stream, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    args_summary, clamp_parallel, InvokeOptions, InvokeResult, PendingToolCall, PendingTurn,
    PolicyDecision, RawToolCall,
};
use crate::tool::{ObservationError, Permission, ToolObservation, ToolRisk};
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn};

use super::helpers::{
    build_tool_invocation, commit_tool_observation, emit_tool_end, emit_tool_start, find_tool_opt,
    parallel_batch_observation, push_tool_result_messages, unknown_tool_observation,
};
use super::LoopState;

/// Outcome of processing one model tool-call batch.
///
/// The pause path intentionally carries the complete pending-turn state so a
/// confirmed batch can resume without reconstructing model inputs.
#[allow(clippy::large_enum_variant)]
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
#[allow(clippy::too_many_arguments)]
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
    if calls.len() > 1 && config.features.wave_scheduling {
        return run_tool_batch_waves(
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
        .await;
    }
    if calls.len() > 1 {
        if !batch_may_pause(state, &calls, *confirms_used)? {
            tracing::debug!(
                batch = calls.len(),
                max_parallel = config.features.max_parallel_tools,
                "tool batch: parallel dispatch (legacy, chunked)"
            );
            return run_tool_batch_parallel(
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
            .await;
        }
        tracing::debug!(
            batch = calls.len(),
            "tool batch: sequential after optional read-only prefix (pause in batch)"
        );
        let (prefix, rest) = split_auto_readonly_prefix(state, calls, *confirms_used);
        if !prefix.is_empty() {
            tracing::debug!(
                prefix = prefix.len(),
                rest = rest.len(),
                "running auto-approved read-only calls before confirm boundary"
            );
            // Prefix is auto-allow, but a policy race could still surface a
            // confirm/input. The parallel path falls back to sequential
            // internally; if that pauses, propagate the pause.
            match run_tool_batch_parallel(
                state,
                prefix,
                tools_used,
                tool_rounds,
                confirms_used,
                user_text,
                config,
                emit,
                cancel.clone(),
            )
            .await?
            {
                ToolBatchResult::Continue => {}
                ToolBatchResult::Paused {
                    outcome,
                    mut pending_turn,
                } => {
                    // A late pause in the prefix must retain the untouched tail.
                    pending_turn.remaining_calls.extend(rest);
                    return Ok(ToolBatchResult::Paused {
                        outcome,
                        pending_turn,
                    });
                }
            }
        }
        if rest.is_empty() {
            return Ok(ToolBatchResult::Continue);
        }
        return run_tool_batch_sequential(
            state,
            rest,
            tools_used,
            tool_rounds,
            confirms_used,
            user_text,
            config,
            emit,
            cancel,
        )
        .await;
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

/// True if any call can pause for confirmation or on-screen input.
/// Unknown tools are treated as non-pausing (soft-failed later with an error observation).
fn batch_may_pause(
    state: &LoopState<'_>,
    calls: &[RawToolCall],
    confirms_used: u32,
) -> Result<bool, AgentError> {
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used,
    };
    for call in calls {
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            continue;
        };
        if tool.meta().collects_input {
            return Ok(true);
        }
        if matches!(
            state.runtime.decide_only(tool, &call.args, opts),
            PolicyDecision::NeedsConfirmation { .. }
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Independent auto-approved read-only calls that sit *before* the first
/// confirmation (or write) boundary. Later dependents / side-effects stay put.
fn split_auto_readonly_prefix(
    state: &LoopState<'_>,
    calls: Vec<RawToolCall>,
    confirms_used: u32,
) -> (Vec<RawToolCall>, Vec<RawToolCall>) {
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used,
    };
    let mut prefix = Vec::new();
    let mut iter = calls.into_iter();
    while let Some(call) = iter.next() {
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            let mut rest = vec![call];
            rest.extend(iter);
            return (prefix, rest);
        };
        let confirm = matches!(
            state.runtime.decide_only(tool, &call.args, opts),
            PolicyDecision::NeedsConfirmation { .. }
        );
        if tool.meta().is_read_only() && !tool.meta().collects_input && !confirm {
            prefix.push(call);
        } else {
            let mut rest = vec![call];
            rest.extend(iter);
            return (prefix, rest);
        }
    }
    (prefix, Vec::new())
}

/// Sequential path — HITL-safe; can pause mid-batch with remaining siblings.
#[allow(clippy::too_many_arguments)]
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
    let (result, _) = run_tool_batch_sequential_inner(
        state,
        calls,
        tools_used,
        tool_rounds,
        confirms_used,
        user_text,
        config,
        emit,
        cancel,
        false,
    )
    .await?;
    Ok(result)
}

/// A single non-pausing action returns its untouched tail to the wave planner.
/// A pause already captures the full tail (including compatible confirmations).
#[allow(clippy::too_many_arguments)]
async fn run_tool_batch_sequential_inner(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    tool_rounds: u32,
    confirms_used: &mut u32,
    user_text: &str,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
    single_step: bool,
) -> Result<(ToolBatchResult, Vec<RawToolCall>), AgentError> {
    let mut iter = calls.into_iter();
    while let Some(call) = iter.next() {
        if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(AgentError::cancelled("tool batch cancelled"));
        }
        emit_tool_start(emit, &call);
        let started = Instant::now();

        // Soft-fail unknown tools so prompt/history drift cannot kill the turn.
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            tracing::warn!(tool = %call.name, "model requested unknown tool; soft-failing");
            state.runtime.audit_unknown_tool(
                &call.name,
                &call.args,
                config.session_id.as_deref(),
                config.turn_id.as_deref(),
            );
            let duration_ms = started.elapsed().as_millis() as u64;
            commit_tool_observation(
                state.context,
                &call,
                unknown_tool_observation(&call.name),
                false,
                duration_ms,
                tools_used,
                emit,
            );
            if single_step {
                return Ok((ToolBatchResult::Continue, iter.collect()));
            }
            continue;
        };

        let inv = build_tool_invocation(&call, config, cancel.clone(), emit);
        let opts = InvokeOptions {
            skip_confirmation: false,
            confirms_used: *confirms_used,
        };

        match state.runtime.invoke(tool, inv, opts).await {
            InvokeResult::Observation(result) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                commit_tool_observation(
                    state.context,
                    &call,
                    result.to_provider_text(),
                    result.looks_ok(),
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
            InvokeResult::NeedsInput {
                pending,
                speak_prompt,
            } => {
                let rest: Vec<RawToolCall> = iter.collect();
                let pending_turn = PendingTurn {
                    pending: pending.clone(),
                    batch_with: Vec::new(),
                    remaining_calls: rest,
                    tools_used: tools_used.clone(),
                    tool_rounds,
                    confirms_used: *confirms_used,
                    user_text: user_text.to_string(),
                    // Enriched by `loop_::agent_loop` / resume paths with the
                    // live todos path, finish budgets, and accounting. Defaults
                    // keep direct `process_tool_calls` unit tests working.
                    todos_file: None,
                    markup_left: crate::finish_gate::FinishGateBudget::MARKUP_INIT,
                    gate_left: 0,
                    token_accounting: crate::types::TokenAccounting::default(),
                };
                return Ok((
                    ToolBatchResult::Paused {
                        outcome: AgentOutcome::NeedsInput {
                            text: speak_prompt,
                            pending,
                        },
                        pending_turn,
                    },
                    Vec::new(),
                ));
            }
            InvokeResult::NeedsConfirmation {
                pending,
                speak_prompt,
            } => {
                // One HITL decision covers pending + compatible siblings.
                // Still budget-aware: `collect_confirm_batch` caps siblings so
                // `confirms_used + batch_with.len()` never exceeds
                // `max_confirms_per_turn`; overflow stays in `remaining` for a
                // later prompt.
                *confirms_used = confirms_used.saturating_add(1);
                let rest: Vec<RawToolCall> = iter.collect();
                let first_is_shell = tool.meta().permissions.contains(&Permission::Shell);
                let max_confirms = state.runtime.policy().max_confirms_per_turn;
                let (batch_with, remaining) = collect_confirm_batch(
                    state,
                    pending.risk,
                    first_is_shell,
                    rest,
                    *confirms_used,
                    max_confirms,
                )?;
                let batch_size = batch_with.len() + 1;
                tracing::info!(
                    batch_size,
                    tool = %pending.name,
                    first_is_shell,
                    confirms_used = *confirms_used,
                    max_confirms,
                    "HITL batch confirm pause"
                );
                let speak_prompt = if batch_with.is_empty() {
                    speak_prompt
                } else {
                    speak_batch_confirm_prompt(&pending, &batch_with, first_is_shell)
                };
                let pending_turn = PendingTurn {
                    pending: pending.clone(),
                    batch_with,
                    remaining_calls: remaining,
                    tools_used: tools_used.clone(),
                    tool_rounds,
                    confirms_used: *confirms_used,
                    user_text: user_text.to_string(),
                    // Enriched by the loop with live todos/budgets/accounting.
                    todos_file: None,
                    markup_left: crate::finish_gate::FinishGateBudget::MARKUP_INIT,
                    gate_left: 0,
                    token_accounting: crate::types::TokenAccounting::default(),
                };
                return Ok((
                    ToolBatchResult::Paused {
                        outcome: AgentOutcome::NeedsConfirmation {
                            text: speak_prompt,
                            pending,
                        },
                        pending_turn,
                    },
                    Vec::new(),
                ));
            }
        }
        if single_step {
            return Ok((ToolBatchResult::Continue, iter.collect()));
        }
    }
    Ok((ToolBatchResult::Continue, Vec::new()))
}

/// Collect a contiguous prefix of remaining calls that need confirmation,
/// share risk with `first_risk`, and match shell-ness of the first pending.
///
/// Shell tools batch with other shell tools only (never mixed with file writes).
/// Non-shell tools batch with non-shell only.
///
/// Budget-aware batch-HITL: one yes/no prompt still covers `pending` plus the
/// returned `batch_with`, but the batch is capped so the turn's
/// `max_confirms_per_turn` is never exceeded by batching alone. If
/// `confirms_used (already including pending) + batch_with.len() + 1 > max`,
/// only as many siblings as fit are taken; the overflow stays at the head of
/// `remaining` for a later prompt. Callers pass the post-increment
/// `confirms_used` (pending already counted).
fn collect_confirm_batch(
    state: &LoopState<'_>,
    first_risk: ToolRisk,
    first_is_shell: bool,
    remaining: Vec<RawToolCall>,
    confirms_used: u32,
    max_confirms: u32,
) -> Result<(Vec<RawToolCall>, Vec<RawToolCall>), AgentError> {
    let mut batch_with = Vec::new();
    let mut iter = remaining.into_iter();
    while let Some(call) = iter.next() {
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            // Unknown tool: leave for sequential soft-fail, stop batching.
            let mut rest = vec![call];
            rest.extend(iter);
            return Ok((batch_with, rest));
        };
        let meta = tool.meta();
        let is_shell = meta.permissions.contains(&Permission::Shell);

        // Never mix shell with non-shell in one HITL decision.
        if is_shell != first_is_shell {
            let mut rest = vec![call];
            rest.extend(iter);
            return Ok((batch_with, rest));
        }

        // Same risk only (e.g. all Dangerous file writes / bash).
        if meta.risk != first_risk {
            let mut rest = vec![call];
            rest.extend(iter);
            return Ok((batch_with, rest));
        }

        // Natural need-confirm check (confirms_used: 0 so cap does not deny
        // siblings that should share this single HITL decision).
        let opts = InvokeOptions {
            skip_confirmation: false,
            confirms_used: 0,
        };
        match state.runtime.decide_only(tool, &call.args, opts) {
            PolicyDecision::NeedsConfirmation { .. } => {
                // Budget-aware split: each batched sibling counts against
                // `max_confirms_per_turn` even though the UI shows one prompt.
                // `confirms_used` already includes `pending`.
                if confirms_used
                    .saturating_add(batch_with.len() as u32)
                    .saturating_add(1)
                    > max_confirms.max(1)
                {
                    let mut rest = vec![call];
                    rest.extend(iter);
                    tracing::debug!(
                        confirms_used,
                        max_confirms,
                        batched = batch_with.len(),
                        "HITL batch capped by confirm budget; leaving overflow for later"
                    );
                    return Ok((batch_with, rest));
                }
                batch_with.push(call);
            }
            _ => {
                // Allow or Deny — not part of this confirm batch.
                let mut rest = vec![call];
                rest.extend(iter);
                return Ok((batch_with, rest));
            }
        }
    }
    Ok((batch_with, Vec::new()))
}

/// Voice-friendly multi-action confirm prompt (kept short for TTS latency).
fn speak_batch_confirm_prompt(
    pending: &PendingToolCall,
    batch_with: &[RawToolCall],
    first_is_shell: bool,
) -> String {
    let n = batch_with.len() + 1;
    let mut parts: Vec<String> = Vec::with_capacity(n.min(4));
    parts.push(voice_item_for_pending(pending));
    for call in batch_with.iter().take(3) {
        parts.push(voice_item_for_call(call));
    }
    let more = if batch_with.len() > 3 {
        format!(", +{}", batch_with.len() - 3)
    } else {
        String::new()
    };
    let list = parts.join("; ");
    if first_is_shell {
        format!("Run {n} commands: {list}{more}?")
    } else {
        format!("Run {n} actions: {list}{more}?")
    }
}

fn voice_item_for_pending(pending: &PendingToolCall) -> String {
    if pending.name == "bash" {
        if let Some(cmd) = pending
            .args
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return truncate_voice(cmd, 40);
        }
    }
    truncate_voice(&pending.args_summary, 40)
}

fn voice_item_for_call(call: &RawToolCall) -> String {
    if call.name == "bash" {
        if let Some(cmd) = call
            .args
            .get("command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return truncate_voice(cmd, 40);
        }
    }
    let s = args_summary(&call.name, &call.args);
    truncate_voice(&s, 40)
}

fn truncate_voice(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Parallel path for batches where every call is auto-allowed or denyable.
///
/// Invokes in a rolling pool bounded by `max_parallel_tools`;
/// only mutates context after all results are collected, preserving original
/// call order.
///
/// Completed calls are committed exactly once. Only unresolved pauses fall
/// back to sequential execution; successful siblings are never replayed.
#[allow(clippy::too_many_arguments)]
async fn run_tool_batch_parallel(
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
    let opts = InvokeOptions {
        skip_confirmation: false,
        confirms_used: *confirms_used,
    };
    let max_par = clamp_parallel(calls.len(), config.features.max_parallel_tools);
    if calls.is_empty() {
        return Ok(ToolBatchResult::Continue);
    }
    let batch_started = Instant::now();
    let batch_len = calls.len();
    emit(AgentEvent::ToolBatchStart {
        call_ids: calls.iter().map(|call| call.call_id.clone()).collect(),
        max_parallel: max_par as u32,
    });
    let tools = state.tools;
    let runtime = state.runtime;
    let futs: Vec<_> = calls
        .into_iter()
        .enumerate()
        .map(|(index, call)| {
            let cancel = cancel.clone();
            async move {
                emit_tool_start(emit, &call);
                let started = Instant::now();
                let inv = build_tool_invocation(&call, config, cancel, emit);
                let result = match find_tool_opt(tools, &call.name) {
                    Some(tool) => runtime.invoke(tool, inv, opts).await,
                    None => {
                        runtime.audit_unknown_tool(
                            &call.name,
                            &call.args,
                            config.session_id.as_deref(),
                            config.turn_id.as_deref(),
                        );
                        InvokeResult::Observation(ToolObservation::err(
                            ObservationError::new(
                                "unknown_tool",
                                false,
                                unknown_tool_observation(&call.name)
                                    .trim_start_matches("Error: ")
                                    .to_string(),
                            ),
                            0,
                        ))
                    }
                };
                let duration_ms = started.elapsed().as_millis() as u64;
                let ok = match &result {
                    InvokeResult::Observation(observation) => Some(observation.looks_ok()),
                    InvokeResult::Denied { .. } => Some(false),
                    _ => None,
                };
                if let Some(ok) = ok {
                    emit_tool_end(emit, &call, ok, duration_ms);
                }
                (index, call, result)
            }
        })
        .collect();
    let run = stream::iter(futs)
        .buffer_unordered(max_par.max(1))
        .collect::<Vec<_>>();
    let mut results = if let Some(token) = &cancel {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                emit(AgentEvent::ToolBatchEnd {
                    tool_count: batch_len, duration_ms: batch_started.elapsed().as_millis() as u64,
                    failed: 0, paused: 0, cancelled: true,
                });
                return Err(AgentError::cancelled("parallel batch cancelled"));
            },
            results = run => results,
        }
    } else {
        run.await
    };
    results.sort_by_key(|(index, _, _)| *index);
    let mut unresolved = Vec::new();
    let mut failed = 0;
    for (_, call, result) in results {
        if matches!(
            result,
            InvokeResult::NeedsConfirmation { .. } | InvokeResult::NeedsInput { .. }
        ) {
            unresolved.push(call);
        } else {
            let (content, ok) = parallel_batch_observation(result);
            failed += usize::from(!ok);
            tools_used.push(call.name.clone());
            push_tool_result_messages(state.context, &call.name, &call.call_id, content, ok);
        }
    }
    emit(AgentEvent::ToolBatchEnd {
        tool_count: batch_len,
        failed,
        paused: unresolved.len(),
        cancelled: false,
        duration_ms: batch_started.elapsed().as_millis() as u64,
    });
    if !unresolved.is_empty() {
        return run_tool_batch_sequential(
            state,
            unresolved,
            tools_used,
            tool_rounds,
            confirms_used,
            user_text,
            config,
            emit,
            cancel,
        )
        .await;
    }

    Ok(ToolBatchResult::Continue)
}

/// Wave scheduling preserves barriers: read/read → write → read/read.
/// Reads after a write never move ahead of it, and safe reads can still run
/// together before or after a confirmation boundary.
#[allow(clippy::too_many_arguments)]
async fn run_tool_batch_waves(
    state: &mut LoopState<'_>,
    mut calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    tool_rounds: u32,
    confirms_used: &mut u32,
    user_text: &str,
    config: &AgentLoopConfig,
    emit: &EmitFn,
    cancel: Option<CancellationToken>,
) -> Result<ToolBatchResult, AgentError> {
    while !calls.is_empty() {
        let (reads, rest) = split_auto_readonly_prefix(state, calls, *confirms_used);
        let (result, tail) = if reads.len() > 1 {
            let result = run_tool_batch_parallel(
                state,
                reads,
                tools_used,
                tool_rounds,
                confirms_used,
                user_text,
                config,
                emit,
                cancel.clone(),
            )
            .await?;
            (result, rest)
        } else {
            // One read or an ordering barrier. Give the sequential handler the
            // full tail so compatible confirmations can still share a prompt.
            let mut next = reads;
            next.extend(rest);
            run_tool_batch_sequential_inner(
                state,
                next,
                tools_used,
                tool_rounds,
                confirms_used,
                user_text,
                config,
                emit,
                cancel.clone(),
                true,
            )
            .await?
        };
        match result {
            ToolBatchResult::Continue => calls = tail,
            ToolBatchResult::Paused {
                outcome,
                mut pending_turn,
            } => {
                pending_turn.remaining_calls.extend(tail);
                return Ok(ToolBatchResult::Paused {
                    outcome,
                    pending_turn,
                });
            }
        }
    }
    Ok(ToolBatchResult::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::json;
    use std::sync::Arc;

    use crate::context::Context;
    use crate::runtime::ToolRuntime;
    use crate::tool::{Tool, ToolError, ToolMeta, ToolRisk};
    use crate::types::AgentLoopConfig;

    struct NoopClient;
    #[async_trait]
    impl LlmClient for NoopClient {
        async fn complete(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
        ) -> Result<serde_json::Value, LlmError> {
            Err(LlmError::new("noop"))
        }
        fn model(&self) -> &str {
            "test"
        }
    }

    struct DangerWrite {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for DangerWrite {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "dangerous write"
        }
        fn parameters(&self) -> serde_json::Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::with_risk(ToolRisk::Dangerous)
                .permissions(&[Permission::FsWrite])
                .confirm(true)
                .read_only(false)
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: serde_json::Value,
        ) -> Result<String, ToolError> {
            Ok("wrote".into())
        }
    }

    #[tokio::test]
    async fn batched_collect_input_pauses_and_preserves_remaining_calls() {
        let tools: Vec<Arc<dyn Tool>> =
            vec![Arc::new(crate::tools::collect_input::CollectInputTool)];
        let runtime = ToolRuntime::null();
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;
        let calls = vec![
            RawToolCall {
                call_id: "input-1".into(),
                name: "collect_input".into(),
                args: json!({"spoken": "Type the first value.", "label": "First"}),
            },
            RawToolCall {
                call_id: "input-2".into(),
                name: "collect_input".into(),
                args: json!({"spoken": "Type the second value.", "label": "Second"}),
            },
        ];
        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "collect two values",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();

        match result {
            ToolBatchResult::Paused {
                outcome,
                pending_turn,
            } => {
                assert!(matches!(outcome, AgentOutcome::NeedsInput { .. }));
                assert!(pending_turn.pending.input.is_some());
                assert_eq!(pending_turn.pending.call_id, "input-1");
                assert_eq!(pending_turn.remaining_calls.len(), 1);
                assert_eq!(pending_turn.remaining_calls[0].call_id, "input-2");
            }
            ToolBatchResult::Continue => panic!("collect_input batch must pause"),
        }
    }

    #[tokio::test]
    async fn sequential_batches_two_dangerous_confirms() {
        // trusted off + two Dangerous non-shell tools → one HITL with batch_with len 1.
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(DangerWrite { name: "file_write" }),
            Arc::new(DangerWrite { name: "file_edit" }),
        ];
        let runtime = ToolRuntime::null(); // default: trusted_auto_moderate = false
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;

        // Empty args: no path hard-gate; risk + confirm flag alone force HITL.
        let calls = vec![
            RawToolCall {
                call_id: "c1".into(),
                name: "file_write".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "c2".into(),
                name: "file_edit".into(),
                args: json!({}),
            },
        ];

        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "write two files",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();

        match result {
            ToolBatchResult::Paused {
                pending_turn,
                outcome,
            } => {
                assert_eq!(pending_turn.pending.name, "file_write");
                assert_eq!(pending_turn.batch_with.len(), 1);
                assert_eq!(pending_turn.batch_with[0].name, "file_edit");
                assert!(pending_turn.remaining_calls.is_empty());
                assert_eq!(pending_turn.confirms_used, 1);
                assert_eq!(confirms_used, 1);
                match outcome {
                    AgentOutcome::NeedsConfirmation { text, .. } => {
                        assert!(
                            text.contains("2 actions") || text.contains("Run 2"),
                            "expected batch speak prompt, got: {text}"
                        );
                    }
                    other => panic!("unexpected outcome {other:?}"),
                }
            }
            ToolBatchResult::Continue => panic!("expected HITL pause with batch"),
        }
    }

    #[tokio::test]
    async fn sequential_does_not_batch_shell_with_writes() {
        struct ShellTool;
        #[async_trait]
        impl Tool for ShellTool {
            fn name(&self) -> &str {
                "bash"
            }
            fn description(&self) -> &str {
                "shell"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::with_risk(ToolRisk::Dangerous)
                    .permissions(&[Permission::Shell])
                    .confirm(true)
                    .read_only(false)
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("ran".into())
            }
        }

        // OpenConfirm shell so hard gate does not Deny.
        let policy = crate::runtime::SandboxConfig {
            shell: crate::runtime::ShellPolicy::OpenConfirm,
            ..Default::default()
        };
        let runtime = ToolRuntime::new(policy, Box::new(crate::runtime::NullAuditSink));

        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(DangerWrite { name: "file_write" }),
            Arc::new(ShellTool),
            Arc::new(DangerWrite { name: "file_edit" }),
        ];
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;

        let calls = vec![
            RawToolCall {
                call_id: "c1".into(),
                name: "file_write".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "c2".into(),
                name: "bash".into(),
                args: json!({ "command": "echo hi" }),
            },
            RawToolCall {
                call_id: "c3".into(),
                name: "file_edit".into(),
                args: json!({}),
            },
        ];

        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "mixed",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();

        match result {
            ToolBatchResult::Paused { pending_turn, .. } => {
                assert_eq!(pending_turn.pending.name, "file_write");
                // Shell never mixes into a non-shell confirm batch.
                assert!(pending_turn.batch_with.is_empty());
                assert_eq!(pending_turn.remaining_calls.len(), 2);
                assert_eq!(pending_turn.remaining_calls[0].name, "bash");
                assert_eq!(pending_turn.remaining_calls[1].name, "file_edit");
            }
            ToolBatchResult::Continue => panic!("expected HITL pause"),
        }
    }

    #[tokio::test]
    async fn sequential_batches_contiguous_shell() {
        struct ShellTool;
        #[async_trait]
        impl Tool for ShellTool {
            fn name(&self) -> &str {
                "bash"
            }
            fn description(&self) -> &str {
                "shell"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::with_risk(ToolRisk::Dangerous)
                    .permissions(&[Permission::Shell])
                    .confirm(true)
                    .read_only(false)
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                Ok("ran".into())
            }
        }

        let policy = crate::runtime::SandboxConfig {
            shell: crate::runtime::ShellPolicy::OpenConfirm,
            ..Default::default()
        };
        let runtime = ToolRuntime::new(policy, Box::new(crate::runtime::NullAuditSink));

        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(ShellTool)];
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;

        let calls = vec![
            RawToolCall {
                call_id: "c1".into(),
                name: "bash".into(),
                args: json!({ "command": "echo one" }),
            },
            RawToolCall {
                call_id: "c2".into(),
                name: "bash".into(),
                args: json!({ "command": "echo two" }),
            },
            RawToolCall {
                call_id: "c3".into(),
                name: "bash".into(),
                args: json!({ "command": "echo three" }),
            },
        ];

        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "three shells",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();

        match result {
            ToolBatchResult::Paused {
                pending_turn,
                outcome,
            } => {
                assert_eq!(pending_turn.pending.name, "bash");
                assert_eq!(pending_turn.batch_with.len(), 2);
                assert!(pending_turn.remaining_calls.is_empty());
                assert_eq!(confirms_used, 1);
                match outcome {
                    AgentOutcome::NeedsConfirmation { text, .. } => {
                        assert!(
                            text.contains("3 commands") || text.contains("echo"),
                            "expected shell batch speak prompt, got: {text}"
                        );
                    }
                    other => panic!("unexpected outcome {other:?}"),
                }
            }
            ToolBatchResult::Continue => panic!("expected HITL pause with shell batch"),
        }
    }

    #[tokio::test]
    async fn sequential_soft_fails_unknown_tool_and_continues() {
        // Model invents a name not in the session table — must not abort the turn.
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(DangerWrite { name: "file_write" })];
        let runtime = ToolRuntime::null();
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;

        let calls = vec![
            RawToolCall {
                call_id: "u1".into(),
                name: "todo_write".into(), // not registered in this test table
                args: json!({}),
            },
            RawToolCall {
                call_id: "c1".into(),
                name: "file_write".into(),
                args: json!({}),
            },
        ];

        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };

        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "continue",
            &config,
            &emit,
            None,
        )
        .await
        .expect("unknown tool must soft-fail, not return AgentError");

        // First call soft-failed; second still pauses for HITL.
        assert!(tools_used.iter().any(|n| n == "todo_write"));
        match result {
            ToolBatchResult::Paused { pending_turn, .. } => {
                assert_eq!(pending_turn.pending.name, "file_write");
            }
            ToolBatchResult::Continue => panic!("expected HITL on second tool"),
        }

        let tool_msgs: Vec<_> = context
            .messages()
            .iter()
            .filter(|m| matches!(m.role, crate::context::Role::Tool))
            .collect();
        assert_eq!(tool_msgs.len(), 1);
        let content = tool_msgs[0].content["content"].as_str().unwrap_or("");
        assert!(
            content.contains("not available"),
            "expected soft-fail observation, got: {content}"
        );
    }

    #[tokio::test]
    async fn confirm_batch_is_capped_by_max_confirms_per_turn() {
        // B3: batch-HITL still shows one prompt but is budget-aware. With
        // max=2, a 4-tool Dangerous batch must split: pending + 1 sibling in
        // the first pause, 2 left in `remaining` for a later prompt.
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(DangerWrite { name: "w1" }),
            Arc::new(DangerWrite { name: "w2" }),
            Arc::new(DangerWrite { name: "w3" }),
            Arc::new(DangerWrite { name: "w4" }),
        ];
        let policy = crate::runtime::SandboxConfig {
            max_confirms_per_turn: 2,
            ..Default::default()
        };
        let runtime = ToolRuntime::new(policy, Box::new(crate::runtime::NullAuditSink));
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;
        let calls = (1..=4)
            .map(|i| RawToolCall {
                call_id: format!("c{i}"),
                name: format!("w{i}"),
                args: json!({}),
            })
            .collect::<Vec<_>>();

        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = process_tool_calls(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "write four",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();
        match result {
            ToolBatchResult::Paused { pending_turn, .. } => {
                assert_eq!(pending_turn.pending.name, "w1");
                // 1 pending + 1 batched = max 2; overflow stays remaining.
                assert_eq!(
                    pending_turn.batch_with.len(),
                    1,
                    "only as many siblings as fit the confirm budget"
                );
                assert_eq!(pending_turn.batch_with[0].name, "w2");
                assert_eq!(pending_turn.remaining_calls.len(), 2);
                assert_eq!(pending_turn.remaining_calls[0].name, "w3");
                assert_eq!(pending_turn.remaining_calls[1].name, "w4");
                assert_eq!(pending_turn.confirms_used, 1);
                assert_eq!(confirms_used, 1);
            }
            ToolBatchResult::Continue => panic!("expected capped HITL pause"),
        }
    }

    #[tokio::test]
    async fn parallel_falls_back_to_sequential_on_unexpected_confirm() {
        // B12: force the parallel path with a confirm-needed batch (bypassing
        // preflight). It must fall back to sequential and pause — never emit
        // "Error: unexpected confirmation..." text.
        struct Safe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        #[async_trait]
        impl Tool for Safe {
            fn name(&self) -> &str {
                "safe"
            }
            fn description(&self) -> &str {
                "safe"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(
                &self,
                _ctx: &crate::tool_context::ToolCallContext,
                _args: serde_json::Value,
            ) -> Result<String, ToolError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok("ok".into())
            }
        }
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(Safe(executions.clone())),
            Arc::new(DangerWrite { name: "danger" }),
        ];
        let runtime = ToolRuntime::null();
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;
        let calls = vec![
            RawToolCall {
                call_id: "c1".into(),
                name: "safe".into(),
                args: json!({}),
            },
            RawToolCall {
                call_id: "c2".into(),
                name: "danger".into(),
                args: json!({}),
            },
        ];
        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        // Direct parallel call (preflight would have routed to sequential).
        let result = run_tool_batch_parallel(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "fallback",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            executions.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "completed siblings must never be replayed"
        );
        assert_eq!(tools_used, ["safe"]);
        match result {
            ToolBatchResult::Paused { pending_turn, .. } => {
                assert_eq!(pending_turn.pending.name, "danger");
            }
            ToolBatchResult::Continue => {
                panic!("parallel fallback must pause, not swallow HITL as error text")
            }
        }
        // No "unexpected confirmation" error text may leak into context.
        for m in context.messages() {
            if matches!(m.role, crate::context::Role::Tool) {
                let s = m.content["content"].as_str().unwrap_or("");
                assert!(
                    !s.contains("unexpected confirmation"),
                    "fallback must not emit parallel-batch error, got: {s}"
                );
            }
        }
    }

    #[tokio::test]
    async fn waves_fall_back_to_sequential_on_unexpected_input() {
        // B12 wave path: a collect_input call smuggled into the wave batch
        // must also fall back to sequential and pause for input.
        let tools: Vec<Arc<dyn Tool>> =
            vec![Arc::new(crate::tools::collect_input::CollectInputTool)];
        let runtime = ToolRuntime::null();
        let mut context = Context::new(20);
        let client = NoopClient;
        let config = AgentLoopConfig::default();
        let emit: EmitFn = Arc::new(|_| {});
        let mut tools_used = Vec::new();
        let mut confirms_used = 0u32;
        let calls = vec![
            RawToolCall {
                call_id: "i1".into(),
                name: "collect_input".into(),
                args: json!({"spoken": "Type it.", "label": "L"}),
            },
            RawToolCall {
                call_id: "i2".into(),
                name: "collect_input".into(),
                args: json!({"spoken": "Type it.", "label": "L"}),
            },
        ];
        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = run_tool_batch_waves(
            &mut state,
            calls,
            &mut tools_used,
            1,
            &mut confirms_used,
            "wave fallback",
            &config,
            &emit,
            None,
        )
        .await
        .unwrap();
        match result {
            ToolBatchResult::Paused { pending_turn, .. } => {
                assert!(pending_turn.pending.input.is_some());
            }
            ToolBatchResult::Continue => panic!("wave fallback must pause for input"),
        }
    }
}
