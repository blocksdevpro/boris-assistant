use std::fmt;
use std::str::FromStr;

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        f.write_str(s)
    }
}

impl Role {
    /// Parse a transcript / wire role string (`"user"`, `"assistant"`, …).
    ///
    /// Unknown roles return `None` (callers skip them when rebuilding history).
    pub fn from_role_str(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Role::System),
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            "tool" => Some(Role::Tool),
            _ => None,
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Role::from_role_str(s).ok_or(())
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    /// For User/System/Assistant: a plain string or content array.
    /// For Tool: a JSON object `{ tool_call_id, content }`.
    /// For Assistant with tool_calls: the raw message object from the LLM.
    pub content: Value,
}

#[derive(Debug, Default)]
pub struct Context {
    pub messages: Vec<Message>,
    pub max_turns: u32,
}

impl Message {
    /// Serialize to an OpenAI / OpenRouter chat-completions message object.
    ///
    /// Always coerces `content` to a string (or keeps a valid parts array). Nested
    /// objects from older compaction bugs are flattened so providers never see
    /// `messages.N.content: Invalid input`.
    pub fn dump(&self) -> Value {
        match self.role {
            // Tool result — must surface tool_call_id at the top level.
            Role::Tool => {
                let tool_call_id = self
                    .content
                    .get("tool_call_id")
                    .cloned()
                    .unwrap_or_else(|| Value::String(String::new()));
                let content = coerce_message_content(
                    self.content.get("content").unwrap_or(&Value::Null),
                );
                json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": content,
                })
            }
            // Assistant with tool_calls — forward the raw object, normalize content.
            Role::Assistant if self.content.get("tool_calls").is_some() => {
                let mut msg = self.content.clone();
                if let Some(obj) = msg.as_object_mut() {
                    obj.insert("role".into(), json!("assistant"));
                    let raw = obj.get("content").cloned().unwrap_or(Value::Null);
                    // Prefer empty string over null — several OpenRouter routes reject null.
                    obj.insert("content".into(), coerce_message_content(&raw));
                }
                msg
            }
            // Everything else (system / user / plain assistant).
            _ => json!({
                "role":    self.role.to_string(),
                "content": coerce_message_content(&self.content),
            }),
        }
    }
}

/// Coerce stored message content into a wire-safe OpenAI `content` value.
///
/// - strings stay strings
/// - null / missing → `""`
/// - nested `{ "role", "content" }` (legacy compact bug) → unwrap inner content
/// - other JSON → stringified (tools always need a string body)
fn coerce_message_content(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.clone()),
        Value::Null => Value::String(String::new()),
        // Multimodal content parts — pass through when well-formed.
        Value::Array(parts) if !parts.is_empty() => {
            let looks_like_parts = parts.iter().all(|p| {
                p.get("type").and_then(|t| t.as_str()).is_some()
            });
            if looks_like_parts {
                v.clone()
            } else {
                Value::String(v.to_string())
            }
        }
        // Botched compact / double-wrap: `{ "role": "assistant", "content": "..." }`.
        Value::Object(map) if map.contains_key("content") => {
            coerce_message_content(map.get("content").unwrap_or(&Value::Null))
        }
        other => Value::String(other.to_string()),
    }
}

impl Context {
    pub fn new(max_turns: u32) -> Self {
        Self {
            messages: Vec::new(),
            max_turns,
        }
    }

    pub fn push(&mut self, role: Role, content: impl Into<Value>) {
        self.messages.push(Message {
            role,
            content: content.into(),
        });
        self.prune();
    }

