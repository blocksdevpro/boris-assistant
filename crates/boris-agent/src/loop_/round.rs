//! Per-round LLM completion, tool-call gating, and finish-gate checks.

use std::time::{Duration, Instant};

use boris_ai::LlmStreamEvent;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::{ContextBudget, Role};
use crate::error::AgentError;
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn, TokenAccounting};

use super::message_parse::extract_reply_text;
use super::{listed_tools_json, LoopState};

/// How often to push a reasoning preview to the UI during one complete.
const REASONING_EMIT_EVERY: Duration = Duration::from_millis(80);
/// Keep the IPC snapshot small — UI shows the tail of the current thought.
const REASONING_PREVIEW_CHARS: usize = 900;

/// Injected when the next round will be the hard tool-round cap (tools still available this round).
pub(super) const NUDGE_NEAR_TOOL_CAP: &str = "\
<system-reminder>\n\
Tool budget is nearly exhausted. Stop calling tools. \
Give a short spoken status of what you finished and what \
(if anything) is left. 1–2 sentences only.\n\
</system-reminder>";

/// Injected at cap when the model returned tool_calls/empty content and must speak.
pub(super) const NUDGE_SPEAK_AT_CAP: &str = "\
<system-reminder>\n\
Reply now with a short spoken status (no tools). What got done?\n\
</system-reminder>";

pub(super) fn cancelled(cancel: &Option<CancellationToken>) -> bool {
    cancel.as_ref().is_some_and(|ct| ct.is_cancelled())
}

/// Tail of accumulated reasoning for the status snapshot.
pub(super) fn reasoning_preview(acc: &str) -> String {
    let count = acc.chars().count();
    if count <= REASONING_PREVIEW_CHARS {
        return acc.to_string();
    }
    acc.chars().skip(count - REASONING_PREVIEW_CHARS).collect()
}

