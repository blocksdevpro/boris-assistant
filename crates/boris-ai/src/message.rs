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
    if has_usable_tool_calls(msg) {
        return true;
    }
    !extract_text_content(msg.get("content").unwrap_or(&Value::Null)).is_empty()
}

/// Whether `tool_calls` is a non-empty array.
///
/// This is a purely structural check (length only). It does NOT verify that
/// the entries are executable tool calls — see [`has_usable_tool_calls`].
pub fn has_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty())
}

/// Whether at least one `tool_calls` entry names a function to call.
///
/// Some providers stream placeholder entries such as `[{"id": "1"}]` with no
/// `function.name`; those are not executable and must not count as a usable
/// payload (otherwise an empty stream would look successful and skip the
/// blocking fallback). Valid entries with a non-empty `function.name` still
/// count, even when `arguments` is empty or missing.
pub fn has_usable_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|calls| {
            calls.iter().any(|tc| {
                tc.get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| !n.is_empty())
            })
        })
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
        // Object-form content such as `{ "type": "text", "text": "..." }`
        // (non-array) carries speakable text in its `text` field.
        Value::Object(_) => {
            let mut out = String::new();
            append_content_part(&mut out, v);
            out.trim().to_string()
        }
        // Bool / Number / Null have no speakable text representation.
        _ => String::new(),
    }
}

/// Append one multimodal content part.
///
/// Handles plain strings and objects with a `text` field (including
/// object-form `{ "type": "text", "text": "..." }` when it appears outside an
/// array, via [`extract_text_content`]).
///
/// Image parts (`image_url`), `refusal` strings, and other non-text shapes are
/// intentionally dropped: there is no speakable text representation for them.
/// (debug builds: enable `BORIS_LOG=debug` tracing at call sites if a drop is
/// ever suspected of losing user-visible text.)
fn append_content_part(out: &mut String, part: &Value) {
    if let Some(s) = part.as_str() {
        out.push_str(s);
        return;
    }
    if let Some(s) = part.get("text").and_then(|t| t.as_str()) {
        out.push_str(s);
    }
    // Anything else (image_url / refusal / numbers / bools) has no speakable
    // text and is deliberately skipped.
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
            // The LlmClient contract wants `content` preferably a string, so a
            // missing key is inserted as "" rather than left missing.
            obj.insert("content".into(), Value::String(String::new()));
        }
    }
    message
}

/// Normalize one outbound chat message (clone; never mutates the input).
///
/// - missing or `null` `content` → `""`
/// - array `content` (multimodal parts) → flattened text via
///   [`extract_text_content`]
/// - object `content` with a `text` field → extracted text
/// - string `content` → kept as-is
///
/// NOTE for Group2 / future wiring: `request.rs` SHOULD call
/// [`normalize_outbound_messages`] on the `messages` array when building the
/// request body so providers that reject `content: null` never see one.
/// Apply selectively (assistant messages at minimum): flattening an array
/// drops non-text parts such as user image blocks, which is only acceptable
/// when the target provider cannot consume them.
#[allow(dead_code)] // wired by Group2's request.rs pass; covered by unit tests.
pub(crate) fn normalize_outbound_message(v: &Value) -> Value {
    let mut out = v.clone();
    let Some(obj) = out.as_object_mut() else {
        return out;
    };
    match obj.get("content").cloned() {
        None | Some(Value::Null) => {
            obj.insert("content".into(), Value::String(String::new()));
        }
        Some(c @ Value::Array(_)) | Some(c @ Value::Object(_)) => {
            let text = extract_text_content(&c);
            obj.insert("content".into(), Value::String(text));
        }
        Some(_) => {}
    }
    out
}

/// Normalize every message in an outbound `messages` array.
///
/// Non-array input is returned cloned and unchanged. See
/// [`normalize_outbound_message`] for per-message rules and the note about
/// `request.rs` wiring.
#[allow(dead_code)] // wired by Group2's request.rs pass; covered by unit tests.
pub fn normalize_outbound_messages(messages: &Value) -> Value {
    match messages.as_array() {
        Some(items) => Value::Array(items.iter().map(normalize_outbound_message).collect()),
        None => messages.clone(),
    }
}

