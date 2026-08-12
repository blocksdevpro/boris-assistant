//! Pure content-shape helpers for Grok chat_history wire format.

use serde_json::{json, Value};

/// Convert content to Grok content-block array when it is plain text.
pub(super) fn content_to_blocks(content: &Value) -> Value {
    match content {
        Value::String(s) => json!([{ "type": "text", "text": s }]),
        Value::Array(arr) => {
            if arr.first().and_then(|b| b.get("type")).is_some() {
                content.clone()
            } else {
                json!([{ "type": "text", "text": content.to_string() }])
            }
        }
        // Nested assistant/message object left over from compaction — unwrap text.
        Value::Object(map) if map.contains_key("content") && !map.contains_key("tool_calls") => {
            content_to_blocks(map.get("content").unwrap_or(&Value::Null))
        }
        other => json!([{ "type": "text", "text": other.to_string() }]),
    }
}

/// Flatten content to a plain string (system / tool_result / tool-call assistant).
pub(super) fn content_to_plain(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(arr) => {
            let mut texts = Vec::new();
            for b in arr {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        texts.push(t.to_string());
                    }
                }
            }
            if !texts.is_empty() {
                texts.join("\n")
            } else {
                content.to_string()
            }
        }
        Value::Object(map) if map.contains_key("content") => {
            content_to_plain(map.get("content").unwrap_or(&Value::Null))
        }
        other => other.to_string(),
    }
}

pub(super) fn flatten_content_value(content: Value) -> Value {
    match content {
        Value::String(s) => Value::String(s),
        Value::Null => Value::String(String::new()),
        Value::Array(_) => {
            let plain = content_to_plain(&content);
            Value::String(plain)
        }
        other => Value::String(content_to_plain(&other)),
    }
}

pub(super) fn blocks_to_content(content: Value) -> Value {
    match content {
        Value::Array(arr) => {
            let mut texts = Vec::new();
            let mut all_text = true;
            for b in &arr {
                if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        texts.push(t.to_string());
                        continue;
                    }
                }
                all_text = false;
                break;
            }
            if all_text && !texts.is_empty() {
                Value::String(texts.join("\n"))
            } else if all_text {
                Value::String(String::new())
            } else {
                Value::Array(arr)
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_to_blocks_string_wraps_text_block() {
        let v = content_to_blocks(&json!("hello"));
        assert_eq!(v, json!([{ "type": "text", "text": "hello" }]));
    }

    #[test]
    fn content_to_blocks_passes_through_typed_array() {
        let blocks = json!([{ "type": "text", "text": "a" }]);
        assert_eq!(content_to_blocks(&blocks), blocks);
    }

    #[test]
    fn content_to_blocks_unwraps_nested_message_object() {
        let nested = json!({ "role": "assistant", "content": "done" });
        assert_eq!(
            content_to_blocks(&nested),
            json!([{ "type": "text", "text": "done" }])
        );
    }

    #[test]
    fn content_to_plain_joins_text_blocks() {
        let blocks = json!([
            { "type": "text", "text": "a" },
            { "type": "text", "text": "b" }
        ]);
        assert_eq!(content_to_plain(&blocks), "a\nb");
    }

    #[test]
    fn content_to_plain_string_passthrough() {
        assert_eq!(content_to_plain(&json!("x")), "x");
        assert_eq!(content_to_plain(&Value::Null), "");
    }

    #[test]
    fn blocks_to_content_collapses_text_array() {
        let blocks = json!([
            { "type": "text", "text": "hello" },
            { "type": "text", "text": "world" }
        ]);
        assert_eq!(
            blocks_to_content(blocks),
            Value::String("hello\nworld".into())
        );
    }

    #[test]
    fn blocks_to_content_keeps_mixed_array() {
        let mixed = json!([
            { "type": "text", "text": "a" },
            { "type": "image", "url": "x" }
        ]);
        let out = blocks_to_content(mixed.clone());
        assert!(out.is_array());
        assert_eq!(out, mixed);
    }

    #[test]
    fn flatten_content_value_array_to_string() {
        let blocks = json!([{ "type": "text", "text": "hi" }]);
        assert_eq!(flatten_content_value(blocks), Value::String("hi".into()));
    }
}
