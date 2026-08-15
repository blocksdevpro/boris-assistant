//! Per-round LLM completion, tool-call gating, and finish-gate checks.

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::Role;
use crate::error::AgentError;
use crate::types::AgentLoopConfig;

use super::message_parse::extract_reply_text;
use super::{listed_tools_json, LoopState};

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

/// One LLM completion for this round (tools withheld at cap).
pub(super) async fn complete_round(
    state: &mut LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    at_cap: bool,
) -> Result<Value, AgentError> {
    let tools_json = if at_cap {
        Value::Null
    } else {
        listed_tools_json(state.tools, config, state.activated)
    };
    // Tool definitions are part of the provider request and must participate
    // in the same soft/hard compaction thresholds as message content.
    state.context.compact_mechanical_for_request(&tools_json);
    let request_chars = state.context.estimate_request_chars(&tools_json);
    tracing::debug!(
        request_chars,
        request_tokens_est = request_chars / 4,
        tools = tools_json.as_array().map(|a| a.len()).unwrap_or(0),
        "llm request token accounting"
    );
    let task = config
        .task
        .unwrap_or_else(|| crate::task::classify_task(user_text));
    let messages = state.context.as_json();
    let round = crate::routing::round_traits_for_task(&messages, task);
    let stage = crate::routing::request_stage_for(task, round);
    let opts = boris_ai::CompleteOptions::for_stage(stage);
    state
        .client
        .complete_with_options(messages, tools_json, opts)
        .await
        .map_err(Into::into)
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
pub(super) async fn ensure_spoken_reply_at_cap(
    state: &mut LoopState<'_>,
    user_text: &str,
    config: &AgentLoopConfig,
    at_cap: bool,
    mut reply: String,
) -> Result<String, AgentError> {
    if reply.is_empty() && at_cap {
        state.context.push(Role::User, json!(NUDGE_SPEAK_AT_CAP));
        let forced = complete_round(state, user_text, config, true).await?;
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
    // Open todos only re-enter when this turn already used tools (avoid stale
    // todos forcing silence on casual replies).
    !tools_used.is_empty() && crate::finish_gate::pending_todo_count(todos_file) > 0
}