    /// Replace or insert the leading system message (used when personal context refreshes).
    pub fn set_system(&mut self, content: impl Into<Value>) {
        let content = content.into();
        if self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System))
        {
            self.messages[0].content = content;
        } else {
            self.messages.insert(
                0,
                Message {
                    role: Role::System,
                    content,
                },
            );
        }
    }

    /// Replace non-system messages with history loaded from a session.
    ///
    /// Always installs `system_prompt` as the first message (current prompt wins
    /// over any system rows that may appear in `history`). Remaining history
    /// messages that are not `Role::System` are appended, then pruned once.
    pub fn load_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.messages.clear();
        self.messages.push(Message {
            role: Role::System,
            content: Value::String(system_prompt.to_string()),
        });
        for msg in history {
            if !matches!(msg.role, Role::System) {
                self.messages.push(msg);
            }
        }
        self.prune();
    }

    /// All messages (borrowed) for persistence / debug.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Build [`Message`] list from transcript role strings + content values.
    ///
    /// Skips unknown roles. Prefer this over importing session transcript types
    /// so `context` stays free of a dependency cycle with `session`.
    pub fn messages_from_transcript(records: &[(String, Value)]) -> Vec<Message> {
        records
            .iter()
            .filter_map(|(role, content)| {
                let role = Role::from_role_str(role)?;
                Some(Message {
                    role,
                    content: content.clone(),
                })
            })
            .collect()
    }

    /// Prune conversation history by **user turns**, not raw message count.
    ///
    /// Algorithm:
    /// 1. Optionally keep index-0 `Role::System` forever.
    /// 2. Partition remaining messages into turn groups; each group starts at
    ///    a `Role::User` and includes all following assistant/tool traffic
    ///    until the next user message.
    /// 3. Keep only the last `max_turns` groups (oldest turns dropped first).
    /// 4. Rebuild: `[system?] + kept groups`.
    ///
    /// This never splits an assistant `tool_calls` message from its following
    /// `Role::Tool` results, because those always live inside the same turn group.
    fn prune(&mut self) {
        if self.messages.is_empty() {
            return;
        }

        let has_system = matches!(self.messages[0].role, Role::System);
        let body_start = if has_system { 1 } else { 0 };

        // Indices in `messages` where each user-turn group begins.
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate().skip(body_start) {
            if matches!(msg.role, Role::User) {
                turn_starts.push(i);
            }
        }

        let max = self.max_turns as usize;

        // max_turns == 0: drop every user turn; keep only the system prompt (if any).
        if max == 0 {
            self.messages.truncate(body_start);
            return;
        }

        // No user turns, or already within budget — nothing to drop.
        if turn_starts.is_empty() || turn_starts.len() <= max {
            return;
        }

        // First message index of the oldest turn we still keep.
        let keep_from = turn_starts[turn_starts.len() - max];

        if has_system {
            // Retain system at index 0, then only the kept turn groups.
            let system = self.messages[0].clone();
            let kept: Vec<Message> = self.messages.drain(keep_from..).collect();
            self.messages.clear();
            self.messages.push(system);
            self.messages.extend(kept);
        } else {
            self.messages.drain(0..keep_from);
        }
    }

    pub fn as_json(&self) -> Value {
        let messages: Vec<Value> = self.messages.iter().map(|m| m.dump()).collect();
        json!(messages)
    }

    // ── Mechanical compaction (tau-inspired, no LLM) ─────────────────────────

    /// Rough token estimate (chars / 4) across all messages.
    pub fn estimate_tokens(&self) -> usize {
        self.messages
            .iter()
            .map(|m| estimate_message_chars(m) / 4)
            .sum()
    }

    /// Soft token budget before aggressive mechanical compact + LLM compact trigger.
    pub const COMPACT_TOKEN_SOFT: usize = 24_000;
    /// Hard token budget: always mask older tool noise aggressively.
    pub const COMPACT_TOKEN_HARD: usize = 40_000;

    /// Apply mechanical reduction before an LLM call:
    /// 1. Truncate large tool observations in place
    /// 2. Mask tool results older than the last `keep_tool_turns` user turns
    /// 3. At hard budget: collapse older assistant tool_calls blobs
    pub fn compact_mechanical(&mut self) {
        let tokens = self.estimate_tokens();
        let max_tool_chars = if tokens > Self::COMPACT_TOKEN_HARD {
            2_000
        } else if tokens > Self::COMPACT_TOKEN_SOFT {
            3_500
        } else {
            6_000
        };
        let keep_tool_turns = if tokens > Self::COMPACT_TOKEN_HARD {
            1
        } else {
            2
        };

        // Tier 1: truncate large tool results.
        for msg in &mut self.messages {
            if !matches!(msg.role, Role::Tool) {
                continue;
            }
            if let Some(content) = msg.content.get_mut("content") {
                if let Some(s) = content.as_str() {
                    if s.chars().count() > max_tool_chars {
                        let head: String = s.chars().take(max_tool_chars / 2).collect();
                        let tail: String = s
                            .chars()
                            .rev()
                            .take(max_tool_chars / 2)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect();
                        *content = Value::String(format!("{head}\n…[compacted]…\n{tail}"));
                    }
                }
            }
        }

        // Tier 2: collapse tool chains from turns older than keep_tool_turns.
        //
        // Important: store a *plain string* for collapsed assistants (not a nested
        // `{role, content}` object). `Message::dump` wraps `content` again, and a
        // nested object becomes `messages.N.content: Invalid input` on OpenRouter.
        // Also drop the matching tool messages so we never leave orphan `role:tool`
        // rows without a preceding `tool_calls` assistant.
        let has_system = self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System));
        let body_start = if has_system { 1 } else { 0 };
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate().skip(body_start) {
            if matches!(msg.role, Role::User) {
                turn_starts.push(i);
            }
        }
        if turn_starts.len() > keep_tool_turns {
            let keep_from = turn_starts[turn_starts.len() - keep_tool_turns];
            for msg in self.messages[body_start..keep_from].iter_mut() {
                if matches!(msg.role, Role::Assistant) && msg.content.get("tool_calls").is_some() {
                    let n = msg
                        .content
                        .get("tool_calls")
                        .and_then(|t| t.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    // Plain string — dump will emit `{ "role": "assistant", "content": "..." }`.
                    msg.content = Value::String(format!(
                        "[prior tool batch: {n} call(s) — details omitted]"
                    ));
                }
            }
            // Remove tool results in the collapsed window (orphans after stripping tool_calls).
            let mut idx = 0usize;
            self.messages.retain(|m| {
                let drop = idx >= body_start
                    && idx < keep_from
                    && matches!(m.role, Role::Tool);
                idx += 1;
                !drop
            });
        }
    }

    /// True when the host should run an LLM summary compact pass.
    pub fn needs_llm_compact(&self) -> bool {
        self.estimate_tokens() > Self::COMPACT_TOKEN_SOFT && self.user_turn_count() >= 3
    }

    /// Count user turns (excludes system).
    pub fn user_turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .count()
    }

    /// Replace middle history with a single summary user/assistant pair.
    ///
    /// Keeps: system, optional summary block, last `keep_turns` user turns.
    pub fn apply_summary_compact(&mut self, summary: &str, keep_turns: usize) {
        if summary.trim().is_empty() {
            return;
        }
        let keep_turns = keep_turns.max(1);
        let has_system = self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System));
        let body_start = if has_system { 1 } else { 0 };
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate().skip(body_start) {
            if matches!(msg.role, Role::User) {
                turn_starts.push(i);
            }
        }
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
        let has_system = self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System));
        let body_start = if has_system { 1 } else { 0 };
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in self.messages.iter().enumerate().skip(body_start) {
            if matches!(msg.role, Role::User) {
                turn_starts.push(i);
            }
        }
        if turn_starts.len() <= keep_recent_turns {
            return String::new();
        }
        let end = turn_starts[turn_starts.len() - keep_recent_turns];
        let mut out = String::new();
        for msg in &self.messages[body_start..end] {
            let line = match msg.role {
                Role::User => format!("User: {}\n", value_preview(&msg.content, 400)),
                Role::Assistant => format!("Assistant: {}\n", value_preview(&msg.content, 400)),
                Role::Tool => format!("Tool: {}\n", value_preview(&msg.content, 200)),
                Role::System => continue,
            };
            out.push_str(&line);
            if out.len() > 12_000 {
                out.push_str("…\n");
                break;
            }
        }
        out
    }
}

