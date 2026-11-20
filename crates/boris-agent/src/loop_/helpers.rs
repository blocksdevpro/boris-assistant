//! Shared loop plumbing: tool lookup, listing, invocation setup, observation writes.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::Context;
use crate::error::AgentError;
use crate::runtime::{
    args_summary, filter_listed_tools, ActivationSet, EventProgressSink, InvokeResult,
    ListToolsContext, RawToolCall, ToolInvocation, MAX_TOOL_SCHEMA_CHARS,
};
use crate::tool::{ObservationError, Tool, ToolObservation};
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
        .ok_or_else(|| AgentError::unknown_tool(format!("unknown tool requested by model: {name}")))
}

/// Soft lookup — `None` when the model named a tool that is not registered
/// (e.g. history / prompt drift vs capability preset). Callers should commit an
/// error observation instead of aborting the whole turn.
pub(super) fn find_tool_opt<'a>(tools: &'a [Arc<dyn Tool>], name: &str) -> Option<&'a dyn Tool> {
    tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
}

/// Observation text when the model calls a tool that is not in the session table.
pub(super) fn unknown_tool_observation(name: &str) -> String {
    format!(
        "Error: tool `{name}` is not available in this session. \
         Continue with other registered tools or finish without it."
    )
}

/// Build listing context (builtins + activated extensions / force-list-all).
pub(super) fn build_list_ctx(
    config: &AgentLoopConfig,
    activated: Option<&ActivationSet>,
) -> ListToolsContext {
    let mut features = config.features.clone();
    if config.force_list_all {
        features.force_list_all = true;
    }
    let activated_snap: HashSet<String> = activated
        .and_then(|a| a.lock().ok().map(|mut g| g.snapshot()))
        .unwrap_or_default();
    ListToolsContext {
        session_id: config.session_id.clone(),
        turn_id: config.turn_id.clone(),
        activated: Arc::new(activated_snap),
        features,
        task: config.task,
    }
}

/// OpenAI-style tools array for the LLM, or `null` when nothing is listable.
///
/// Returns `(payload, pruned_count)` where `pruned_count` is the number of
/// listed definitions dropped to fit `MAX_TOOL_SCHEMA_CHARS`. Callers that
/// build LLM requests (see `loop_::round::complete_round`) must surface a
/// non-zero count: `tool_search` is already retained as the last-resort
/// escape hatch, and a `warn!` with the count keeps the silent prune visible.
/// The pruning itself is silent on the wire — the model only sees the
/// retained definitions and can recover via `tool_search`.
pub(super) fn tools_json_for_llm(
    tools: &[Arc<dyn Tool>],
    list_ctx: &ListToolsContext,
) -> (Value, usize) {
    let listed = filter_listed_tools(tools, list_ctx);
    if listed.is_empty() {
        return (Value::Null, 0);
    }
    let mut list: Vec<(&Arc<dyn Tool>, Value, usize, i32)> = listed
        .iter()
        .map(|t| {
            let definition = json!({
                "type": "function",
                "function": {
                    "name":        t.name(),
                    "description": t.description(),
                    "parameters":  t.parameters(),
                }
            });
            let serialized_len = definition.to_string().len();
            let priority = crate::runtime::listing::schema_retention_priority(t.as_ref(), list_ctx);
            (*t, definition, serialized_len, priority)
        })
        .collect();

    let listed_before = list.len();
    let mut pruned_core = 0usize;
    while serialized_definitions_len(&list) > MAX_TOOL_SCHEMA_CHARS {
        // Remove the lowest-value definition. Within an
        // equal priority tier, removing the largest schema recovers the most
        // budget while disturbing the fewest capabilities.
        let candidate = list
            .iter()
            .enumerate()
            .filter(|(_, (tool, _, _, _))| tool.name() != "tool_search")
            .min_by(|(_, a), (_, b)| schema_removal_order(a, b))
            .map(|(index, (tool, _, _, _))| {
                (
                    index,
                    crate::runtime::is_core_name(tool.name(), &list_ctx.features),
                )
            });
        // Keep discovery until last, but enforce the cap even if it alone
        // exceeds the budget.
        let last_resort = list
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| schema_removal_order(a, b))
            .map(|(index, _)| (index, true));
        let Some((index, protected)) = candidate.or(last_resort) else {
            break;
        };
        pruned_core += usize::from(protected);
        list.remove(index);
    }

    let pruned = listed_before.saturating_sub(list.len());
    if pruned > 0 {
        tracing::debug!(
            listed_before,
            listed_after = list.len(),
            pruned,
            pruned_core,
            schema_chars = serialized_definitions_len(&list),
            schema_budget_chars = MAX_TOOL_SCHEMA_CHARS,
            "pruned tool definitions to request schema budget"
        );
        // Silent on the wire but visible in logs: the model can still recover
        // via `tool_search`, which the loop above retains as the last resort.
        tracing::warn!(
            pruned,
            pruned_core,
            listed_after = list.len(),
            schema_budget_chars = MAX_TOOL_SCHEMA_CHARS,
            "tool definitions pruned to schema budget; model may need tool_search to discover dropped tools"
        );
    }
    if pruned_core > 0 {
        tracing::warn!(
            pruned_core,
            schema_budget_chars = MAX_TOOL_SCHEMA_CHARS,
            "core tool definitions exceeded request schema budget"
        );
    }
    if list.is_empty() {
        return (Value::Null, pruned);
    }
    (
        Value::Array(
            list.into_iter()
                .map(|(_, definition, _, _)| definition)
                .collect(),
        ),
        pruned,
    )
}

