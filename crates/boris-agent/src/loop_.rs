//! Pure ReAct agent loop (tau-inspired).
//!
//! No personal memory, session I/O, or host side effects — only LLM complete +
//! [`ToolRuntime`] mediation. The [`crate::Agent`] facade owns state and learning.
//!
//! Tool batches that may need HITL run sequentially so remaining sibling calls
//! can be paused. Auto-allow batches (safe/moderate) still run sequentially but
//! never pause mid-batch.

use std::sync::Arc;
use std::time::Instant;

use boris_ai::LlmClient;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::{Context, Role};
use crate::error::AgentError;
use crate::outcome::AgentOutcome;
use crate::runtime::{
    args_summary, InvokeOptions, InvokeResult, PendingTurn, RawToolCall, ToolInvocation,
    ToolRuntime,
};
use crate::tool::Tool;
use crate::types::{AgentEvent, AgentLoopConfig, LoopResult};

/// Emit helper — no-op when no listeners are wired.
pub type EmitFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

fn noop_emit() -> EmitFn {
    Arc::new(|_| {})
}

/// Mutable state the loop may write back (context + tools + runtime).
pub struct LoopState<'a> {
    pub context: &'a mut Context,
    pub tools: &'a [Box<dyn Tool>],
    pub runtime: &'a ToolRuntime,
    pub client: &'a dyn LlmClient,
}

