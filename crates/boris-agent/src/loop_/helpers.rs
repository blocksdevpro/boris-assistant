//! Shared loop plumbing: tool lookup, listing, invocation setup, observation writes.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::{Context, Role};
use crate::error::AgentError;
use crate::runtime::{
    args_summary, filter_listed_tools, ActivationSet, EventProgressSink, InvokeResult,
    ListToolsContext, RawToolCall, ToolInvocation, MAX_TOOL_SCHEMA_CHARS,
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
pub(super) fn tools_json_for_llm(tools: &[Arc<dyn Tool>], list_ctx: &ListToolsContext) -> Value {
    let listed = filter_listed_tools(tools, list_ctx);
    if listed.is_empty() {
        return Value::Null;
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
        // Prefer removing the lowest-value non-core definition. Within an
        // equal priority tier, removing the largest schema recovers the most
        // budget while disturbing the fewest capabilities.
        let non_core = list
            .iter()
            .enumerate()
            .filter(|(_, (tool, _, _, _))| {
                !crate::runtime::is_core_name(tool.name(), &list_ctx.features)
                    && tool.name() != "tool_search"
            })
            .min_by(|(_, a), (_, b)| schema_removal_order(a, b))
            .map(|(index, _)| (index, false));
        // The cap is unconditional. If retained/core schemas alone exceed it,
        // remove the lowest-value core definition next, keeping tool_search as
        // the last discovery escape hatch whenever it fits.
        let core = list
            .iter()
            .enumerate()
            .filter(|(_, (tool, _, _, _))| tool.name() != "tool_search")
            .min_by(|(_, a), (_, b)| schema_removal_order(a, b))
            .map(|(index, _)| (index, true));
        let last_resort = list
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| schema_removal_order(a, b))
            .map(|(index, _)| (index, true));
        let Some((index, protected)) = non_core.or(core).or(last_resort) else {
            break;
        };
        pruned_core += usize::from(protected);
        list.remove(index);
    }

    if list.len() < listed_before {
        tracing::debug!(
            listed_before,
            listed_after = list.len(),
            pruned_core,
            schema_chars = serialized_definitions_len(&list),
            schema_budget_chars = MAX_TOOL_SCHEMA_CHARS,
            "pruned tool definitions to request schema budget"
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
        return Value::Null;
    }
    Value::Array(
        list.into_iter()
            .map(|(_, definition, _, _)| definition)
            .collect(),
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
    !(content.starts_with("Error:") || content.starts_with("Error ["))
}

/// Count useful web-search/fetch observations in the current user turn.
///
/// Tool names live on assistant `tool_calls` while results carry only call ids,
/// so join the two here instead of treating raw call counts as evidence. Old
/// turns and harness-injected reminder messages cannot satisfy the gate.
pub(super) fn useful_research_observation_count(context: &Context) -> u32 {
    let messages = context.messages();
    let turn_start = messages
        .iter()
        .rposition(|m| {
            matches!(m.role, Role::User)
                && m.content
                    .as_str()
                    .is_some_and(|s| !is_control_user_message(s))
        })
        .map_or(0, |i| i.saturating_add(1));

    let mut research_calls = HashSet::new();
    for message in &messages[turn_start..] {
        if !matches!(message.role, Role::Assistant) {
            continue;
        }
        let Some(calls) = message.content.get("tool_calls").and_then(Value::as_array) else {
            continue;
        };
        for call in calls {
            let name = call
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str);
            if !matches!(name, Some("web_search" | "web_fetch")) {
                continue;
            }
            if let Some(id) = call.get("id").and_then(Value::as_str) {
                research_calls.insert(id.to_string());
            }
        }
    }

    let mut counted = HashSet::new();
    messages[turn_start..]
        .iter()
        .filter(|m| matches!(m.role, Role::Tool))
        .filter_map(|m| {
            let id = m.content.get("tool_call_id")?.as_str()?;
            let content = m.content.get("content")?.as_str()?;
            (research_calls.contains(id)
                && counted.insert(id.to_string())
                && observation_has_useful_evidence(content))
            .then_some(())
        })
        .count() as u32
}

fn is_control_user_message(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<system-reminder>") || text.starts_with("<conversation_summary>")
}

fn observation_has_useful_evidence(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() || !observation_looks_ok(trimmed) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !lower.starts_with("no search results")
        && !lower.starts_with("no results")
        && !lower.starts_with("search returned empty")
        && !lower.starts_with("not found")
        && !lower.contains("search backends returned empty")
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
    log_tool_done(&call.name, ok, duration_ms);
    emit(AgentEvent::ToolExecutionEnd {
        call_id: call.call_id.clone(),
        tool_name: call.name.clone(),
        ok,
        duration_ms,
    });
    tools_used.push(call.name.clone());
    context.push(Role::Tool, tool_observation_json(&call.call_id, content));
}

/// Structured completion line so each tool call has a wall-clock in the log.
pub(super) fn log_tool_done(tool: &str, ok: bool, ms: u64) {
    tracing::info!(tool = %tool, ok, ms, "tool done");
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
    use crate::runtime::{InvokeResult, MAX_TOOL_SCHEMA_CHARS};
    use crate::tool::{ToolError, ToolMeta};
    use async_trait::async_trait;

    struct LargeDefinitionTool {
        name: String,
        description: String,
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
            Arc::new(LargeDefinitionTool {
                name: "get_time".into(),
                description: "core".into(),
            }),
            Arc::new(LargeDefinitionTool {
                name: "activated_tool".into(),
                description: "a".repeat(40_000),
            }),
            Arc::new(LargeDefinitionTool {
                name: "low_priority_tool".into(),
                description: "z".repeat(40_000),
            }),
        ];
        let mut activated = HashSet::new();
        activated.insert("activated_tool".to_string());
        let ctx = ListToolsContext {
            activated: Arc::new(activated),
            ..Default::default()
        };

        let payload = tools_json_for_llm(&tools, &ctx);
        let names = payload
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|definition| definition["function"]["name"].as_str())
            .collect::<Vec<_>>();

        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
        assert!(names.contains(&"get_time"), "core tool must be retained");
        assert!(
            names.contains(&"activated_tool"),
            "actual-use activation must outrank an unselected tool"
        );
        assert!(!names.contains(&"low_priority_tool"));
    }

    #[test]
    fn tool_payload_prunes_core_when_core_alone_exceeds_hard_budget() {
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(LargeDefinitionTool {
                name: "get_time".into(),
                description: "t".repeat(40_000),
            }),
            Arc::new(LargeDefinitionTool {
                name: "get_date".into(),
                description: "d".repeat(40_000),
            }),
        ];

        let payload = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
        assert_eq!(
            payload.as_array().map(Vec::len),
            Some(1),
            "retain as many core definitions as fit"
        );
    }

    #[test]
    fn individually_oversized_tool_search_is_omitted() {
        let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(LargeDefinitionTool {
            name: "tool_search".into(),
            description: "s".repeat(MAX_TOOL_SCHEMA_CHARS + 1),
        })];

        let payload = tools_json_for_llm(&tools, &ListToolsContext::default());
        assert!(payload.is_null());
        assert!(payload.to_string().len() <= MAX_TOOL_SCHEMA_CHARS);
    }

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
    fn research_evidence_counts_only_useful_current_turn_results() {
        let mut context = Context::new(20);
        context.push(Role::User, "old research");
        context.push(
            Role::Assistant,
            json!({"tool_calls":[{
                "id":"old", "function":{"name":"web_search","arguments":"{}"}
            }]}),
        );
        context.push(
            Role::Tool,
            tool_observation_json("old", "Search results for: old\n1. stale"),
        );

        context.push(Role::User, "research current topic");
        context.push(
            Role::Assistant,
            json!({"tool_calls":[
                {"id":"empty", "function":{"name":"web_search","arguments":"{}"}},
                {"id":"failed", "function":{"name":"web_fetch","arguments":"{}"}},
                {"id":"hit", "function":{"name":"web_search","arguments":"{}"}},
                {"id":"local", "function":{"name":"file_read","arguments":"{}"}}
            ]}),
        );
        context.push(
            Role::Tool,
            tool_observation_json("empty", "No search results for: current"),
        );
        context.push(
            Role::Tool,
            tool_observation_json("failed", "Error: fetch HTTP 500"),
        );
        context.push(
            Role::Tool,
            tool_observation_json("hit", "Search results for: current\n1. useful"),
        );
        context.push(
            Role::Tool,
            tool_observation_json("local", "local file content"),
        );
        context.push(
            Role::User,
            "<system-reminder>\nkeep researching\n</system-reminder>",
        );

        assert_eq!(useful_research_observation_count(&context), 1);
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
        assert!(
            observation_text_from_invoke(InvokeResult::NeedsConfirmation {
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
            .is_none()
        );
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
