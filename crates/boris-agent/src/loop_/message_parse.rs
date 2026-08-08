//! Parse assistant message payloads into loop-friendly forms.
//!
//! Keeps OpenAI-style JSON shape handling out of the ReAct control flow.

use serde_json::{json, Value};

use crate::runtime::RawToolCall;

/// Parse OpenAI-style `tool_calls` array entries into [`RawToolCall`]s.
///
/// Missing fields become empty strings / empty objects; malformed `arguments`
/// JSON falls back to `{}` (same as the historical loop behaviour).
pub(super) fn parse_raw_tool_calls(calls: &[Value]) -> Vec<RawToolCall> {
    calls.iter().map(parse_one_tool_call).collect()
}

fn parse_one_tool_call(call: &Value) -> RawToolCall {
    let call_id = call["id"].as_str().unwrap_or("").to_string();
    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
    let args: Value = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))
        .unwrap_or_else(|_| json!({}));
    RawToolCall {
        call_id,
        name,
        args,
    }
}

/// Pull speakable text from an assistant message (`content` string or parts array).
pub(super) fn extract_reply_text(response: &Value) -> String {
    match response.get("content") {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                if let Some(s) = p.as_str() {
                    out.push_str(s);
                } else if let Some(s) = p.get("text").and_then(|t| t.as_str()) {
                    out.push_str(s);
                }
            }
            out.trim().to_string()
        }
        _ => String::new(),
    }
}

/// Truncate for event previews (Unicode-char-aware, not byte-sliced).
pub(super) fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_tool_calls_reads_id_name_and_args() {
        let calls = vec![json!({
            "id": "c1",
            "type": "function",
            "function": { "name": "echo", "arguments": "{\"x\":1}" }
        })];
        let parsed = parse_raw_tool_calls(&calls);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].call_id, "c1");
        assert_eq!(parsed[0].name, "echo");
        assert_eq!(parsed[0].args, json!({"x": 1}));
    }

    #[test]
    fn parse_tool_calls_defaults_missing_and_bad_args() {
        let calls = vec![
            json!({}),
            json!({
                "id": "c2",
                "function": { "name": "x", "arguments": "not-json" }
            }),
        ];
        let parsed = parse_raw_tool_calls(&calls);
        assert_eq!(parsed[0].call_id, "");
        assert_eq!(parsed[0].name, "");
        assert_eq!(parsed[0].args, json!({}));
        assert_eq!(parsed[1].args, json!({}));
    }

    #[test]
    fn extract_reply_from_string_content() {
        let r = json!({ "role": "assistant", "content": "  Hello  " });
        assert_eq!(extract_reply_text(&r), "Hello");
    }

    #[test]
    fn extract_reply_from_parts_array() {
        let r = json!({
            "content": [
                "Hi ",
                { "type": "text", "text": "there" },
                42
            ]
        });
        assert_eq!(extract_reply_text(&r), "Hi there");
    }

    #[test]
    fn extract_reply_empty_when_missing_or_null() {
        assert_eq!(extract_reply_text(&json!({})), "");
        assert_eq!(extract_reply_text(&json!({ "content": null })), "");
    }

    #[test]
    fn log_preview_truncates_with_ellipsis() {
        assert_eq!(log_preview("hello", 10), "hello");
        assert_eq!(log_preview("hello world", 5), "hello…");
        // multi-byte safety
        assert_eq!(log_preview("你好世界", 2), "你好…");
    }
}
