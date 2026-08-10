//! Shared loop plumbing: tool lookup, listing, invocation setup, observation writes.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::{Context, Role};
use crate::error::AgentError;
use crate::runtime::{
    args_summary, filter_listed_tools, ActivationSet, EventProgressSink, InvokeResult,
    ListToolsContext, RawToolCall, ToolInvocation,
};
use crate::tool::Tool;
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn};

/// Resolve a tool by name from the session tool table.
pub(super) fn find_tool<'a>(
    tools: &'a [Arc<dyn Tool>],
    name: &str,
) -> Result<&'a dyn Tool, AgentError> {
    tools
        .iter()
        .find(|t| t.name() == name)
        .map(|t| t.as_ref())
        .ok_or_else(|| {
            AgentError::unknown_tool(format!("unknown tool requested by model: {name}"))
        })
}

/// Soft lookup — `None` when the model named a tool that is not registered
/// (e.g. history / prompt drift vs capability preset). Callers should commit an
/// error observation instead of aborting the whole turn.
pub(super) fn find_tool_opt<'a>(
    tools: &'a [Arc<dyn Tool>],
    name: &str,
) -> Option<&'a dyn Tool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}

/// Observation text when the model calls a tool that is not in the session table.
pub(super) fn unknown_tool_observation(name: &str) -> String {
    format!(
        "Error: tool `{name}` is not available in this session. \
         Continue with other registered tools or finish without it."
    )
}

/// Build listing context (progressive disclosure / force-list-all).
pub(super) fn build_list_ctx(
    config: &AgentLoopConfig,
    activated: Option<&ActivationSet>,
) -> ListToolsContext {
    let mut features = config.features.clone();
    if config.force_list_all {
        features.force_list_all = true;
    }
    let activated_snap: HashSet<String> = activated
        .and_then(|a| a.lock().ok().map(|g| g.clone()))
        .unwrap_or_default();
    ListToolsContext {
        session_id: config.session_id.clone(),
        turn_id: config.turn_id.clone(),
        activated: Arc::new(activated_snap),
        features,
    }
}

/// OpenAI-style tools array for the LLM, or `null` when nothing is listable.
pub(super) fn tools_json_for_llm(tools: &[Arc<dyn Tool>], list_ctx: &ListToolsContext) -> Value {
    let listed = filter_listed_tools(tools, list_ctx);
    if listed.is_empty() {
        return Value::Null;
    }
    let list: Vec<Value> = listed
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

/// Build a [`ToolInvocation`] from a raw model call + loop config.
pub(super) fn build_tool_invocation(
    call: &RawToolCall,
    config: &AgentLoopConfig,
    cancel: Option<CancellationToken>,
    emit: &EmitFn,
) -> ToolInvocation {
    let mut inv = ToolInvocation::new(call.call_id.clone(), call.name.clone(), call.args.clone());
    inv.session_id = config.session_id.clone();
    inv.turn_id = config.turn_id.clone();
    inv.cancel = cancel;
    // Stamp process cwd so tools can resolve relative paths without closing over host state.
    inv.cwd = std::env::current_dir().ok();
    if config.features.progress_events {
        inv.progress = Some(
            EventProgressSink::new(Arc::clone(emit), call.call_id.clone(), call.name.clone())
                .into_arc(),
        );
    }
    inv
}

/// Context message content for a tool observation.
pub(super) fn tool_observation_json(call_id: &str, content: impl Into<String>) -> Value {
    json!({ "tool_call_id": call_id, "content": content.into() })
}

/// Observation text is treated as failure when it starts with the conventional prefix.
pub(super) fn observation_looks_ok(content: &str) -> bool {
    !content.starts_with("Error:")
}

/// Emit `ToolExecutionStart` for a raw call (uses standard args summary).
pub(super) fn emit_tool_start(emit: &EmitFn, call: &RawToolCall) {
    emit(AgentEvent::ToolExecutionStart {
        call_id: call.call_id.clone(),
        tool_name: call.name.clone(),
        args_summary: args_summary(&call.name, &call.args),
    });
}

/// Emit end event, record tool name, and push observation into context.
pub(super) fn commit_tool_observation(
    context: &mut Context,
    call: &RawToolCall,
    content: String,
    ok: bool,
    duration_ms: u64,
    tools_used: &mut Vec<String>,
    emit: &EmitFn,
) {
    emit(AgentEvent::ToolExecutionEnd {
        call_id: call.call_id.clone(),
        tool_name: call.name.clone(),
        ok,
        duration_ms,
    });
    tools_used.push(call.name.clone());
    context.push(Role::Tool, tool_observation_json(&call.call_id, content));
}

/// Map a non-HITL invoke result to observation text.
///
/// Returns `None` only for [`InvokeResult::NeedsConfirmation`] (caller decides
/// whether that is a pause or an unexpected parallel-batch error).
pub(super) fn observation_text_from_invoke(result: InvokeResult) -> Option<String> {
    match result {
        InvokeResult::Observation(s) => Some(s),
        InvokeResult::Denied { reason } => Some(format!("Error: {reason}")),
        InvokeResult::NeedsConfirmation { .. } => None,
    }
}

/// Convert parallel-batch invoke outcomes (confirmation is unexpected → error string).
pub(super) fn parallel_batch_observation(result: InvokeResult) -> (String, bool) {
    match observation_text_from_invoke(result) {
        Some(text) => {
            let ok = observation_looks_ok(&text);
            // Denied always reports ok=false even if formatting changes later.
            // observation_looks_ok already treats "Error:" prefix as fail.
            (text, ok)
        }
        None => (
            "Error: unexpected confirmation required in parallel batch".to_string(),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::InvokeResult;

    #[test]
    fn tool_observation_json_shape() {
        let v = tool_observation_json("c1", "hello");
        assert_eq!(v["tool_call_id"], "c1");
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn observation_looks_ok_checks_error_prefix() {
        assert!(observation_looks_ok("ok result"));
        assert!(!observation_looks_ok("Error: denied"));
    }

    #[test]
    fn observation_text_from_invoke_variants() {
        assert_eq!(
            observation_text_from_invoke(InvokeResult::Observation("x".into())),
            Some("x".into())
        );
        assert_eq!(
            observation_text_from_invoke(InvokeResult::Denied {
                reason: "nope".into()
            }),
            Some("Error: nope".into())
        );
        assert!(observation_text_from_invoke(InvokeResult::NeedsConfirmation {
            pending: crate::runtime::PendingToolCall::new(
                "id",
                "t",
                json!({}),
                "sum",
                crate::tool::ToolRisk::Safe,
                "c1",
            ),
            speak_prompt: "confirm?".into(),
        })
        .is_none());
    }

    #[test]
    fn parallel_batch_observation_unexpected_confirm() {
        let (text, ok) = parallel_batch_observation(InvokeResult::NeedsConfirmation {
            pending: crate::runtime::PendingToolCall::new(
                "id",
                "t",
                json!({}),
                "sum",
                crate::tool::ToolRisk::Safe,
                "c1",
            ),
            speak_prompt: "confirm?".into(),
        });
        assert!(!ok);
        assert!(text.contains("unexpected confirmation"));
    }
}