/// Build a final assistant message from assembled stream pieces.
///
/// An empty `tool_calls` list is always omitted (never emitted as
/// `"tool_calls": []`) so stream-assembled messages match the blocking path,
/// which strips empty arrays after [`normalize_assistant_message`].
pub fn assistant_message_from_stream(role: &str, content: String, tool_calls: Vec<Value>) -> Value {
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
        // Valid tool calls (non-empty function.name) count as usable.
        assert!(message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [{
                "id": "1",
                "function": { "name": "get_time", "arguments": "{}" }
            }]
        })));
        assert!(message_has_usable_payload(&json!({ "content": "hi" })));
        assert!(!message_has_usable_payload(&json!({ "content": "  " })));
        assert!(!message_has_usable_payload(&json!({ "content": null })));
    }

    #[test]
    fn nameless_tool_placeholders_are_not_usable() {
        // Placeholder entries without function.name must not count.
        assert!(!message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [{ "id": "1" }]
        })));
        assert!(!message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [{ "function": {} }]
        })));
        assert!(!message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [{ "function": { "name": "" } }]
        })));
        // One named call among placeholders is enough.
        assert!(message_has_usable_payload(&json!({
            "content": "",
            "tool_calls": [
                { "id": "1" },
                { "id": "2", "function": { "name": "bash" } }
            ]
        })));
    }

    #[test]
    fn has_usable_tool_calls_unit() {
        assert!(has_usable_tool_calls(&json!({
            "tool_calls": [{ "function": { "name": "x" } }]
        })));
        // Empty arguments are fine; the name is what matters.
        assert!(has_usable_tool_calls(&json!({
            "tool_calls": [{ "id": "a", "function": { "name": "x" } }]
        })));
        assert!(!has_usable_tool_calls(&json!({ "tool_calls": [] })));
        assert!(!has_usable_tool_calls(&json!({ "content": "hi" })));
        // Legacy length-only check still sees the placeholder array.
        assert!(has_tool_calls(&json!({ "tool_calls": [{ "id": "1" }] })));
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

        // Missing content is inserted as "" per the LlmClient contract.
        let msg = normalize_assistant_message(json!({ "role": "assistant" }));
        assert_eq!(msg["content"], "");

        // Object-form content extracts its text.
        let msg = normalize_assistant_message(json!({
            "role": "assistant",
            "content": { "type": "text", "text": "obj hi" }
        }));
        assert_eq!(msg["content"], "obj hi");
    }

    #[test]
    fn extract_object_form_content() {
        assert_eq!(
            extract_text_content(&json!({ "type": "text", "text": " hi " })),
            "hi"
        );
        // Image / refusal shapes still yield no text.
        assert!(extract_text_content(&json!({ "type": "image_url" })).is_empty());
        assert!(extract_text_content(&json!({ "refusal": "no" })).is_empty());
    }

    #[test]
    fn outbound_normalization_shapes() {
        // Null / missing -> "".
        assert_eq!(
            normalize_outbound_message(&json!({ "role": "a", "content": null }))["content"],
            ""
        );
        assert_eq!(
            normalize_outbound_message(&json!({ "role": "a" }))["content"],
            ""
        );
        // Array parts -> flattened text.
        assert_eq!(
            normalize_outbound_message(&json!({
                "role": "a",
                "content": [
                    { "type": "text", "text": "hello " },
                    { "type": "text", "text": "world" }
                ]
            }))["content"],
            "hello world"
        );
        // Object with text -> extracted.
        assert_eq!(
            normalize_outbound_message(&json!({
                "role": "a",
                "content": { "type": "text", "text": "solo" }
            }))["content"],
            "solo"
        );
        // Strings untouched; non-objects cloned as-is.
        let msg = json!({ "role": "a", "content": "keep" });
        assert_eq!(normalize_outbound_message(&msg)["content"], "keep");
        assert_eq!(normalize_outbound_message(&json!("raw")), json!("raw"));
        // Input is not mutated.
        let input = json!({ "role": "a", "content": null });
        let _ = normalize_outbound_message(&input);
        assert!(input["content"].is_null());
    }

    #[test]
    fn outbound_messages_maps_arrays() {
        let out = normalize_outbound_messages(&json!([
            { "role": "a", "content": null },
            { "role": "b", "content": "hi" }
        ]));
        assert_eq!(out[0]["content"], "");
        assert_eq!(out[1]["content"], "hi");
        // Non-array input passes through unchanged.
        let obj = json!({ "role": "a" });
        assert_eq!(normalize_outbound_messages(&obj), obj);
    }

    #[test]
    fn stream_message_omits_empty_tool_calls() {
        let msg = assistant_message_from_stream("assistant", "hi".into(), vec![]);
        assert!(msg.get("tool_calls").is_none());
        let msg = assistant_message_from_stream(
            "assistant",
            String::new(),
            vec![json!({ "id": "1" })],
        );
        assert!(msg.get("tool_calls").is_some());
    }

    #[test]
    fn stream_delta_preserves_spaces() {
        let mut s = String::new();
        append_stream_content_delta(&mut s, &json!("Hello "));
        append_stream_content_delta(&mut s, &json!("world"));
        assert_eq!(s, "Hello world");
    }
}