/// One LLM completion for this round (tools withheld at cap).
///
/// Tool-schema pruning (see `helpers::tools_json_for_llm`) is silent on the
/// wire: when `pruned > 0`, `tool_search` is already retained as the
/// last-resort hatch and a `warn!` records the count. The model can recover
/// via `tool_search`.
pub(super) async fn complete_round(
    state: &mut LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    at_cap: bool,
    emit: &EmitFn,
    cancel: &Option<CancellationToken>,
    accounting: &mut TokenAccounting,
) -> Result<Value, AgentError> {
    let mut tools_json = if at_cap {
        Value::Null
    } else {
        let (payload, pruned) = listed_tools_json(state.tools, config, state.activated);
        if pruned > 0 {
            tracing::warn!(
                pruned,
                tools = payload.as_array().map(|a| a.len()).unwrap_or(0),
                "tool schemas pruned to budget; tool_search retained for discovery"
            );
        }
        payload
    };
    if state.runtime.artifact_delivery_exhausted() {
        if let Some(tools) = tools_json.as_array_mut() {
            tools.retain(|tool| tool["function"]["name"] != "present_artifact");
        }
    }
    let listed_names: Vec<String> = tools_json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str().map(ToOwned::to_owned))
        .collect();
    let listing_changed = state.activated.and_then(|set| {
        set.lock()
            .ok()
            .map(|mut table| table.record_listed(listed_names.iter().cloned()))
    });
    if let Some(debug) = &config.debug {
        debug.record(config.turn_id.as_deref(), "tool_listing", json!({
            "registered": state.tools.len(), "listed": listed_names.len(), "names": listed_names,
            "schema_tokens_est": if tools_json.is_null() { 0 } else { crate::context::estimate_serialized_tokens(&tools_json.to_string()) },
            "availability_changed": listing_changed,
            "presentation_disabled": state.runtime.artifact_delivery_exhausted(),
        }));
    }
    let task = config
        .task
        .unwrap_or_else(|| crate::task::classify_task(user_text));
    let mut round = crate::routing::round_traits_for_task(&state.context.as_json(), task);
    round.has_error_evidence = state.context.current_turn_has_tool_error();
    let stage = crate::routing::request_stage_for(task, round);
    let mut opts = boris_ai::CompleteOptions::for_stage(stage);
    opts.tool_error = Some(round.has_error_evidence);
    let context_limit = state
        .client
        .context_window_tokens()
        .unwrap_or(boris_ai::DEFAULT_CONTEXT_WINDOW_TOKENS);
    let output_reserve = opts.max_tokens.unwrap_or(boris_ai::DEFAULT_MAX_TOKENS);
    let budget = ContextBudget::for_request(context_limit, output_reserve);
    // Tool definitions are part of the provider request and participate in
    // the same model-aware budget as message content.
    state
        .context
        .compact_mechanical_for_request_with_budget(&tools_json, budget);
    let request_chars = state.context.estimate_request_chars(&tools_json);
    let request_tokens_est = state.context.estimate_request_tokens(&tools_json);
    accounting.record_request_estimate(request_tokens_est, context_limit);
    tracing::debug!(
        request_chars,
        request_tokens_est,
        context_limit,
        output_reserve,
        input_hard_limit = budget.hard_input_tokens,
        tools = tools_json.as_array().map(|a| a.len()).unwrap_or(0),
        "llm request token accounting"
    );
    if request_tokens_est > budget.hard_input_tokens {
        return Err(AgentError::input_too_large(
            request_tokens_est,
            budget.hard_input_tokens,
        ));
    }
    let messages = state.context.as_json();
    let request_seq = config.debug.as_ref().and_then(|debug| {
        debug.record_request(
            config.turn_id.as_deref(),
            &format!("{stage:?}"),
            state.context,
            &tools_json,
            request_tokens_est,
            context_limit,
            output_reserve,
        )
    });
    let started = Instant::now();
    let debug = config.debug.clone();
    let turn_id = config.turn_id.clone();
    let mut reported_usage = None;
    let emit = emit.clone();
    let mut acc = String::new();
    let mut last_emit = Instant::now();
    let mut dirty = false;
    let mut on_event = |ev: LlmStreamEvent| {
        let text = match ev {
            LlmStreamEvent::Usage(usage) => {
                accounting.record_provider_usage(&usage);
                reported_usage = Some(usage);
                return;
            }
            LlmStreamEvent::ModelSend { model } => {
                if let Some(debug) = &debug {
                    debug.record(
                        turn_id.as_deref(),
                        "model_send",
                        json!({
                            "request_seq": request_seq,
                            "model": model,
                        }),
                    );
                }
                return;
            }
            LlmStreamEvent::TransportAttempt { mode } => {
                if let Some(debug) = &debug {
                    debug.record(
                        turn_id.as_deref(),
                        "transport_attempt",
                        json!({
                            "request_seq": request_seq,
                            "mode": mode,
                        }),
                    );
                }
                return;
            }
            LlmStreamEvent::FirstDelta { ttfb_ms } => {
                if let Some(debug) = &debug {
                    debug.record(
                        turn_id.as_deref(),
                        "first_delta",
                        json!({
                            "request_seq": request_seq,
                            "ttfb_ms": ttfb_ms,
                        }),
                    );
                }
                return;
            }
            LlmStreamEvent::ReasoningDelta { text } => text,
            _ => return,
        };
        if text.is_empty() {
            return;
        }
        acc.push_str(&text);
        dirty = true;
        let first = acc.len() == text.len();
        if first || last_emit.elapsed() >= REASONING_EMIT_EVERY {
            emit(AgentEvent::Reasoning {
                preview: reasoning_preview(&acc),
            });
            last_emit = Instant::now();
            dirty = false;
        }
    };
    let stream = state
        .client
        .complete_stream(messages, tools_json, opts, &mut on_event);
    let result = if let Some(ct) = cancel.as_ref() {
        tokio::select! {
            biased;
            _ = ct.cancelled() => {
                Err(AgentError::cancelled("llm cancelled"))
            }
            msg = stream => msg.map_err(AgentError::from),
        }
    } else {
        stream.await.map_err(AgentError::from)
    };
    drop(on_event);
    if let Some(debug) = &config.debug {
        match &result {
            Ok(message) => {
                debug.record(
                    config.turn_id.as_deref(),
                    "response",
                    json!({
                        "request_seq": request_seq,
                        "duration_ms": started.elapsed().as_millis() as u64,
                        "message": message,
                        "usage": reported_usage.as_ref().map(|usage| json!({
                            "prompt_tokens": usage.prompt_tokens,
                            "completion_tokens": usage.completion_tokens,
                            "total_tokens": usage.total_tokens,
                            "cached_tokens": usage.cached_tokens,
                            "cache_write_tokens": usage.cache_write_tokens,
                            "reasoning_tokens": usage.reasoning,
                        })),
                    }),
                );
            }
            Err(error) => {
                debug.record(
                    config.turn_id.as_deref(),
                    "request_error",
                    json!({
                        "request_seq": request_seq,
                        "duration_ms": started.elapsed().as_millis() as u64,
                        "message": error.to_string(),
                    }),
                );
            }
        }
    }
    let msg = result?;
    if dirty {
        emit(AgentEvent::Reasoning {
            preview: reasoning_preview(&acc),
        });
    }
    Ok(msg)
}