fn value_preview(v: &Value, max: usize) -> String {
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

fn estimate_message_chars(m: &Message) -> usize {
    match &m.content {
        Value::String(s) => s.len(),
        other => other.to_string().len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn roles_of(ctx: &Context) -> Vec<&'static str> {
        ctx.messages
            .iter()
            .map(|m| match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            })
            .collect()
    }

    fn text(m: &Message) -> String {
        match &m.content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    #[test]
    fn prune_plain_user_assistant_pairs() {
        let mut ctx = Context::new(2);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        ctx.push(Role::User, "u2");
        ctx.push(Role::Assistant, "a2");
        // Still within budget.
        assert_eq!(
            roles_of(&ctx),
            vec!["system", "user", "assistant", "user", "assistant"]
        );
        assert_eq!(text(&ctx.messages[1]), "u1");

        // Third turn drops the oldest (u1/a1).
        ctx.push(Role::User, "u3");
        ctx.push(Role::Assistant, "a3");
        assert_eq!(
            roles_of(&ctx),
            vec!["system", "user", "assistant", "user", "assistant"]
        );
        assert_eq!(text(&ctx.messages[1]), "u2");
        assert_eq!(text(&ctx.messages[2]), "a2");
        assert_eq!(text(&ctx.messages[3]), "u3");
        assert_eq!(text(&ctx.messages[4]), "a3");
    }

    #[test]
    fn prune_keeps_tool_call_chains_together() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "sys");

        // Turn 1: user → assistant(tool_calls) → tool → assistant(final)
        ctx.push(Role::User, "u1");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_1", "type": "function" }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_1", "content": "tool-result-1" }),
        );
        ctx.push(Role::Assistant, "final-1");

        // Turn 2 with its own tool chain — turn 1 must drop as a whole.
        ctx.push(Role::User, "u2");
        ctx.push(
            Role::Assistant,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{ "id": "call_2", "type": "function" }]
            }),
        );
        ctx.push(
            Role::Tool,
            json!({ "tool_call_id": "call_2", "content": "tool-result-2" }),
        );

        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant", "tool"]);
        assert_eq!(text(&ctx.messages[1]), "u2");
        // Assistant tool_calls still paired with its tool result.
        assert!(ctx.messages[2].content.get("tool_calls").is_some());
        assert_eq!(ctx.messages[3].content["tool_call_id"], json!("call_2"));
        // No leftover turn-1 tool results (would be orphaned without assistant).
        assert!(!ctx
            .messages
            .iter()
            .any(|m| m.content.get("tool_call_id") == Some(&json!("call_1"))));
    }

    #[test]
    fn prune_never_drops_system_prompt() {
        let mut ctx = Context::new(1);
        ctx.push(Role::System, "you are boris");
        for i in 0..5 {
            ctx.push(Role::User, format!("user-{i}"));
            ctx.push(Role::Assistant, format!("asst-{i}"));
        }

        assert!(!ctx.messages.is_empty());
        assert!(matches!(ctx.messages[0].role, Role::System));
        assert_eq!(text(&ctx.messages[0]), "you are boris");
        // Only the latest user turn remains after system.
        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant"]);
        assert_eq!(text(&ctx.messages[1]), "user-4");
    }

    #[test]
    fn prune_max_turns_zero_keeps_only_system() {
        let mut ctx = Context::new(0);
        ctx.push(Role::System, "sys");
        ctx.push(Role::User, "u1");
        ctx.push(Role::Assistant, "a1");
        assert_eq!(roles_of(&ctx), vec!["system"]);
    }

    #[test]
    fn load_history_forces_system_and_skips_history_system() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "old-sys");
        ctx.push(Role::User, "stale");

        let history = vec![
            Message {
                role: Role::System,
                content: json!("history-sys"),
            },
            Message {
                role: Role::User,
                content: json!("hello"),
            },
            Message {
                role: Role::Assistant,
                content: json!("hi"),
            },
        ];
        ctx.load_history("fresh-sys", history);

        assert_eq!(roles_of(&ctx), vec!["system", "user", "assistant"]);
        assert_eq!(text(&ctx.messages[0]), "fresh-sys");
        assert_eq!(text(&ctx.messages[1]), "hello");
        assert_eq!(text(&ctx.messages[2]), "hi");
    }

    #[test]
    fn messages_from_transcript_skips_unknown_roles() {
        let records = vec![
            ("user".into(), json!("u")),
            ("bogus".into(), json!("x")),
            ("assistant".into(), json!("a")),
            (
                "tool".into(),
                json!({ "tool_call_id": "c1", "content": "ok" }),
            ),
        ];
        let msgs = Context::messages_from_transcript(&records);
        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0].role, Role::User));
        assert!(matches!(msgs[1].role, Role::Assistant));
        assert!(matches!(msgs[2].role, Role::Tool));
    }

    #[test]
    fn role_from_str_roundtrip() {
        assert!(matches!(Role::from_str("user"), Ok(Role::User)));
        assert!(matches!(Role::from_str("assistant"), Ok(Role::Assistant)));
        assert!(matches!(Role::from_str("system"), Ok(Role::System)));
        assert!(matches!(Role::from_str("tool"), Ok(Role::Tool)));
        assert!(Role::from_str("nope").is_err());
    }

    #[test]
    fn dump_coerces_nested_assistant_content_to_string() {
        // Legacy compact stored a full message object as content; dump must not
        // double-wrap it (OpenRouter: messages.N.content Invalid input).
        let msg = Message {
            role: Role::Assistant,
            content: json!({
                "role": "assistant",
                "content": "[prior tool batch: 2 call(s) — details omitted]"
            }),
        };
        let dumped = msg.dump();
        assert_eq!(dumped["role"], "assistant");
        assert!(
            dumped["content"].is_string(),
            "content must be string, got {}",
            dumped["content"]
        );
        assert!(dumped["content"]
            .as_str()
            .unwrap()
            .contains("prior tool batch"));
    }

    #[test]
    fn dump_tool_calls_uses_empty_string_not_null() {
        let msg = Message {
            role: Role::Assistant,
            content: json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            }),
        };
        let dumped = msg.dump();
        assert_eq!(dumped["content"], json!(""));
        assert!(dumped.get("tool_calls").is_some());
    }

    #[test]
    fn compact_mechanical_collapses_old_tool_chains_to_plain_strings() {
        let mut ctx = Context::new(20);
        ctx.push(Role::System, "sys");

        // Turn 1: tool chain (will be compacted once we have >2 user turns)
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

        // Turns 2–3 keep the tool window closed so turn 1 collapses.
        ctx.push(Role::User, "u2");
        ctx.push(Role::Assistant, "a2");
        ctx.push(Role::User, "u3");
        ctx.push(Role::Assistant, "a3");

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

        // Orphan tool results from the collapsed window must be gone.
        assert!(
            !ctx.messages
                .iter()
                .any(|m| matches!(m.role, Role::Tool)),
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
}
