//! Wire-facing chat message + OpenAI/OpenRouter serialization.

use serde_json::{json, Value};

use super::Role;

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    /// For User/System/Assistant: a plain string or content array.
    /// For Tool: a JSON object `{ tool_call_id, content }`.
    /// For Assistant with tool_calls: the raw message object from the LLM.
    pub content: Value,
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
    fn dump_tool_surfaces_tool_call_id() {
        let msg = Message {
            role: Role::Tool,
            content: json!({ "tool_call_id": "call_9", "content": "ok" }),
        };
        let dumped = msg.dump();
        assert_eq!(dumped["role"], "tool");
        assert_eq!(dumped["tool_call_id"], "call_9");
        assert_eq!(dumped["content"], "ok");
    }
}
