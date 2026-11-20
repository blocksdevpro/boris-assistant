//! Wire-facing chat message + OpenAI/OpenRouter serialization.

use serde_json::{json, Value};

use super::Role;

/// Provenance of a context message, independent of its provider wire role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageOrigin {
    System,
    Human,
    Assistant,
    Tool,
    /// Harness instruction encoded as a user-role message for provider compatibility.
    HostControl,
    /// LLM-written compacted history; context, but never a human turn.
    Summary,
    /// Host-compacted, untrusted observations from a removed tool-call batch.
    CompactedTool,
    /// Host-loaded skill/playbook instruction.
    Skill,
    /// User-derived personal memory injected as untrusted reference data.
    PersonalContext,
    /// Host-maintained structured state for the active task.
    TaskState,
    /// Search result injected proactively from durable memory.
    RetrievedMemory,
    /// Image a tool attached (screen capture, image file), delivered as a
    /// user-role message after the tool batch. Ephemeral: never persisted and
    /// replaced by `[image omitted]` text at the next human turn.
    ToolImage,
    /// Merged host-derived reference block (personal + skills + memory + task).
    ///
    /// Single `Role::User` message inserted after the system prompt. Wire role
    /// stays `user` so provider shape is unchanged; origin is only harness
    /// provenance and is never persisted as history.
    DerivedContext,
}

impl MessageOrigin {
    pub fn for_role(role: Role) -> Self {
        match role {
            Role::System => Self::System,
            Role::User => Self::Human,
            Role::Assistant => Self::Assistant,
            Role::Tool => Self::Tool,
        }
    }

    pub fn is_human(self) -> bool {
        matches!(self, Self::Human)
    }

    /// Ephemeral host controls should never become transcript history.
    pub fn should_persist(self) -> bool {
        self.is_history_event()
    }

    /// Canonical append-only transcript events. Everything else is a derived
    /// model aid and may be regenerated, replaced, or discarded safely.
    pub fn is_history_event(self) -> bool {
        matches!(
            self,
            Self::System | Self::Human | Self::Assistant | Self::Tool
        )
    }
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub origin: MessageOrigin,
    /// For User/System/Assistant: a plain string or content array.
    /// For Tool: a JSON object `{ tool_call_id, content }`.
    /// For Assistant with tool_calls: the raw message object from the LLM.
    pub content: Value,
    /// Wall-clock send time (Unix ms) of a human message, when the host knows it.
    ///
    /// Host metadata, never part of `content`: it is rendered as a leading
    /// `<sent_at>` tag only on the provider wire ([`Message::dump`]) and
    /// persisted beside the content ([`Message::persist_pair`]).
    pub sent_at_ms: Option<u64>,
}

/// Host-only key carrying a human message's send time inside a persisted
/// `(role, content)` pair: `{ "content": …, "_sent_at_ms": u64 }`.
pub const SENT_AT_KEY: &str = "_sent_at_ms";

const SENT_AT_OPEN: &str = "<sent_at>";
const SENT_AT_CLOSE: &str = "</sent_at>";

impl Message {
    pub fn new(role: Role, content: impl Into<Value>) -> Self {
        Self {
            origin: MessageOrigin::for_role(role.clone()),
            role,
            content: content.into(),
            sent_at_ms: None,
        }
    }

    pub fn with_origin(role: Role, origin: MessageOrigin, content: impl Into<Value>) -> Self {
        Self {
            role,
            origin,
            content: content.into(),
            sent_at_ms: None,
        }
    }

    /// A real human turn stamped with the time it was sent.
    pub fn human_at(content: impl Into<Value>, sent_at_ms: u64) -> Self {
        Self {
            sent_at_ms: Some(sent_at_ms),
            ..Self::new(Role::User, content)
        }
    }

    /// Send time to render on the wire: human turns only.
    fn wire_sent_at(&self) -> Option<u64> {
        if self.origin == MessageOrigin::Human && matches!(self.role, Role::User) {
            self.sent_at_ms
        } else {
            None
        }
    }

