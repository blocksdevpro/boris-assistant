//! Pure ReAct agent loop (tau-inspired).
//!
//! No personal memory, session I/O, or host side effects — only LLM complete +
//! [`ToolRuntime`] mediation. The [`crate::Agent`] facade owns state and learning.
//!
//! # Module layout
//!
//! - [`message_parse`] — tool-call / reply text parsing from LLM JSON
//! - [`helpers`] — tool lookup, listing, invocation setup, observation writes
//! - [`tool_batch`] — sequential / parallel / wave-scheduling batch execution
//! - [`round`] — per-round LLM complete, tool-call gating, finish-gate checks
//! - [`finish`] — terminal emit helpers (speech / HITL pause)
//!
//! Tool batches that may need HITL run sequentially so remaining sibling calls
//! can be paused. Auto-allow batches (no confirmation needed) run in parallel
//! via `join_all` (or read/write waves), preserving original order in context.

mod finish;
mod helpers;
mod message_parse;
mod round;
mod tool_batch;

use std::time::Instant;

use boris_ai::LlmClient;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::context::{Context, Role};
use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    ActivationSet, InvokeOptions, InvokeResult, PendingTurn, RawToolCall, ToolRuntime,
};
use crate::tool::Tool;
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn, LoopResult};

use finish::{finish_paused, finish_with_speech, noop_emit};
use helpers::{
    build_tool_invocation, find_tool, observation_looks_ok, tool_observation_json,
};
use message_parse::{extract_reply_text, parse_raw_tool_calls};
use round::{
    cancelled, complete_round, ensure_spoken_reply_at_cap, should_reenter_finish_gate,
    tool_calls_if_runnable, NUDGE_NEAR_TOOL_CAP,
};
use tool_batch::{process_tool_calls, ToolBatchResult};

/// Mutable state the loop may write back (context + tools + runtime).
pub struct LoopState<'a> {
    pub context: &'a mut Context,
    pub tools: &'a [std::sync::Arc<dyn Tool>],
    pub runtime: &'a ToolRuntime,
    pub client: &'a dyn LlmClient,
    /// Session activation set (tool_search). Optional when progressive is off.
    pub activated: Option<&'a ActivationSet>,
}

/// Run the ReAct loop until a final reply, HITL pause, cancel, or error.
///
/// `user_text` is only used for pending-turn bookkeeping (post-turn learn).
/// Context must already contain the user message (and any prior history).
pub async fn agent_loop(
    mut state: LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    tools_used: Vec<String>,
    tool_rounds: u32,
    confirms_used: u32,
    cancel: Option<CancellationToken>,
    emit: Option<EmitFn>,
    sandbox_root: Option<std::path::PathBuf>,
    mut finish_gate_left: u32,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    emit(AgentEvent::AgentStart);

    let mut tools_used = tools_used;
    let mut tool_rounds = tool_rounds;
    let mut confirms_used = confirms_used;
    let max_rounds = config.max_tool_rounds as usize;
    let sandbox_root = sandbox_root.unwrap_or_else(crate::finish_gate::default_sandbox_guess);
    // Last non-empty spoken line this turn (finish-gate re-entry must not discard it).
    let mut last_speakable: Option<String> = None;

    for round in 0..=max_rounds {
        if cancelled(&cancel) {
            emit(AgentEvent::Error {
                message: "cancelled".into(),
            });
            return Err(AgentError::cancelled("agent loop cancelled"));
        }

        emit(AgentEvent::TurnStart {
            round: round as u32,
        });

        // Mechanical compaction before each LLM call.
        state.context.compact_mechanical();

        // On the final allowed round, withhold tools so the model must speak.
        let at_cap = round >= max_rounds;
        let response = complete_round(&state, config, at_cap).await?;

        if let Some(batch) = tool_calls_if_runnable(&response, at_cap) {
            // One round before cap: run tools, then inject a finish nudge and
            // continue so the next iteration (at_cap) produces a spoken reply.
            let force_finish_next = round + 1 >= max_rounds;

            tool_rounds += 1;
            state.context.push(Role::Assistant, response.clone());
            emit(AgentEvent::MessageEnd {
                role: Role::Assistant,
                preview: format!("{} tool call(s)", batch.len()),
            });

            let raw_calls = parse_raw_tool_calls(batch);

            match process_tool_calls(
                &mut LoopState {
                    context: state.context,
                    tools: state.tools,
                    runtime: state.runtime,
                    client: state.client,
                    activated: state.activated,
                },
                raw_calls,
                &mut tools_used,
                tool_rounds,
                &mut confirms_used,
                user_text,
                config,
                &emit,
                cancel.clone(),
            )
            .await?
            {
                ToolBatchResult::Continue => {
                    if force_finish_next {
                        state.context.push(Role::User, json!(NUDGE_NEAR_TOOL_CAP));
                    }
                    emit(AgentEvent::TurnEnd {
                        round: round as u32,
                    });
                    continue;
                }
                ToolBatchResult::Paused {
                    outcome,
                    pending_turn,
                } => {
                    return finish_paused(
                        &emit,
                        round as u32,
                        outcome,
                        tool_rounds,
                        tools_used,
                        pending_turn,
                    );
                }
            }
        }

        // Content-only response (or tools withheld / ignored at cap).
        let mut reply = extract_reply_text(&response);
        reply = ensure_spoken_reply_at_cap(&mut state, at_cap, reply).await?;

        if !reply.is_empty() {
            last_speakable = Some(reply.clone());
        }

        // Finish gate: only when *this turn* used tools and open todos remain.
        // Stale todos from a prior session must not force re-entry (and silence)
        // on a casual "what are you doing?" reply.
        if should_reenter_finish_gate(at_cap, finish_gate_left, &reply, &tools_used, &sandbox_root)
        {
            finish_gate_left = finish_gate_left.saturating_sub(1);
            let pending = crate::finish_gate::pending_todo_count(&sandbox_root);
            tracing::info!(
                pending,
                left = finish_gate_left,
                "finish gate: open todos — continue tooling"
            );
            state.context.push(
                Role::User,
                crate::finish_gate::todo_gate_reminder(pending),
            );
            emit(AgentEvent::TurnEnd {
                round: round as u32,
            });
            continue;
        }

        // Prefer current reply; if finish-gate re-entry went silent, keep earlier speech.
        if reply.is_empty() {
            if let Some(prev) = last_speakable.clone() {
                reply = prev;
            }
        }

        return finish_with_speech(&emit, round as u32, reply, tool_rounds, tools_used);
    }

    // Unreachable with 0..=max_rounds + return inside, but keep a soft landing.
    Ok(LoopResult {
        outcome: AgentOutcome::speak(
            "I ran out of steps on that. Ask me to pick it up again.",
        ),
        tool_rounds,
        tools_used,
        pending_turn: None,
    })
}

