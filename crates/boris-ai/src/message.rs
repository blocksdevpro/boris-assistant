//! Helpers for OpenAI-compatible assistant `message` objects.
//!
//! Providers disagree on `content` shape (plain string vs multimodal parts).
//! The agent loop expects a **string** `content` field and optional `tool_calls`.

use serde_json::{json, Value};

/// True when an assistant message has speakable text and/or tool calls.
///
/// Used after streaming: some OpenRouter/Gemini paths return HTTP 200 with
/// thinking-only / empty assembly — treat that as failure and fall back to
/// the non-stream path so the voice agent does not go silent.
pub fn message_has_usable_payload(msg: &Value) -> bool {
    if has_tool_calls(msg) {
        return true;
    }
    !extract_text_content(msg.get("content").unwrap_or(&Value::Null)).is_empty()
}

/// Whether `tool_calls` is a non-empty array.
pub fn has_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty())
}

/// Pull plain text from OpenAI-style `content` (string or multimodal parts).
///
/// Trims the final result. For **streaming** deltas that are plain strings,
/// prefer appending the raw `as_str()` value so intentional spaces are kept.
pub fn extract_text_content(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => {
            let mut out = String::new();
            for part in parts {
                append_content_part(&mut out, part);
            }
            out.trim().to_string()
        }
        // Object / Bool / Number / Null all have no speakable text representation.
        _ => String::new(),
    }
}

/// Append one multimodal content part.
///
/// Handles plain strings and objects with a `text` field (including
/// `{ "type": "text", "text": "..." }`).
fn append_content_part(out: &mut String, part: &Value) {
    if let Some(s) = part.as_str() {
        out.push_str(s);
        return;
    }
    if let Some(s) = part.get("text").and_then(|t| t.as_str()) {
        out.push_str(s);
    }
}

/// Append streaming `content` delta into `sink`.
///
/// Plain strings are appended raw (preserve spaces). Multimodal parts use
/// [`extract_text_content`].
pub fn append_stream_content_delta(sink: &mut String, content: &Value) {
    if let Some(s) = content.as_str() {
        sink.push_str(s);
        return;
    }
    let chunk = extract_text_content(content);
    if !chunk.is_empty() {
        sink.push_str(&chunk);
    }
}

/// Normalize `message.content` to a JSON string (never null / parts array).
///
/// Several OpenRouter providers reject `content: null` on assistant messages
/// (including tool-call turns). The agent loop also reads `.as_str()`.
pub fn normalize_assistant_message(mut message: Value) -> Value {
    let Some(obj) = message.as_object_mut() else {
        return message;
    };
    match obj.get("content").cloned() {
        Some(c) if c.is_string() => {}
        Some(c) if c.is_null() => {
            obj.insert("content".into(), Value::String(String::new()));
        }
        Some(c) => {
            let text = extract_text_content(&c);
            obj.insert("content".into(), Value::String(text));
        }
        None => {
            // Leave missing content alone; callers may insert explicitly.
        }
    }
    message
}

/// Build a final assistant message from assembled stream pieces.
pub fn assistant_message_from_stream(
    role: &str,
    content: String,
    tool_calls: Vec<Value>,
) -> Value {
    let mut message = json!({
        "role": role,
        "content": content,
    });
    if !tool_calls.is_empty() {
        if let Some(obj) = message.as_object_mut() {
            obj.insert("tool_calls".into(), Value::Array(tool_calls));
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_string_and_parts() {
        assert_eq!(extract_text_content(&json!("  hi  ")), "hi");
        assert_eq!(
            extract_text_content(&json!([
                { "type": "text", "text": "hello " },
                { "text": "world" }
            ])),
            "hello world"
        );
        assert!(extract_text_content(&Value::Null).is_empty());
        assert!(extract_text_content(&json!(42)).is_empty());
    }

    #[test]
    fn usable_payload_tools_or_text() {
        assert!(message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [{ "id": "1" }]
        })));
        assert!(message_has_usable_payload(&json!({ "content": "hi" })));
        assert!(!message_has_usable_payload(&json!({ "content": "  " })));
        assert!(!message_has_usable_payload(&json!({ "content": null })));
    }

    #[test]
    fn normalize_parts_to_string() {
        let msg = normalize_assistant_message(json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": "ok" }]
        }));
        assert_eq!(msg["content"], "ok");

        let msg = normalize_assistant_message(json!({
            "role": "assistant",
            "content": null
        }));
        assert_eq!(msg["content"], "");
    }

    #[test]
    fn stream_delta_preserves_spaces() {
        let mut s = String::new();
        append_stream_content_delta(&mut s, &json!("Hello "));
        append_stream_content_delta(&mut s, &json!("world"));
        assert_eq!(s, "Hello world");
    }
}
