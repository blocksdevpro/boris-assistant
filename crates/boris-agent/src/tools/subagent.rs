//! Research subagent: thorough read-mostly tool loop and return a summary.
//!
//! Child tools are filtered with [`ToolMeta::is_read_only`](crate::tool::ToolMeta::is_read_only)
//! and `risk <= Moderate`. Production tools must set explicit `read_only(true)`
//! on their meta (kind-only heuristics only treat Read/Search as RO). After the
//! profile-tool meta fix, `get_user_context` / `recall_notes` / file reads are
//! eligible; writers (`save_user_fact`, `remember_note`, bash, …) are not.
//!
//! Progress: child loop events are mapped onto the parent tool's progress sink
//! so the host UI can show live sub-steps instead of a frozen "Subagent" chip.
//!
//! # On-disk layout (under parent session)
//!
//! ```text
//! {session_root}/subagents/{child_id}/
//!   meta.json
//!   tool_calls.jsonl
//!   summary.md
//! ```
//!
//! Clean break: no resume of incomplete children. Child audits go to the
//! child's `tool_calls.jsonl` via [`JsonlAuditSink`] (never [`NullAuditSink`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use boris_ai::LlmClient;
use serde_json::{json, Value};
use tracing::warn;

use crate::context::{Context, Role};
use crate::loop_::{agent_loop, LoopState};
use crate::runtime::{JsonlAuditSink, SandboxConfig, ToolRuntime, ToolRuntimeFeatures};
use crate::tool::{
    require_object, require_string, truncate_tool_result, Tool, ToolError, ToolKind, ToolMeta,
    ToolRisk,
};
use crate::tool_context::ToolCallContext;
use crate::types::{AgentEvent, AgentLoopConfig, EmitFn, DEFAULT_MAX_TOOL_ROUNDS};

/// Default tool-round budget for spawn_subagent when `max_rounds` is omitted.
const DEFAULT_SUBAGENT_ROUNDS: u32 = 8;
/// Hard upper clamp on `max_rounds` (also capped by [`DEFAULT_MAX_TOOL_ROUNDS`]).
const MAX_SUBAGENT_ROUNDS: u32 = 16;
/// Child conversation max user-turn window (room for multi-wave research).
const SUBAGENT_CONTEXT_MAX_TURNS: u32 = 20;
/// Wall-clock timeout for the whole subagent tool invocation.
const SUBAGENT_TIMEOUT_SECS: u64 = 180;

/// Shared client for subagent runs (same API key/route as parent).
pub type SharedLlm = Arc<dyn LlmClient>;

/// Read-only (or safe) tools the parent registers for subagents.
pub type SharedTools = Arc<Mutex<Vec<Arc<dyn Tool>>>>;

/// Parent session root handle: `sessions/desktop/{uuid}`. Shared so [`crate::Agent`]
/// can rebind after session create/resume without reconstructing the tool.
pub type SharedSessionRoot = Arc<Mutex<Option<PathBuf>>>;

pub struct SpawnSubagentTool {
    client: SharedLlm,
    /// Tools available to children (filtered to read-ish kinds at execute time).
    tools: SharedTools,
    sandbox: SandboxConfig,
    /// Parent session root: sessions/desktop/{uuid}. Shared so Agent can rebind.
    session_root: SharedSessionRoot,
}

impl SpawnSubagentTool {
    /// Construct with a shared session-root slot (typically held by [`crate::Agent`]).
    pub fn new(
        client: SharedLlm,
        tools: SharedTools,
        sandbox: SandboxConfig,
        session_root: SharedSessionRoot,
    ) -> Self {
        Self {
            client,
            tools,
            sandbox,
            session_root,
        }
    }

    /// Convenience: own a fresh unbound session-root slot.
    pub fn with_unbound_session(
        client: SharedLlm,
        tools: SharedTools,
        sandbox: SandboxConfig,
    ) -> Self {
        Self::new(client, tools, sandbox, Arc::new(Mutex::new(None)))
    }
}