/// Run the ReAct loop until a final reply, HITL pause, cancel, or error.
///
/// `user_text` is only used for pending-turn bookkeeping (post-turn learn).
/// Context must already contain the user message (and any prior history).
pub async fn agent_loop(
    state: LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    tools_used: Vec<String>,
    tool_rounds: u32,
    confirms_used: u32,
    cancel: Option<CancellationToken>,
    emit: Option<EmitFn>,
) -> Result<LoopResult, AgentError> {
    let emit = emit.unwrap_or_else(noop_emit);
    emit(AgentEvent::AgentStart);

    let mut tools_used = tools_used;
    let mut tool_rounds = tool_rounds;
    let mut confirms_used = confirms_used;
    let max_rounds = config.max_tool_rounds as usize;

    for round in 0..=max_rounds {
        if let Some(ref ct) = cancel {
            if ct.is_cancelled() {
                emit(AgentEvent::Error {
                    message: "cancelled".into(),
                });
                return Err(AgentError::cancelled("agent loop cancelled"));
            }
        }

        emit(AgentEvent::TurnStart {
            round: round as u32,
        });

        // Mechanical compaction before each LLM call.
        state.context.compact_mechanical();

        let tools_json = tools_json(state.tools);
        let response = state
            .client
            .complete(state.context.as_json(), tools_json)
            .await?;

        let tool_calls = &response["tool_calls"];
        if let Some(calls) = tool_calls.as_array() {
            if !calls.is_empty() {
                if round == max_rounds {
                    return Err(AgentError::tool_loop(format!(
                        "tool loop exceeded {max_rounds} rounds without a final reply"
                    )));
                }

                tool_rounds += 1;
                state.context.push(Role::Assistant, response.clone());
                emit(AgentEvent::MessageEnd {
                    role: Role::Assistant,
                    preview: format!("{} tool call(s)", calls.len()),
                });

                let raw_calls: Vec<RawToolCall> = calls
                    .iter()
                    .map(|call| {
                        let call_id = call["id"].as_str().unwrap_or("").to_string();
                        let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                        let args: Value = serde_json::from_str(
                            call["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));
                        RawToolCall {
                            call_id,
                            name,
                            args,
                        }
                    })
                    .collect();

                match process_tool_calls(
                    &mut LoopState {
                        context: state.context,
                        tools: state.tools,
                        runtime: state.runtime,
                        client: state.client,
                    },
                    raw_calls,
                    &mut tools_used,
                    tool_rounds,
                    &mut confirms_used,
                    user_text,
                    config,
                    &emit,
                )
                .await?
                {
                    ToolBatchResult::Continue => {
                        emit(AgentEvent::TurnEnd {
                            round: round as u32,
                        });
                        continue;
                    }
                    ToolBatchResult::Paused {
                        outcome,
                        pending_turn,
                    } => {
                        emit(AgentEvent::NeedsConfirmation {
                            pending: pending_turn.pending.clone(),
                        });
                        emit(AgentEvent::TurnEnd {
                            round: round as u32,
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
            }
        }

        let reply = response["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        state.context.push(Role::Assistant, reply.clone());
        emit(AgentEvent::MessageEnd {
            role: Role::Assistant,
            preview: log_preview(&reply, 80),
        });
        emit(AgentEvent::TurnEnd {
            round: round as u32,
        });

        let outcome = if reply.is_empty() {
            AgentOutcome::Silent
        } else {
            AgentOutcome::speak(reply)
        };
        emit(AgentEvent::AgentEnd {
            outcome: outcome.clone(),
        });
        return Ok(LoopResult {
            outcome,
            tool_rounds,
            tools_used,
            pending_turn: None,
        });
    }

    Err(AgentError::tool_loop("tool loop exhausted"))
}

/// Process a batch of tool calls sequentially (HITL-safe).
async fn process_tool_calls(
    state: &mut LoopState<'_>,
    calls: Vec<RawToolCall>,
    tools_used: &mut Vec<String>,
    tool_rounds: u32,
    confirms_used: &mut u32,
    user_text: &str,
    config: &AgentLoopConfig,
    emit: &EmitFn,
) -> Result<ToolBatchResult, AgentError> {
    let mut iter = calls.into_iter();
    while let Some(call) = iter.next() {
        let tool = find_tool(state.tools, &call.name)?;
        let inv = ToolInvocation {
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            args: call.args.clone(),
            session_id: config.session_id.clone(),
            turn_id: config.turn_id.clone(),
        };
        let opts = InvokeOptions {
            skip_confirmation: false,
            confirms_used: *confirms_used,
        };

        let summary = args_summary(&call.name, &call.args);
        emit(AgentEvent::ToolExecutionStart {
            call_id: call.call_id.clone(),
            tool_name: call.name.clone(),
            args_summary: summary,
        });
        let started = Instant::now();

        match state.runtime.invoke(tool, inv, opts).await {
            InvokeResult::Observation(result) => {
                let duration_ms = started.elapsed().as_millis() as u64;
                emit(AgentEvent::ToolExecutionEnd {
                    call_id: call.call_id.clone(),
                    tool_name: call.name.clone(),
                    ok: !result.starts_with("Error:"),
                    duration_ms,
                });
                tools_used.push(call.name.clone());
                state.context.push(
                    Role::Tool,
                    json!({ "tool_call_id": call.call_id, "content": result }),
                );
            }
            InvokeResult::Denied { reason } => {
                let duration_ms = started.elapsed().as_millis() as u64;
                emit(AgentEvent::ToolExecutionEnd {
                    call_id: call.call_id.clone(),
                    tool_name: call.name.clone(),
                    ok: false,
                    duration_ms,
                });
                tools_used.push(call.name.clone());
                state.context.push(
                    Role::Tool,
                    json!({
                        "tool_call_id": call.call_id,
                        "content": format!("Error: {reason}")
                    }),
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

fn find_tool<'a>(tools: &'a [Box<dyn Tool>], name: &str) -> Result<&'a dyn Tool, AgentError> {
    tools
        .iter()
        .find(|t| t.name() == name)
        .map(|t| t.as_ref())
        .ok_or_else(|| AgentError::unknown_tool(format!("unknown tool requested by model: {name}")))
}

fn tools_json(tools: &[Box<dyn Tool>]) -> Value {
    if tools.is_empty() {
        return Value::Null;
    }
    let list: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name":        t.name(),
                    "description": t.description(),
                    "parameters":  t.parameters(),
                }
            })
        })
        .collect();
    json!(list)
}

fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

enum ToolBatchResult {
    Continue,
    Paused {
        outcome: AgentOutcome,
        pending_turn: PendingTurn,
    },
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
        let inv = ToolInvocation {
            call_id: pending.call_id.clone(),
            name: pending.name.clone(),
            args: pending.args.clone(),
            session_id: config.session_id.clone(),
            turn_id: config.turn_id.clone(),
        };
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
        ok: approved && !observation.starts_with("Error:"),
        duration_ms,
    });

    tools_used.push(pending.name.clone());
    state.context.push(
        Role::Tool,
        json!({ "tool_call_id": pending.call_id, "content": observation }),
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
        responses: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl LlmClient for ScriptedClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
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
        let tools: Vec<Box<dyn Tool>> = vec![];
        let config = AgentLoopConfig::default();

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
        };
        let result = agent_loop(state, "hi", &config, vec![], 0, 0, None, None)
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
            fn parameters(&self) -> Value {
                json!({"type":"object","properties":{},"required":[]})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::safe_default()
            }
            async fn execute(&self, _args: Value) -> Result<String, ToolError> {
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
        let tools: Vec<Box<dyn Tool>> = vec![Box::new(Echo)];
        let config = AgentLoopConfig::default();

        let state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
        };
        let result = agent_loop(state, "ping", &config, vec![], 0, 0, None, None)
            .await
            .unwrap();
        assert_eq!(result.tools_used, vec!["echo".to_string()]);
        assert_eq!(result.tool_rounds, 1);
        match result.outcome {
            AgentOutcome::Speak { text, .. } => assert_eq!(text, "Done."),
            other => panic!("unexpected {other:?}"),
        }
    }
}