/// Execute one already-approved (or rejected) pending tool, then remaining siblings.
pub async fn resume_pending_tool(
    mut state: LoopState<'_>,
    pending_turn: PendingTurn,
    approved: bool,
    config: &AgentLoopConfig,
    emit: Option<EmitFn>,
    cancel: Option<CancellationToken>,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    let mut tools_used = pending_turn.tools_used;
    let tool_rounds = pending_turn.tool_rounds;
    let mut confirms_used = pending_turn.confirms_used;
    let remaining = pending_turn.remaining_calls;
    let pending = pending_turn.pending;
    let user_text = pending_turn.user_text;

    let tool = find_tool(state.tools, &pending.name)?;

    emit(AgentEvent::ToolExecutionStart {
        call_id: pending.call_id.clone(),
        tool_name: pending.name.clone(),
        args_summary: pending.args_summary.clone(),
    });
    let started = Instant::now();

    let observation = if approved {
        // Disjoint borrows: `tool` from `state.tools`, invoke via `state.runtime`.
        let raw = RawToolCall {
            call_id: pending.call_id.clone(),
            name: pending.name.clone(),
            args: pending.args.clone(),
        };
        let mut inv = build_tool_invocation(&raw, config, cancel.clone(), &emit);
        inv.cwd = None;
        let opts = InvokeOptions {
            skip_confirmation: true,
            confirms_used,
        };
        match state.runtime.invoke(tool, inv, opts).await {
            InvokeResult::Observation(s) => s,
            InvokeResult::Denied { reason } => format!("Error: {reason}"),
            InvokeResult::NeedsConfirmation { .. } => {
                "Error: unexpected confirmation after grant".to_string()
            }
        }
    } else {
        state.runtime.audit_rejection(
            &pending,
            config.session_id.as_deref(),
            config.turn_id.as_deref(),
        );
        "Error: user declined this action".to_string()
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    emit(AgentEvent::ToolExecutionEnd {
        call_id: pending.call_id.clone(),
        tool_name: pending.name.clone(),
        ok: approved && observation_looks_ok(&observation),
        duration_ms,
    });

    tools_used.push(pending.name.clone());
    state.context.push(
        Role::Tool,
        tool_observation_json(&pending.call_id, observation),
    );

    match process_tool_calls(
        &mut state,
        remaining,
        &mut tools_used,
        tool_rounds,
        &mut confirms_used,
        &user_text,
        config,
        &emit,
        cancel.clone(),
    )
    .await?
    {
        ToolBatchResult::Continue => {}
        ToolBatchResult::Paused {
            outcome,
            pending_turn,
        } => {
            emit(AgentEvent::NeedsConfirmation {
                pending: pending_turn.pending.clone(),
            });
            emit(AgentEvent::AgentEnd {
                outcome: outcome.clone(),
            });
            return Ok(LoopResult {
                outcome,
                tool_rounds,
                tools_used,
                pending_turn: Some(pending_turn),
            });
        }
    }

    agent_loop(
        state,
        &user_text,
        config,
        tools_used,
        tool_rounds,
        confirms_used,
        cancel,
        Some(emit),
        None,
        0, // no finish-gate after HITL resume
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::json;
    use std::sync::Mutex;

    struct ScriptedClient {
        responses: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait]
    impl LlmClient for ScriptedClient {
        async fn complete(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
        ) -> Result<serde_json::Value, LlmError> {
            let mut guard = self.responses.lock().unwrap();
            if guard.is_empty() {
                return Err(LlmError::new("no more scripted responses"));
            }
            Ok(guard.remove(0))
        }

        fn model(&self) -> &str {
            "test"
        }
    }

    #[tokio::test]
    async fn loop_speaks_without_tools() {
        let client = ScriptedClient {
            responses: Mutex::new(vec![json!({
                "role": "assistant",
                "content": "Hello there."
            })]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "hi");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![];
        let config = AgentLoopConfig::default();

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(state, "hi", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Hello there."),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(result.tool_rounds, 0);
        assert!(result.pending_turn.is_none());
    }

    #[tokio::test]
    async fn loop_runs_safe_tool_then_speaks() {
        use crate::tool::{ToolError, ToolMeta};

        struct Echo;
        #[async_trait]
        impl Tool for Echo {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "echo"
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
                Ok("pong".into())
            }
        }

        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": { "name": "echo", "arguments": "{}" }
                    }]
                }),
                json!({
                    "role": "assistant",
                    "content": "Done."
                }),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "ping");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(Echo)];
        let config = AgentLoopConfig::default();

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(state, "ping", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();
        assert_eq!(result.tools_used, vec!["echo".to_string()]);
        assert_eq!(result.tool_rounds, 1);
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Done."),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn loop_runs_parallel_safe_tools_then_speaks() {
        use crate::tool::{ToolError, ToolMeta};

        struct Alpha;
        #[async_trait]
        impl Tool for Alpha {
            fn name(&self) -> &str {
                "alpha"
            }
            fn description(&self) -> &str {
                "alpha"
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
                Ok("A".into())
            }
        }

        struct Beta;
        #[async_trait]
        impl Tool for Beta {
            fn name(&self) -> &str {
                "beta"
            }
            fn description(&self) -> &str {
                "beta"
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
                Ok("B".into())
            }
        }

        let client = ScriptedClient {
            responses: Mutex::new(vec![
                json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "c1",
                            "type": "function",
                            "function": { "name": "alpha", "arguments": "{}" }
                        },
                        {
                            "id": "c2",
                            "type": "function",
                            "function": { "name": "beta", "arguments": "{}" }
                        }
                    ]
                }),
                json!({
                    "role": "assistant",
                    "content": "Both done."
                }),
            ]),
        };
        let mut context = Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "run both");
        let runtime = ToolRuntime::null();
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(Alpha), std::sync::Arc::new(Beta)];
        let config = AgentLoopConfig::default();

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let result = agent_loop(state, "run both", &config, vec![], 0, 0, None, None, None, 0)
            .await
            .unwrap();

        assert_eq!(
            result.tools_used,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(result.tool_rounds, 1);
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Both done."),
            other => panic!("unexpected {other:?}"),
        }

        // Both tool observations present, in original batch order.
        let tool_msgs: Vec<_> = context
            .messages()
            .iter()
            .filter(|m| matches!(m.role, Role::Tool))
            .collect();
        assert_eq!(tool_msgs.len(), 2);
        assert_eq!(tool_msgs[0].content["tool_call_id"], "c1");
        assert_eq!(tool_msgs[0].content["content"], "A");
        assert_eq!(tool_msgs[1].content["tool_call_id"], "c2");
        assert_eq!(tool_msgs[1].content["content"], "B");
    }
}