#[async_trait]
impl Tool for SpawnSubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Run a thorough research subagent with read-only tools and return a compact summary. \
         Use for deep multi-query digs — person/profile research, multi-source web investigation, \
         or broad codebase exploration — while you stay on the main plan. \
         The child is expected to fan out searches, fetch sources, and reformulate before giving up. \
         Args: goal (required), max_rounds (optional, default 8, max 16)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What the subagent should investigate or gather (be specific)"
                },
                "max_rounds": {
                    "type": "integer",
                    "description": "Tool rounds budget (default 8, max 16)"
                }
            },
            "required": ["goal"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Other)
            .timeout(std::time::Duration::from_secs(SUBAGENT_TIMEOUT_SECS))
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(&self, ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let goal = require_string(obj, "goal")?;
        let max_rounds = obj
            .get("max_rounds")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_SUBAGENT_ROUNDS)
            .clamp(1, MAX_SUBAGENT_ROUNDS);

        // 1. Session must be bound for on-disk child storage.
        let session_root = {
            let guard = self
                .session_root
                .lock()
                .map_err(|_| ToolError::failed("subagent session_root lock"))?;
            guard.clone()
        };
        let Some(session_root) = session_root else {
            return Err(ToolError::failed(
                "session not bound; cannot spawn subagent",
            ));
        };

        let goal_preview = truncate_chars(goal.trim(), 42);
        ctx.report_text(format!("Research: {goal_preview}"));

        let child_tools: Vec<std::sync::Arc<dyn Tool>> = {
            let tools_arc = self
                .tools
                .lock()
                .map_err(|_| ToolError::failed("subagent tools lock"))?;
            tools_arc
                .iter()
                .filter(|t| {
                    if t.name() == "spawn_subagent" {
                        return false;
                    }
                    let m = t.meta();
                    // Prefer explicit meta.read_only (concurrency annotations).
                    m.is_read_only() && m.risk <= ToolRisk::Moderate
                })
                .cloned()
                .collect()
        };

        if child_tools.is_empty() {
            ctx.report_text("No read-only tools available");
            return Ok(truncate_tool_result(
                "Subagent has no read-only tools available.".into(),
            ));
        }

        // 2–4. Allocate child dir + initial meta.json.
        let child_id = generate_child_id();
        let child_dir = session_root.join("subagents").join(&child_id);
        fs::create_dir_all(&child_dir).map_err(|e| {
            ToolError::failed(format!("subagent create dir {}: {e}", child_dir.display()))
        })?;

        let started_ms = now_ms();
        let parent_session_id = ctx
            .session_id
            .clone()
            .or_else(|| {
                session_root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        write_meta(
            &child_dir,
            &json!({
                "child_id": child_id,
                "parent_session_id": parent_session_id,
                "parent_tool_call_id": ctx.call_id,
                "goal": goal,
                "status": "running",
                "started_ms": started_ms,
            }),
        );

        // 5. Child ToolRuntime audits to child tool_calls.jsonl (not NullAuditSink).
        let mut context = Context::new(SUBAGENT_CONTEXT_MAX_TURNS);
        context.push(Role::System, CHILD_SYSTEM_PROMPT);
        context.push(Role::User, goal.as_str());

        let audit_path = child_dir.join("tool_calls.jsonl");
        let runtime = ToolRuntime::new(
            self.sandbox.clone(),
            Box::new(JsonlAuditSink::new(audit_path)),
        );
        let features = ToolRuntimeFeatures {
            wave_scheduling: true,
            progress_events: true,
            force_list_all: true,
            ..Default::default()
        };
        let config = AgentLoopConfig {
            max_tool_rounds: max_rounds.min(DEFAULT_MAX_TOOL_ROUNDS),
            session_id: ctx.session_id.clone(),
            turn_id: ctx.turn_id.clone(),
            features,
            // Child registries are small; always list all child tools.
            force_list_all: true,
        };

        let state = LoopState {
            context: &mut context,
            tools: &child_tools,
            runtime: &runtime,
            client: self.client.as_ref(),
            activated: None,
        };

        // Bubble child lifecycle into parent tool progress → host overlay.
        let emit = child_progress_emit(ctx);
        let cancel = ctx.cancel.clone();

        let result = agent_loop(
            state,
            &goal,
            &config,
            vec![],
            0,
            0,
            cancel,
            Some(emit),
            None,
            0,
        )
        .await;

        // 6–7. Always finalize meta + summary (success or error).
        match result {
            Ok(result) => {
                ctx.report_text("Research complete");

                let summary = match result.outcome {
                    crate::outcome::AgentOutcome::Speak { text, .. } => text,
                    crate::outcome::AgentOutcome::Silent => {
                        "(subagent finished with no text)".into()
                    }
                    crate::outcome::AgentOutcome::NeedsConfirmation { text, .. } => {
                        format!("(subagent paused for confirm: {text})")
                    }
                };
                let tools = if result.tools_used.is_empty() {
                    "none".into()
                } else {
                    result.tools_used.join(", ")
                };

                let low_effort = research_effort_low(&result.tools_used, &summary);
                let mut body = summary;
                let effort_attr = if low_effort {
                    body.push_str(
                        "\nParent: re-run with more queries or research yourself; child under-tooled.",
                    );
                    r#" effort="low""#
                } else {
                    ""
                };

                let tool_result = format!(
                    "<subagent_result tools=\"{tools}\" rounds={}{effort_attr}>\n{body}\n</subagent_result>",
                    result.tool_rounds
                );
                finalize_child(&child_dir, "completed", &body, started_ms);
                Ok(truncate_tool_result_to_summary(tool_result))
            }
            Err(e) => {
                let msg = format!("subagent failed: {e}");
                finalize_child(&child_dir, "failed", &msg, started_ms);
                Err(ToolError::failed(msg))
            }
        }
    }
}

