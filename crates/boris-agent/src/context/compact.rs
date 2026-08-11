//! Mechanical and summary compaction for conversation context.
//!
//! Inspired by tau: truncate large tool observations, collapse older tool chains,
//! and optionally replace middle history with an LLM-written summary.
//!
//! Design goals (voice agent quality):
//! - **Never gut the current research turn** — recent tool results stay large.
//! - **Prefer truncate over drop** until the hard budget is hit.
//! - **Only collapse** old tool_calls chains when we are truly over hard budget.

use serde_json::Value;

use super::turns::{body_start, user_turn_starts};
use super::{Context, Message, Role};

/// Collapse a large tool observation to head + tail with a compact marker.
pub(super) fn truncate_tool_text(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let half = max_chars / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(half)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{head}\n…[compacted]…\n{tail}")
}

pub(super) fn value_preview(v: &Value, max: usize) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if s.chars().count() <= max {
        s
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

pub(super) fn estimate_message_chars(m: &Message) -> usize {
    match &m.content {
        Value::String(s) => s.len(),
        other => other.to_string().len(),
    }
}

impl Context {
    // ── Mechanical compaction (tau-inspired, no LLM) ─────────────────────────

    /// Rough token estimate (chars / 4) across all messages.
    pub fn estimate_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| estimate_message_chars(m) / 4)
            .sum()
    }

    /// Soft token budget: mild truncation of *older* tools only; LLM compact may run.
    /// Raised so multi-step research + web results are not crushed mid-session.
    pub const COMPACT_TOKEN_SOFT: usize = 64_000;
    /// Hard token budget: aggressive truncate + collapse of old tool chains.
    pub const COMPACT_TOKEN_HARD: usize = 120_000;

    /// How many recent user turns keep full-size tool observations.
    pub const KEEP_RECENT_TOOL_TURNS: usize = 6;
    /// At hard budget, still protect this many recent turns from collapse.
    pub const KEEP_RECENT_TOOL_TURNS_HARD: usize = 4;

    /// Apply mechanical reduction before an LLM call:
    /// 1. Truncate large tool observations (recent turns keep a high cap)
    /// 2. Only at **hard** budget: collapse tool chains older than keep turns
    pub fn compact_mechanical(&mut self) {
        let tokens = self.estimate_tokens();
        let (recent_cap, older_cap) = if tokens > Self::COMPACT_TOKEN_HARD {
            (12_000, 3_000)
        } else if tokens > Self::COMPACT_TOKEN_SOFT {
            (20_000, 6_000)
        } else {
            // Comfortable: barely touch recent tools; older can still be large.
            (32_000, 12_000)
        };
        let keep_tool_turns = if tokens > Self::COMPACT_TOKEN_HARD {
            Self::KEEP_RECENT_TOOL_TURNS_HARD
        } else {
            Self::KEEP_RECENT_TOOL_TURNS
        };

        let body = body_start(&self.messages);
        let turn_starts = user_turn_starts(&self.messages, body);
        let keep_from = if turn_starts.len() > keep_tool_turns {
            turn_starts[turn_starts.len() - keep_tool_turns]
        } else {
            body
        };

        // Tier 1: truncate large tool results — recent turns keep more.
        for (idx, msg) in self.messages.iter_mut().enumerate() {
            if !matches!(msg.role, Role::Tool) {
                continue;
            }
            let cap = if idx >= keep_from {
                recent_cap
            } else {
                older_cap
            };
            if let Some(content) = msg.content.get_mut("content") {
                if let Some(s) = content.as_str() {
                    if s.chars().count() > cap {
                        *content = Value::String(truncate_tool_text(s, cap));
                    }
                }
            }
        }

        // Tier 2: collapse old tool chains **only under hard budget**.
        // Soft budget prefers keeping structure so follow-ups still see research.
        if tokens <= Self::COMPACT_TOKEN_HARD {
            return;
        }
        if turn_starts.len() <= keep_tool_turns {
            return;
        }

        // Important: store a *plain string* for collapsed assistants (not a nested
        // `{role, content}` object). `Message::dump` wraps `content` again, and a
        // nested object becomes `messages.N.content: Invalid input` on OpenRouter.
        // Also drop the matching tool messages so we never leave orphan `role:tool`
        // rows without a preceding `tool_calls` assistant.
        for msg in self.messages[body..keep_from].iter_mut() {
            if matches!(msg.role, Role::Assistant) && msg.content.get("tool_calls").is_some() {
                let summary = summarize_tool_batch(&msg.content);
                msg.content = Value::String(summary);
            }
        }
        // Remove tool results in the collapsed window (orphans after stripping tool_calls).
        let mut idx = 0usize;
        self.messages.retain(|m| {
            let drop = idx >= body && idx < keep_from && matches!(m.role, Role::Tool);
            idx += 1;
            !drop
        });
    }

    /// True when the host should run an LLM summary compact pass.
    pub fn needs_llm_compact(&self) -> bool {
        // Need real multi-turn history AND soft budget pressure.
        self.estimate_tokens() > Self::COMPACT_TOKEN_SOFT && self.user_turn_count() >= 5
    }

    /// Replace middle history with a single summary user/assistant pair.
    ///
    /// Keeps: system, optional summary block, last `keep_turns` user turns.
    pub fn apply_summary_compact(&mut self, summary: &str, keep_turns: usize) {
        if summary.trim().is_empty() {
            return;
        }
        let keep_turns = keep_turns.max(1);
        let body = body_start(&self.messages);
        let has_system = body == 1;
        let turn_starts = user_turn_starts(&self.messages, body);
        if turn_starts.len() <= keep_turns + 1 {
            return;
        }
        let keep_from = turn_starts[turn_starts.len() - keep_turns];
        let system = if has_system {
            Some(self.messages[0].clone())
        } else {
            None
        };
        let recent: Vec<Message> = self.messages[keep_from..].to_vec();
        self.messages.clear();
        if let Some(sys) = system {
            self.messages.push(sys);
        }
        self.messages.push(Message {
            role: Role::User,
            content: Value::String(format!(
                "<conversation_summary>\n{summary}\n</conversation_summary>"
            )),
        });
        self.messages.push(Message {
            role: Role::Assistant,
            content: Value::String(
                "Got it — I'll use that summary as prior context and keep going.".into(),
            ),
        });
        self.messages.extend(recent);
    }

    /// Collect text from older turns for an LLM summarizer (capped).
    pub fn older_turns_digest(&self, keep_recent_turns: usize) -> String {
        let body = body_start(&self.messages);
        let turn_starts = user_turn_starts(&self.messages, body);
        if turn_starts.len() <= keep_recent_turns {
            return String::new();
        }
        let end = turn_starts[turn_starts.len() - keep_recent_turns];
        let mut out = String::new();
        for msg in &self.messages[body..end] {
            let line = match msg.role {
                Role::User => format!("User: {}\n", value_preview(&msg.content, 600)),
                Role::Assistant => format!("Assistant: {}\n", value_preview(&msg.content, 600)),
                // Keep more tool signal so summaries retain findings / paths / URLs.
                Role::Tool => format!("Tool: {}\n", value_preview(&msg.content, 500)),
                Role::System => continue,
            };
            out.push_str(&line);
            if out.len() > 20_000 {
                out.push_str("…\n");
                break;
            }
        }
        out
    }
}

