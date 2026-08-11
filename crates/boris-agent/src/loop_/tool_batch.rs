//! Execute a batch of tool calls from one assistant message.
//!
//! Dispatch rules:
//! - Length 1 → sequential path.
//! - Length > 1 → preflight with [`ToolRuntime::decide_only`]; if any call needs
//!   confirmation, fall back to sequential (HITL-safe). Otherwise:
//!   - **wave scheduling** (default): read-only wave (parallel, chunked) then
//!     write wave (sequential)
//!   - else: legacy parallel path — all auto-allow calls, still chunked by
//!     `max_parallel_tools` (no unbounded `join_all`)
//!
//! Sequential batches can pause mid-batch for HITL and keep remaining siblings.
//! Contiguous confirm-needed calls that share risk **and shell-ness** are
//! batched into one HITL decision (`PendingTurn::batch_with`):
//! - multiple file writes → one yes
//! - multiple bash commands → one yes
//! - shell is never mixed with non-shell in the same prompt

use std::time::Instant;

use futures::future::join_all;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    args_summary, clamp_parallel, partition_read_write, InvokeOptions, InvokeResult, PendingToolCall,
    PendingTurn, PolicyDecision, RawToolCall,
};
use crate::tool::{Permission, Tool, ToolRisk};
use crate::types::{AgentLoopConfig, EmitFn};

use super::helpers::{
    build_tool_invocation, commit_tool_observation, emit_tool_start, find_tool_opt,
    observation_looks_ok, parallel_batch_observation, unknown_tool_observation,
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
                max_parallel = config.features.max_parallel_tools,
                "tool batch: parallel dispatch (legacy, chunked)"
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
/// Unknown tools are treated as non-confirm (soft-failed later with an error observation).
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
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            continue;
        };
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
        emit_tool_start(emit, &call);
        let started = Instant::now();

        // Soft-fail unknown tools so prompt/history drift cannot kill the turn.
        let Some(tool) = find_tool_opt(state.tools, &call.name) else {
            tracing::warn!(tool = %call.name, "model requested unknown tool; soft-failing");
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
                // One HITL decision covers pending + compatible siblings.
                *confirms_used = confirms_used.saturating_add(1);
                let rest: Vec<RawToolCall> = iter.collect();
                let first_is_shell = tool.meta().permissions.contains(&Permission::Shell);
                let (batch_with, remaining) =
                    collect_confirm_batch(state, pending.risk, first_is_shell, rest)?;
                let batch_size = batch_with.len() + 1;
                tracing::info!(
                    batch_size,
                    tool = %pending.name,
                    first_is_shell,
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

/// Collect a contiguous prefix of remaining calls that need confirmation,
/// share risk with `first_risk`, and match shell-ness of the first pending.
///
/// Shell tools batch with other shell tools only (never mixed with file writes).
/// Non-shell tools batch with non-shell only.
fn collect_confirm_batch(
    state: &LoopState<'_>,
    first_risk: ToolRisk,
    first_is_shell: bool,
    remaining: Vec<RawToolCall>,
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
/// Invokes in chunks of `max_parallel_tools` (same clamp as the wave path);
/// only mutates context after all results are collected, preserving original
/// call order.
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
    let max_par = clamp_parallel(calls.len(), config.features.max_parallel_tools);
    let mut results: Vec<(u64, InvokeResult)> = Vec::with_capacity(calls.len());

    // Chunk so we never spawn an unbounded join_all over the full batch.
    for chunk in calls.chunks(max_par.max(1)) {
        let chunk_results = {
            let tools = state.tools;
            let runtime = state.runtime;

            // Resolve tools first; unknown names become instant error observations.
            let mut resolved: Vec<Option<&dyn Tool>> = Vec::with_capacity(chunk.len());
            for call in chunk {
                emit_tool_start(emit, call);
                resolved.push(find_tool_opt(tools, &call.name));
            }

            let futs = chunk.iter().zip(resolved).map(|(call, tool)| {
                let inv = build_tool_invocation(call, config, cancel.clone(), emit);
                let started = Instant::now();
                let name = call.name.clone();
                async move {
                    let result = match tool {
                        Some(tool) => runtime.invoke(tool, inv, opts).await,
                        None => {
                            tracing::warn!(tool = %name, "model requested unknown tool; soft-failing");
                            InvokeResult::Observation(unknown_tool_observation(&name))
                        }
                    };
                    (started.elapsed().as_millis() as u64, result)
                }
            });

            join_all(futs).await
        };
        results.extend(chunk_results);
    }

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
    // Contract with `partition_read_write`: every index in 0..calls.len() lands
    // in exactly one of read_idx/write_idx (checked below, right before the
    // `ordered[i]` invariant it backs is relied on via `.expect()`).
    let partitioned_len = read_idx.len() + write_idx.len();
    let max_par = clamp_parallel(read_idx.len(), config.features.max_parallel_tools);

    // Pre-size results in original order.
    let mut ordered: Vec<Option<(u64, InvokeResult)>> = (0..calls.len()).map(|_| None).collect();

    // Read-only wave in chunks of max_par.
    for chunk in read_idx.chunks(max_par.max(1)) {
        let results = {
            let tools = state.tools;
            let runtime = state.runtime;
            let mut pairs: Vec<(&RawToolCall, Option<&dyn Tool>)> = Vec::new();
            for &i in chunk {
                let call = &calls[i];
                emit_tool_start(emit, call);
                pairs.push((call, find_tool_opt(tools, &call.name)));
            }
            let futs = pairs.into_iter().map(|(call, tool)| {
                let inv = build_tool_invocation(call, config, cancel.clone(), emit);
                let started = Instant::now();
                let name = call.name.clone();
                async move {
                    let result = match tool {
                        Some(tool) => runtime.invoke(tool, inv, opts).await,
                        None => {
                            tracing::warn!(tool = %name, "model requested unknown tool; soft-failing");
                            InvokeResult::Observation(unknown_tool_observation(&name))
                        }
                    };
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
        emit_tool_start(emit, call);
        let started = Instant::now();
        let result = match find_tool_opt(state.tools, &call.name) {
            Some(tool) => {
                let inv = build_tool_invocation(call, config, cancel.clone(), emit);
                state.runtime.invoke(tool, inv, opts).await
            }
            None => {
                tracing::warn!(tool = %call.name, "model requested unknown tool; soft-failing");
                InvokeResult::Observation(unknown_tool_observation(&call.name))
            }
        };
        ordered[i] = Some((started.elapsed().as_millis() as u64, result));
    }

    // Every `ordered[i]` must have been filled by exactly one of the read/write
    // waves above — guaranteed by `partition_read_write` partitioning every
    // index into exactly one of read_idx/write_idx (see `runtime::concurrency`
    // unit tests for the checked contract). Debug-only: cheap, and a violation
    // here would mean the `.expect()` below is masking a real bug.
    debug_assert_eq!(
        partitioned_len,
        calls.len(),
        "partition_read_write must assign every call index to exactly one wave"
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::json;
    use std::sync::Arc;

    use crate::context::Context;
    use crate::runtime::ToolRuntime;
    use crate::tool::{ToolError, ToolMeta, ToolRisk};
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
            ToolBatchResult::Paused { pending_turn, outcome } => {
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
        let mut policy = crate::runtime::SandboxConfig::default();
        policy.shell = crate::runtime::ShellPolicy::OpenConfirm;
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

        let mut policy = crate::runtime::SandboxConfig::default();
        policy.shell = crate::runtime::ShellPolicy::OpenConfirm;
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
}