/// Write meta.json + summary.md and stamp ended_ms / final status.
fn finalize_child(child_dir: &Path, status: &str, summary: &str, started_ms: u64) {
    let ended_ms = now_ms();
    write_summary(child_dir, summary);

    // Re-read existing meta so we keep identity fields; fall back if missing.
    let meta_path = child_dir.join("meta.json");
    let mut meta: Value = fs::read_to_string(&meta_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = meta.as_object_mut() {
        obj.insert("status".into(), json!(status));
        obj.insert("started_ms".into(), json!(started_ms));
        obj.insert("ended_ms".into(), json!(ended_ms));
    } else {
        meta = json!({
            "status": status,
            "started_ms": started_ms,
            "ended_ms": ended_ms,
        });
    }
    write_meta(child_dir, &meta);
}

fn write_meta(child_dir: &Path, meta: &Value) {
    let path = child_dir.join("meta.json");
    match serde_json::to_string_pretty(meta) {
        Ok(s) => {
            if let Err(e) = fs::write(&path, s) {
                warn!(error = %e, path = %path.display(), "subagent meta write failed");
            }
        }
        Err(e) => {
            warn!(error = %e, "subagent meta serialize failed");
        }
    }
}

fn write_summary(child_dir: &Path, summary: &str) {
    let path = child_dir.join("summary.md");
    if let Err(e) = fs::write(&path, summary) {
        warn!(error = %e, path = %path.display(), "subagent summary write failed");
    }
}

/// Unique child id: wall-clock hex + process-local counter (no extra crate).
fn generate_child_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = now_ms();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{t:x}-{n:04x}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Structured system prompt for the research child.
const CHILD_SYSTEM_PROMPT: &str = "\
You are a thorough research subagent. Your job is multi-step investigation, not a single glance.

## Web / people / profile goals
- Fan out 3+ web_search queries with different phrasings in one multi-tool message.
- web_fetch the best candidate pages; match names, places, roles, and employers.
- If results are empty or weak, reformulate and search again — minimum 2 search waves \
before concluding not found.
- Never invent URLs, usernames, or profiles.

## Codebase / file goals
- Batch multi grep/glob/file_read (and list_dir when useful) in one step.
- Follow promising paths; do not stop after one empty grep.

## Output
When done, reply with compact bullets only:
- findings
- evidence (URLs or file paths)
- confidence (high/medium/low)
- what was tried (queries/tools) if incomplete
Do not call spawn_subagent.";

/// Tools that count as real research work for the effort soft-check.
const RESEARCH_TOOL_NAMES: &[&str] = &[
    "web_search",
    "web_fetch",
    "grep",
    "glob",
    "file_read",
    "list_dir",
    "memory_search",
];

/// True when the child under-used research tools and the summary looks thin/failure-like.
///
/// Soft signal for the parent: either fewer than two tool calls overall, or no
/// web/file research tools plus a short / "not found" style summary.
fn research_effort_low(tools_used: &[String], summary: &str) -> bool {
    if tools_used.len() < 2 {
        return true;
    }
    let has_research = tools_used.iter().any(|t| is_research_tool(t));
    if has_research {
        return false;
    }
    summary_looks_thin(summary)
}

fn is_research_tool(name: &str) -> bool {
    RESEARCH_TOOL_NAMES.contains(&name)
}

fn summary_looks_thin(summary: &str) -> bool {
    let trimmed = summary.trim();
    if trimmed.chars().count() < 80 {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.contains("not found")
        || lower.contains("nothing found")
        || lower.contains("no results")
        || lower.contains("nothing useful")
        || lower.contains("unable to find")
        || lower.contains("could not find")
        || (lower.contains("nothing") && lower.contains("found"))
}

/// Map child agent events → parent progress lines (rate-limited by EventProgressSink).
fn child_progress_emit(ctx: &ToolCallContext) -> EmitFn {
    let ctx = ctx.clone();
    Arc::new(move |ev: AgentEvent| {
        let line = match &ev {
            AgentEvent::ToolExecutionStart {
                tool_name,
                args_summary,
                ..
            } => {
                let detail = short_args_detail(tool_name, args_summary);
                if detail.is_empty() {
                    Some(format!("via {tool_name}"))
                } else {
                    Some(format!("via {tool_name}: {detail}"))
                }
            }
            AgentEvent::ToolProgress {
                tool_name, message, ..
            } => {
                let msg = message.trim();
                if msg.is_empty() {
                    Some(format!("via {tool_name}"))
                } else {
                    Some(format!("via {tool_name}: {}", truncate_chars(msg, 48)))
                }
            }
            AgentEvent::ToolExecutionEnd { tool_name, ok, .. } => Some(if *ok {
                format!("via {tool_name} · done")
            } else {
                format!("via {tool_name} · failed")
            }),
            AgentEvent::TurnStart { round } if *round > 0 => Some(format!("step {}", round + 1)),
            AgentEvent::Error { message } => {
                let m = message.trim();
                if m.is_empty() {
                    Some("subagent error".into())
                } else {
                    Some(format!("error: {}", truncate_chars(m, 40)))
                }
            }
            _ => None,
        };
        if let Some(text) = line {
            ctx.report_text(text);
        }
    })
}

fn short_args_detail(tool_name: &str, args_summary: &str) -> String {
    let s = args_summary.trim();
    if s.is_empty() || s == tool_name {
        return String::new();
    }
    let inner = s
        .strip_prefix(tool_name)
        .map(str::trim)
        .and_then(|rest| rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')))
        .unwrap_or(s);
    truncate_chars(inner.trim(), 40)
}

fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

fn truncate_tool_result_to_summary(s: String) -> String {
    truncate_tool_result(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_context::ToolCallContext;
    use boris_ai::error::LlmError;

    #[test]
    fn truncate_chars_short_unchanged() {
        assert_eq!(truncate_chars("hello", 10), "hello");
    }

    #[test]
    fn truncate_chars_long_ellipsis() {
        let out = truncate_chars("abcdefghij", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
        assert!(out.starts_with("abcd"));
    }

    #[test]
    fn truncate_chars_unicode() {
        let out = truncate_chars("日本語テスト文字", 4);
        assert_eq!(out.chars().count(), 4);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn effort_low_when_fewer_than_two_tools() {
        assert!(research_effort_low(&[], "some findings about Alice"));
        assert!(research_effort_low(
            &["web_search".into()],
            "Found a promising lead at example.com with details"
        ));
    }

    #[test]
    fn effort_ok_with_two_research_tools() {
        assert!(!research_effort_low(
            &["web_search".into(), "web_fetch".into()],
            "not found" // has research tools → not low
        ));
        assert!(!research_effort_low(
            &["grep".into(), "file_read".into()],
            "Found match in src/lib.rs"
        ));
    }

    #[test]
    fn effort_low_no_research_tools_and_thin_summary() {
        assert!(research_effort_low(
            &["get_time".into(), "get_date".into()],
            "not found"
        ));
        assert!(research_effort_low(
            &["get_time".into(), "todo_read".into()],
            "short"
        ));
        assert!(research_effort_low(
            &["recall_notes".into(), "get_user_context".into()],
            "Nothing useful came back from the tools I ran."
        ));
    }

    #[test]
    fn effort_ok_no_research_but_substantive_summary() {
        // Two non-research tools but a long summary without failure phrasing.
        let long = "x".repeat(100);
        assert!(!research_effort_low(
            &["get_time".into(), "todo_read".into()],
            &long
        ));
    }

    #[test]
    fn is_research_tool_names() {
        assert!(is_research_tool("web_search"));
        assert!(is_research_tool("web_fetch"));
        assert!(is_research_tool("grep"));
        assert!(is_research_tool("glob"));
        assert!(is_research_tool("file_read"));
        assert!(is_research_tool("list_dir"));
        assert!(is_research_tool("memory_search"));
        assert!(!is_research_tool("bash"));
        assert!(!is_research_tool("spawn_subagent"));
    }

    #[test]
    fn summary_looks_thin_patterns() {
        assert!(summary_looks_thin("not found"));
        assert!(summary_looks_thin("No results for that query"));
        assert!(summary_looks_thin("I could not find anyone matching"));
        assert!(!summary_looks_thin(
            "Found Jane Doe, software engineer at Acme Corp in Austin; evidence: https://example.com/jdoe"
        ));
    }

    #[test]
    fn constants_match_spec() {
        assert_eq!(DEFAULT_SUBAGENT_ROUNDS, 8);
        assert_eq!(MAX_SUBAGENT_ROUNDS, 16);
        assert_eq!(SUBAGENT_TIMEOUT_SECS, 180);
    }

    #[test]
    fn generate_child_id_is_unique() {
        let a = generate_child_id();
        let b = generate_child_id();
        assert_ne!(a, b);
        assert!(a.contains('-'));
    }

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

    #[tokio::test]
    async fn unbound_session_returns_error() {
        let tool = SpawnSubagentTool::new(
            Arc::new(NoopClient),
            Arc::new(Mutex::new(Vec::new())),
            SandboxConfig::default(),
            Arc::new(Mutex::new(None)),
        );
        let ctx = ToolCallContext::new("call-test");
        let err = tool
            .execute(&ctx, json!({ "goal": "find Alice" }))
            .await
            .expect_err("unbound session must fail");
        assert!(
            err.message.contains("session not bound"),
            "unexpected message: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn bound_session_writes_failed_child_on_llm_error() {
        let root = std::env::temp_dir().join(format!("boris-subagent-test-{}", now_ms()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // One read-only tool so the child is eligible to run.
        struct RoTool;
        #[async_trait]
        impl Tool for RoTool {
            fn name(&self) -> &str {
                "list_dir"
            }
            fn description(&self) -> &str {
                "list"
            }
            fn parameters(&self) -> Value {
                json!({"type": "object", "properties": {}})
            }
            fn meta(&self) -> ToolMeta {
                ToolMeta::with_risk(ToolRisk::Safe)
                    .kind(ToolKind::Read)
                    .read_only(true)
            }
            async fn execute(
                &self,
                _ctx: &ToolCallContext,
                _args: Value,
            ) -> Result<String, ToolError> {
                Ok("ok".into())
            }
        }

        let session_root = Arc::new(Mutex::new(Some(root.clone())));
        let tool = SpawnSubagentTool::new(
            Arc::new(NoopClient),
            Arc::new(Mutex::new(vec![Arc::new(RoTool) as Arc<dyn Tool>])),
            SandboxConfig::default(),
            Arc::clone(&session_root),
        );
        let ctx = ToolCallContext::new("call-bound")
            .with_session(Some("sess-abc".into()), Some("turn-1".into()));
        let err = tool
            .execute(&ctx, json!({ "goal": "find Bob" }))
            .await
            .expect_err("noop LLM should fail the child loop");
        assert!(err.message.contains("subagent failed"));

        let subagents = root.join("subagents");
        assert!(subagents.is_dir(), "subagents/ should exist");
        let mut children: Vec<_> = fs::read_dir(&subagents)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(children.len(), 1, "exactly one child dir");
        let child_dir = children.pop().unwrap().path();

        let meta_raw = fs::read_to_string(child_dir.join("meta.json")).unwrap();
        let meta: Value = serde_json::from_str(&meta_raw).unwrap();
        assert_eq!(meta["status"], "failed");
        assert_eq!(meta["parent_session_id"], "sess-abc");
        assert_eq!(meta["parent_tool_call_id"], "call-bound");
        assert_eq!(meta["goal"], "find Bob");
        assert!(meta["started_ms"].as_u64().unwrap() > 0);
        assert!(meta["ended_ms"].as_u64().unwrap() > 0);

        let summary = fs::read_to_string(child_dir.join("summary.md")).unwrap();
        assert!(summary.contains("subagent failed"));

        // tool_calls.jsonl is created on first audit write; empty loop may leave it
        // missing — presence of meta + summary is the finalize contract.
        let _ = fs::remove_dir_all(&root);
    }
}