/// Non-empty tool_calls array when tools are allowed this round.
pub(super) fn tool_calls_if_runnable(response: &Value, at_cap: bool) -> Option<&Vec<Value>> {
    if at_cap {
        return None;
    }
    let calls = response.get("tool_calls")?.as_array()?;
    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

/// At cap with empty content: force one more speak attempt (no tools).
///
/// This is the single extra completion beyond the `0..=max_tool_rounds` loop
/// (see `agent_loop` docs for the full accounting: up to `max+1` completions
/// in-loop plus at most one forced-speak here). It never issues tools and
/// never recurses — exactly one additional `complete_round` when the cap
/// round came back empty.
#[allow(clippy::too_many_arguments)]
pub(super) async fn ensure_spoken_reply_at_cap(
    state: &mut LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    at_cap: bool,
    mut reply: String,
    emit: &EmitFn,
    cancel: &Option<CancellationToken>,
    accounting: &mut TokenAccounting,
) -> Result<String, AgentError> {
    if reply.is_empty() && at_cap {
        state.context.push_control(json!(NUDGE_SPEAK_AT_CAP));
        let forced =
            complete_round(state, user_text, config, true, emit, cancel, accounting).await?;
        reply = extract_reply_text(&forced);
        if !reply.is_empty() {
            state.context.push(Role::Assistant, reply.clone());
        }
    } else if !reply.is_empty() {
        // Never push empty assistant content — OpenRouter returns
        // `messages.N.content: Invalid input` for empty/null content.
        state.context.push(Role::Assistant, reply.clone());
    }
    Ok(reply)
}

pub(super) fn should_reenter_finish_gate(
    at_cap: bool,
    finish_gate_left: u32,
    reply: &str,
    tools_used: &[String],
    useful_research_results: u32,
    todos_file: &std::path::Path,
    user_text: &str,
) -> bool {
    if at_cap || finish_gate_left == 0 || reply.is_empty() {
        return false;
    }
    // Research gate may fire with zero tools (freestyle LinkedIn/find-person).
    if crate::finish_gate::should_research_gate_with(
        user_text,
        reply,
        tools_used,
        useful_research_results,
    ) {
        return true;
    }
    if crate::finish_gate::should_local_work_gate(user_text, tools_used) {
        return true;
    }
    // Open todos only re-enter when this turn already used tools (avoid stale
    // todos forcing silence on casual replies).
    !tools_used.is_empty() && crate::finish_gate::pending_todo_count(todos_file) > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingClient {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmClient for CountingClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(json!({"role": "assistant", "content": "unexpected"}))
        }

        fn context_window_tokens(&self) -> Option<u32> {
            Some(4_096)
        }
    }

    #[test]
    fn reasoning_preview_keeps_short_text() {
        assert_eq!(reasoning_preview("hello"), "hello");
    }

    #[test]
    fn reasoning_preview_keeps_tail() {
        let acc = "α".repeat(REASONING_PREVIEW_CHARS + 40);
        let preview = reasoning_preview(&acc);
        assert_eq!(preview.chars().count(), REASONING_PREVIEW_CHARS);
        assert!(preview.chars().all(|c| c == 'α'));
    }

    #[tokio::test]
    async fn over_hard_budget_request_is_rejected_before_provider_call() {
        let client = CountingClient {
            calls: AtomicUsize::new(0),
        };
        let runtime = crate::runtime::ToolRuntime::null();
        let tools: Vec<Arc<dyn crate::Tool>> = Vec::new();
        let mut context = crate::Context::new(20);
        context.push(Role::System, "sys");
        context.push(Role::User, "x".repeat(20_000));
        let mut state = LoopState {
            context: &mut context,
            tools: &tools,
            runtime: &runtime,
            client: &client,
            activated: None,
        };
        let mut accounting = TokenAccounting::default();
        let emit: EmitFn = Arc::new(|_| {});

        let error = complete_round(
            &mut state,
            "large input",
            &AgentLoopConfig::default(),
            true,
            &emit,
            &None,
            &mut accounting,
        )
        .await
        .expect_err("oversized requests must fail locally");

        let crate::AgentErrorKind::InputTooLarge {
            estimated_tokens,
            allowed_tokens,
        } = error.kind()
        else {
            panic!("expected typed input-too-large error");
        };
        assert_eq!(
            estimated_tokens,
            accounting.peak_estimated_request_tokens as usize
        );
        assert!(estimated_tokens > allowed_tokens);
        assert!(error.to_string().contains("split the request"));
        assert_eq!(client.calls.load(Ordering::Relaxed), 0);
    }
}