/// Build a plain-string stand-in for a collapsed tool_calls assistant message.
///
/// Includes tool names (and short arg snippets) so the model retains what it did.
fn summarize_tool_batch(content: &Value) -> String {
    let calls = content
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    let n = calls.len();
    let mut names: Vec<String> = Vec::new();
    for tc in calls.iter().take(8) {
        let name = tc
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("tool");
        let args = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(|a| a.as_str())
            .unwrap_or("");
        let args_preview: String = args.chars().take(80).collect();
        if args_preview.is_empty() {
            names.push(name.to_string());
        } else {
            names.push(format!("{name}({args_preview})"));
        }
    }
    if n > 8 {
        names.push(format!("…+{} more", n - 8));
    }
    format!(
        "[prior tool batch: {n} call(s) — details omitted] {}",
        names.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truncate_tool_text_short_unchanged() {
        assert_eq!(truncate_tool_text("hi", 100), "hi");
    }

    #[test]
    fn truncate_tool_text_long_has_marker() {
        let s = "a".repeat(100);
        let out = truncate_tool_text(&s, 20);
        assert!(out.contains("…[compacted]…"));
        assert!(out.chars().count() < 100);
    }

    #[test]
    fn value_preview_caps_long_strings() {
        let s = "x".repeat(50);
        let out = value_preview(&Value::String(s), 10);
        assert_eq!(out.chars().count(), 11); // 10 + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn value_preview_short_unchanged() {
        assert_eq!(value_preview(&json!("hi"), 10), "hi");
    }

    #[test]
    fn estimate_message_chars_string_and_object() {
        let m = Message {
            role: Role::User,
            content: Value::String("abcd".into()),
        };
        assert_eq!(estimate_message_chars(&m), 4);
        let m2 = Message {
            role: Role::Assistant,
            content: json!({"a": 1}),
        };
        assert_eq!(estimate_message_chars(&m2), m2.content.to_string().len());
    }

    #[test]
    fn estimate_tokens_is_chars_over_four() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "a".repeat(40));
        assert_eq!(ctx.estimate_tokens(), 10);
    }

    #[test]
    fn needs_llm_compact_requires_budget_and_turns() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "u1");
        ctx.push(Role::User, "u2");
        // Only 2 user turns → false even if tokens high.
        assert!(!ctx.needs_llm_compact());
        ctx.push(Role::User, "u3");
        // 3 turns but tiny tokens → false.
        assert!(!ctx.needs_llm_compact());
    }

    #[test]
    fn apply_summary_compact_inserts_summary_pair() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        for i in 0..5 {
            ctx.push(Role::User, format!("u{i}"));
            ctx.push(Role::Assistant, format!("a{i}"));
        }
        ctx.apply_summary_compact("did stuff", 2);
        let roles: Vec<_> = ctx
            .messages
            .iter()
            .map(|m| match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            })
            .collect();
        // system + summary user + summary assistant + last 2 turns (u/a × 2)
        assert_eq!(
            roles,
            vec![
                "system",
                "user",
                "assistant",
                "user",
                "assistant",
                "user",
                "assistant"
            ]
        );
        assert!(ctx.messages[1]
            .content
            .as_str()
            .unwrap()
            .contains("<conversation_summary>"));
        assert!(ctx.messages[1]
            .content
            .as_str()
            .unwrap()
            .contains("did stuff"));
        assert_eq!(ctx.messages[3].content.as_str().unwrap(), "u3");
    }

    #[test]
    fn apply_summary_compact_empty_is_noop() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "u1");
        ctx.push(Role::User, "u2");
        ctx.push(Role::User, "u3");
        ctx.apply_summary_compact("   ", 1);
        assert_eq!(ctx.messages.len(), 3);
    }

    #[test]
    fn older_turns_digest_returns_older_only() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "old-user");
        ctx.push(Role::Assistant, "old-asst");
        ctx.push(Role::User, "new-user");
        ctx.push(Role::Assistant, "new-asst");
        let dig = ctx.older_turns_digest(1);
        assert!(dig.contains("old-user"));
        assert!(dig.contains("old-asst"));
        assert!(!dig.contains("new-user"));
    }

    #[test]
    fn older_turns_digest_empty_when_nothing_to_drop() {
        let mut ctx = Context::new(20);
        ctx.push(Role::User, "only");
        assert_eq!(ctx.older_turns_digest(1), "");
    }

    #[test]
    fn compact_mechanical_keeps_tool_chains_under_soft_budget() {
        // Soft budget must NOT gut research after a few voice turns.
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" }
                }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_1", "content": "file1\nfile2\nfile3" }),
        );
        ctx.push(Role::Assistant, "done u1");
        for i in 2..8 {
            ctx.push(Role::User, format!("u{i}"));
            ctx.push(Role::Assistant, format!("a{i}"));
        }
        assert!(ctx.estimate_tokens() < Context::COMPACT_TOKEN_SOFT);
        ctx.compact_mechanical();

        assert!(
            ctx.messages.iter().any(|m| matches!(m.role, Role::Tool)),
            "tool results must survive under soft budget"
        );
        assert!(
            !ctx.messages.iter().any(|m| {
                m.content
                    .as_str()
                    .is_some_and(|s| s.contains("prior tool batch"))
            }),
            "must not collapse tool batches under soft budget"
        );
    }

    #[test]
    fn compact_mechanical_collapses_old_tool_chains_at_hard_budget() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");

        // Turn 1: tool chain (will be compacted once hard budget + enough turns)
        ctx.push(Role::User, "u1");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{\"cmd\":\"ls\"}" }
                }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_1", "content": "file1\nfile2\nfile3" }),
        );
        ctx.push(Role::Assistant, "done u1");

        // Extra user turns beyond KEEP_RECENT_TOOL_TURNS_HARD.
        for i in 2..8 {
            ctx.push(Role::User, format!("u{i}"));
            ctx.push(Role::Assistant, format!("a{i}"));
        }
        // Pad to hard budget so collapse engages.
        ctx.push(Role::User, "pad".repeat(Context::COMPACT_TOKEN_HARD * 4 / 3));

        assert!(ctx.estimate_tokens() > Context::COMPACT_TOKEN_HARD);
        ctx.compact_mechanical();

        // Turn-1 assistant tool_calls → plain summary string (not nested object).
        let collapsed = ctx
            .messages
            .iter()
            .find(|m| {
                matches!(m.role, Role::Assistant)
                    && m.content
                        .as_str()
                        .is_some_and(|s| s.contains("prior tool batch"))
            })
            .expect("collapsed tool batch summary");
        assert!(collapsed.content.is_string());
        assert!(
            collapsed
                .content
                .as_str()
                .unwrap()
                .contains("bash"),
            "collapse summary should retain tool name"
        );

        // Orphan tool results from the collapsed window must be gone.
        assert!(
            !ctx.messages.iter().any(|m| matches!(m.role, Role::Tool)),
            "old tool messages should be dropped after collapse"
        );

        // Wire dump must be OpenRouter-safe (string content everywhere).
        let wire = ctx.as_json();
        let arr = wire.as_array().unwrap();
        for (i, m) in arr.iter().enumerate() {
            let c = &m["content"];
            assert!(
                c.is_string() || c.is_array(),
                "messages[{i}].content must be string/array, got {c}"
            );
            assert!(!c.is_object(), "messages[{i}].content must not be object");
        }
    }

    #[test]
    fn compact_mechanical_preserves_medium_tool_results_under_soft() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        // 10k used to always truncate (old 6k cap); under soft budget it must survive.
        let body = "z".repeat(10_000);
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "c1", "content": body.clone() }),
        );
        assert!(ctx.estimate_tokens() < Context::COMPACT_TOKEN_SOFT);
        ctx.compact_mechanical();
        let tool = ctx
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::Tool))
            .unwrap();
        let s = tool.content["content"].as_str().unwrap();
        assert_eq!(s.len(), body.len(), "10k tool result must not compact under soft");
        assert!(!s.contains("…[compacted]…"));
    }

    #[test]
    fn compact_mechanical_truncates_huge_tool_results() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        let big = "z".repeat(80_000);
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "c1", "content": big }),
        );
        ctx.compact_mechanical();
        let tool = ctx
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::Tool))
            .unwrap();
        let s = tool.content["content"].as_str().unwrap();
        assert!(s.contains("…[compacted]…"));
        assert!(s.chars().count() < 80_000);
    }
}