    /// `(role, content)` for session persistence. A stamped human message
    /// wraps its content as `{ "content": …, "_sent_at_ms": … }` so the send
    /// time survives transcript rewrites; [`split_sent_at`] reverses it.
    pub fn persist_pair(&self) -> (String, Value) {
        let content = match self.wire_sent_at() {
            Some(ms) => json!({ "content": self.content.clone(), SENT_AT_KEY: ms }),
            None => self.content.clone(),
        };
        (self.role.to_string(), content)
    }
}

/// Unwrap a persisted `{ "content", "_sent_at_ms" }` pair body. Anything else
/// is returned unchanged with no time.
pub fn split_sent_at(content: &Value) -> (Value, Option<u64>) {
    if let Some(obj) = content.as_object() {
        if let (Some(ms), Some(inner)) = (
            obj.get(SENT_AT_KEY).and_then(Value::as_u64),
            obj.get("content"),
        ) {
            return (inner.clone(), Some(ms));
        }
    }
    (content.clone(), None)
}

/// Local wall-clock label for a send time, e.g. `Thu 2026-11-19 10:58`.
fn sent_at_label(ms: u64) -> Option<String> {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(i64::try_from(ms).ok()?) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            Some(dt.format("%a %Y-%m-%d %H:%M").to_string())
        }
        chrono::LocalResult::None => None,
    }
}

/// Prefix wire content with `<sent_at>…</sent_at>` (text or content parts).
fn stamp_content(content: Value, ms: u64) -> Value {
    let Some(label) = sent_at_label(ms) else {
        return content;
    };
    let tag = format!("{SENT_AT_OPEN}{label}{SENT_AT_CLOSE}");
    match content {
        Value::String(text) => Value::String(format!("{tag}\n{text}")),
        Value::Array(mut parts) => {
            parts.insert(0, json!({ "type": "text", "text": tag }));
            Value::Array(parts)
        }
        other => other,
    }
}

/// Remove any `<sent_at>…</sent_at>` a model copied into its own reply so the
/// host stamp is never spoken or persisted as assistant text.
pub fn strip_sent_at_tags(text: &str) -> String {
    if !text.contains(SENT_AT_OPEN) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(SENT_AT_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + SENT_AT_OPEN.len()..];
        rest = match after.find(SENT_AT_CLOSE) {
            Some(end) => &after[end + SENT_AT_CLOSE.len()..],
            None => after,
        };
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Raw provider reasoning blocks on an assistant message (OpenRouter
/// "preserving reasoning blocks"). They are returned unmodified on later
/// requests of the same human turn, then dropped: never persisted, digested,
/// or spoken.
pub(crate) const REASONING_DETAILS_KEY: &str = "reasoning_details";

impl Message {
    /// Remove raw reasoning blocks; returns true when any were present.
    pub(crate) fn strip_reasoning_details(&mut self) -> bool {
        matches!(self.role, Role::Assistant)
            && self
                .content
                .as_object_mut()
                .is_some_and(|obj| obj.remove(REASONING_DETAILS_KEY).is_some())
    }

    /// Copy without raw reasoning blocks (persistence, summaries).
    pub(crate) fn without_reasoning_details(&self) -> Message {
        let mut message = self.clone();
        message.strip_reasoning_details();
        message
    }
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
                let content =
                    coerce_message_content(self.content.get("content").unwrap_or(&Value::Null));
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
            _ => {
                let mut content = coerce_message_content(&self.content);
                if let Some(ms) = self.wire_sent_at() {
                    content = stamp_content(content, ms);
                }
                json!({
                    "role":    self.role.to_string(),
                    "content": content,
                })
            }
        }
    }
}