fn schema_removal_order(
    a: &(&Arc<dyn Tool>, Value, usize, i32),
    b: &(&Arc<dyn Tool>, Value, usize, i32),
) -> std::cmp::Ordering {
    a.3.cmp(&b.3)
        // `min_by` should choose the larger definition on a priority tie.
        .then_with(|| b.2.cmp(&a.2))
}

fn serialized_definitions_len(definitions: &[(&Arc<dyn Tool>, Value, usize, i32)]) -> usize {
    // JSON array brackets + commas plus the already-built definition values.
    2usize.saturating_add(
        definitions
            .iter()
            .map(|(_, _, serialized_len, _)| serialized_len.saturating_add(1))
            .sum::<usize>()
            .saturating_sub(usize::from(!definitions.is_empty())),
    )
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
    if config.features.progress_events || call.name == "present_artifact" {
        inv.progress = Some(
            EventProgressSink::new(Arc::clone(emit), call.call_id.clone(), call.name.clone())
                .into_arc(),
        );
    }
    inv
}

/// Context message content for a tool observation.
///
/// Test-only helper for building `Role::Tool` message payloads; production
/// observations go through [`Context::push_tool_result`].
#[cfg(test)]
pub(super) fn tool_observation_json(call_id: &str, content: impl Into<String>) -> Value {
    json!({ "tool_call_id": call_id, "content": content.into() })
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
    emit_tool_end(emit, call, ok, duration_ms);
    tools_used.push(call.name.clone());
    push_tool_result_messages(context, &call.name, &call.call_id, content, ok);
}

/// Emit completion when a future finishes, independently of context ordering.
pub(super) fn emit_tool_end(emit: &EmitFn, call: &RawToolCall, ok: bool, duration_ms: u64) {
    log_tool_done(&call.name, ok, duration_ms);
    emit(AgentEvent::ToolExecutionEnd {
        call_id: call.call_id.clone(),
        tool_name: call.name.clone(),
        ok,
        duration_ms,
    });
}

/// Store an unmodified tool observation, followed by any host-authored nudge
/// as a separate control message with explicit provenance.
pub(super) fn push_tool_result_messages(
    context: &mut Context,
    tool_name: &str,
    call_id: &str,
    content: String,
    ok: bool,
) {
    context.push_tool_result(tool_name, call_id, content, ok);
}

/// Structured completion line so each tool call has a wall-clock in the log.
pub(super) fn log_tool_done(tool: &str, ok: bool, ms: u64) {
    tracing::info!(tool = %tool, ok, ms, "tool done");
}

/// Map a non-HITL invoke result to its trusted observation.
///
/// Returns `None` for confirmation or input pauses (caller decides
/// whether that is a pause or an unexpected parallel-batch error).
pub(super) fn observation_from_invoke(result: InvokeResult) -> Option<ToolObservation> {
    match result {
        InvokeResult::Observation(s) => Some(s),
        InvokeResult::Denied { reason } => Some(ToolObservation::err(
            ObservationError::new("denied", false, reason),
            0,
        )),
        InvokeResult::NeedsConfirmation { .. } | InvokeResult::NeedsInput { .. } => None,
    }
}