/// Coerce stored message content into a wire-safe OpenAI `content` value.
///
/// - strings stay strings
/// - null / missing → `""`
/// - nested `{ "role", "content" }` (legacy compact bug) → unwrap inner content
/// - other JSON → stringified (tools always need a string body)
pub(super) fn coerce_message_content(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(s.clone()),
        Value::Null => Value::String(String::new()),
        // Multimodal content parts — pass through when well-formed.
        Value::Array(parts) if !parts.is_empty() => {
            let looks_like_parts = parts
                .iter()
                .all(|p| p.get("type").and_then(|t| t.as_str()).is_some());
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coerce_string_and_null() {
        assert_eq!(coerce_message_content(&json!("hi")), json!("hi"));
        assert_eq!(coerce_message_content(&Value::Null), json!(""));
    }

    #[test]
    fn coerce_unwraps_nested_message_object() {
        let nested = json!({
            "role": "assistant",
            "content": "inner text"
        });
        assert_eq!(coerce_message_content(&nested), json!("inner text"));
    }

    #[test]
    fn coerce_passes_through_content_parts() {
        let parts = json!([
            { "type": "text", "text": "hello" },
            { "type": "image_url", "image_url": { "url": "x" } }
        ]);
        assert_eq!(coerce_message_content(&parts), parts);
    }

    #[test]
    fn coerce_stringifies_non_parts_array() {
        let arr = json!([1, 2, 3]);
        let out = coerce_message_content(&arr);
        assert!(out.is_string());
    }

    #[test]
    fn dump_coerces_nested_assistant_content_to_string() {
        // Legacy compact stored a full message object as content; dump must not
        // double-wrap it (OpenRouter: messages.N.content Invalid input).
        let msg = Message {
            role: Role::Assistant,
            origin: MessageOrigin::Assistant,
            content: json!({
                "role": "assistant",
                "content": "[prior tool batch: 2 call(s) — details omitted]"
            }),
            sent_at_ms: None,
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
            origin: MessageOrigin::Assistant,
            content: json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            }),
            sent_at_ms: None,
        };
        let dumped = msg.dump();
        assert_eq!(dumped["content"], json!(""));
        assert!(dumped.get("tool_calls").is_some());
    }

    #[test]
    fn dump_tool_surfaces_tool_call_id() {
        let msg = Message {
            role: Role::Tool,
            origin: MessageOrigin::Tool,
            content: json!({ "tool_call_id": "call_9", "content": "ok" }),
            sent_at_ms: None,
        };
        let dumped = msg.dump();
        assert_eq!(dumped["role"], "tool");
        assert_eq!(dumped["tool_call_id"], "call_9");
        assert_eq!(dumped["content"], "ok");
    }

    #[test]
    fn stamped_human_dump_prefixes_sent_at_and_keeps_content_clean() {
        let msg = Message::human_at("what are you doing?", 1_700_000_000_000);
        let dumped = msg.dump();
        let text = dumped["content"].as_str().unwrap();
        assert!(text.starts_with("<sent_at>"), "{text}");
        assert!(text.ends_with("</sent_at>\nwhat are you doing?"), "{text}");
        assert_eq!(msg.content, json!("what are you doing?"));
    }

    #[test]
    fn unstamped_and_non_human_dump_unchanged() {
        assert_eq!(Message::new(Role::User, "hi").dump()["content"], json!("hi"));
        let mut control = Message::with_origin(Role::User, MessageOrigin::HostControl, "note");
        control.sent_at_ms = Some(1_700_000_000_000);
        assert_eq!(control.dump()["content"], json!("note"));
    }

    #[test]
    fn stamped_parts_get_leading_text_part() {
        let msg = Message::human_at(json!([{ "type": "text", "text": "hi" }]), 1_700_000_000_000);
        let parts = msg.dump()["content"].clone();
        assert!(parts[0]["text"].as_str().unwrap().starts_with("<sent_at>"));
        assert_eq!(parts[1]["text"], "hi");
    }

    #[test]
    fn persist_pair_roundtrips_sent_at() {
        let msg = Message::human_at("stop", 1_700_000_000_000);
        let (role, body) = msg.persist_pair();
        assert_eq!(role, "user");
        assert_eq!(split_sent_at(&body), (json!("stop"), Some(1_700_000_000_000)));
        let plain = Message::new(Role::Assistant, "ok").persist_pair().1;
        assert_eq!(split_sent_at(&plain), (json!("ok"), None));
    }

    #[test]
    fn strip_sent_at_tags_removes_echoed_stamp() {
        assert_eq!(
            strip_sent_at_tags("<sent_at>Thu 2026-11-19 10:58</sent_at>\nNothing right now."),
            "Nothing right now."
        );
        assert_eq!(strip_sent_at_tags("plain"), "plain");
    }
}