/// Convert parallel-batch invoke outcomes (confirmation is unexpected → error string).
pub(super) fn parallel_batch_observation(result: InvokeResult) -> (String, bool) {
    match observation_from_invoke(result) {
        Some(observation) => {
            let ok = observation.looks_ok();
            (observation.to_provider_text(), ok)
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
    use crate::runtime::{InvokeResult, MAX_TOOL_SCHEMA_CHARS};
    use crate::tool::{ToolError, ToolMeta};
    use async_trait::async_trait;

    struct LargeDefinitionTool {
        name: String,
        description: String,
        extension: bool,
    }

    fn large(name: &str, description: String) -> Arc<dyn Tool> {
        Arc::new(LargeDefinitionTool {
            name: name.into(),
            description,
            extension: false,
        })
    }

    fn large_extension(name: &str, description: String) -> Arc<dyn Tool> {
        Arc::new(LargeDefinitionTool {
            name: name.into(),
            description,
            extension: true,
        })
    }

    #[async_trait]
    impl Tool for LargeDefinitionTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{}})
        }

        fn meta(&self) -> ToolMeta {
            ToolMeta::safe_default()
        }

        fn is_extension(&self) -> bool {
            self.extension
        }

        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            Ok(String::new())
        }
    }

    #[test]
    fn tool_payload_prunes_low_priority_definitions_to_hard_budget() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            large("remember_note", "core".into()),
            large_extension("activated_tool", "a".repeat(40_000)),
            large("low_priority_tool", "z".repeat(40_000)),
        ];
        let mut activated = HashSet::new();
        activated.insert("activated_tool".to_string());
        let ctx = ListToolsContext {
            activated: Arc::new(activated),
            ..Default::default()
        };

        let (payload, pruned) = tools_json_for_llm(&tools, &ctx);
        let names = payload
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|definition| definition["function"]["name"].as_str())
            .collect::<Vec<_>>();

        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
        assert!(
            names.contains(&"remember_note"),
            "core tool must be retained"
        );
        assert!(
            names.contains(&"activated_tool"),
            "an activated extension must outrank a shared builtin"
        );
        assert!(!names.contains(&"low_priority_tool"));
        assert_eq!(
            pruned, 1,
            "one low-priority definition must be reported pruned"
        );
    }

    #[test]
    fn schema_budget_keeps_discoveries_above_unused_shared_tools() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            large("remember_note", "n".repeat(40_000)),
            large_extension("activated_tool", "a".repeat(40_000)),
            large("tool_search", "discovery".into()),
        ];
        let ctx = ListToolsContext {
            activated: Arc::new(HashSet::from(["activated_tool".into()])),
            ..Default::default()
        };
        let (payload, pruned) = tools_json_for_llm(&tools, &ctx);
        let names = payload
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|definition| definition["function"]["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["activated_tool", "tool_search"]);
        assert_eq!(pruned, 1);
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
    }

    #[test]
    fn tool_payload_prunes_core_when_core_alone_exceeds_hard_budget() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            large("remember_note", "t".repeat(40_000)),
            large("present_artifact", "d".repeat(40_000)),
        ];

        let (payload, pruned) = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
        assert_eq!(
            payload.as_array().map(Vec::len),
            Some(1),
            "retain as many core definitions as fit"
        );
        assert_eq!(pruned, 1, "core prune must report the dropped definition");
    }

    #[test]
    fn individually_oversized_tool_search_is_omitted() {
        // An unactivated extension keeps discovery listed but is not listed itself.
        let tools: Vec<Arc<dyn Tool>> = vec![
            large("tool_search", "s".repeat(MAX_TOOL_SCHEMA_CHARS + 1)),
            large_extension("plugin__hidden", "hidden".into()),
        ];

        let (payload, pruned) = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert!(payload.is_null());
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
        assert_eq!(pruned, 1, "oversized lone definition counts as pruned");
    }

    #[test]
    fn tool_observation_json_shape() {
        let v = tool_observation_json("c1", "hello");
        assert_eq!(v["tool_call_id"], "c1");
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn observation_from_invoke_variants() {
        assert_eq!(
            observation_from_invoke(InvokeResult::Observation(ToolObservation::ok_text("x", 0)))
                .map(|observation| observation.to_provider_text()),
            Some("x".into())
        );
        assert_eq!(
            observation_from_invoke(InvokeResult::Denied {
                reason: "nope".into()
            })
            .map(|observation| observation.to_provider_text()),
            Some("Error [denied]: nope".into())
        );
        assert!(observation_from_invoke(InvokeResult::NeedsConfirmation {
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
    fn parallel_batch_uses_status_even_when_success_text_looks_like_error() {
        let text = "Error: quoted file contents; 0 failed";
        let (rendered, ok) = parallel_batch_observation(InvokeResult::Observation(
            ToolObservation::ok_text(text, 0),
        ));
        assert!(ok);
        assert_eq!(rendered, text);

        let (rendered, ok) =
            parallel_batch_observation(InvokeResult::Observation(ToolObservation::err(
                ObservationError::new("failed", false, "connection closed"),
                0,
            )));
        assert!(!ok);
        assert!(rendered.contains("connection closed"));
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

    #[test]
    fn pruning_retains_tool_search_as_last_resort_and_reports_count() {
        // tool_search must survive pruning so the model can discover dropped
        // tools; the pruned count lets `complete_round` log a budget-aware warn.
        let mut tools: Vec<Arc<dyn Tool>> = vec![
            large("tool_search", "search".into()),
            large_extension("plugin__hidden", "hidden".into()),
        ];
        for i in 0..6 {
            tools.push(large(&format!("filler-{i}"), "x".repeat(20_000)));
        }
        let (payload, pruned) = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert!(pruned >= 1, "expected at least one prune, got {pruned}");
        let names = payload
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|d| d["function"]["name"].as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // tool_search is the last-resort discovery hatch: it is only dropped
        // when even it alone exceeds the budget (covered above).
        if !payload.is_null() {
            assert!(
                names.contains(&"tool_search"),
                "tool_search must be retained when anything fits, got {names:?}"
            );
        }
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
    }

    #[test]
    fn no_prune_reports_zero() {
        let tools: Vec<Arc<dyn Tool>> = vec![large("get_time", "core".into())];
        let (payload, pruned) = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert_eq!(pruned, 0);
        assert!(payload.is_array());
    }
}
