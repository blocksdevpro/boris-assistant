//! Pure OpenAI ↔ Grok tool_calls shape conversion.

use serde_json::{json, Value};

/// Pull tool_calls from an assistant content value (raw LLM object or already flat).
pub(super) fn extract_tool_calls(content: &Value) -> Option<Value> {
    let calls = content.get("tool_calls")?;
    let arr = calls.as_array()?;
    if arr.is_empty() {
        return None;
    }
    Some(grok_tool_calls_from_openai(calls.clone()))
}

/// OpenAI → Grok flat tool_calls: `{id, name, arguments}`.
pub(super) fn grok_tool_calls_from_openai(calls: Value) -> Value {
    let Value::Array(arr) = calls else {
        return calls;
    };
    let mapped: Vec<Value> = arr
        .into_iter()
        .map(|c| {
            if c.get("name").is_some() && c.get("function").is_none() {
                // Already flat (Grok / Boris disk).
                return c;
            }
            let id = c.get("id").cloned().unwrap_or(Value::String(String::new()));
            let name = c
                .pointer("/function/name")
                .or_else(|| c.get("name"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let arguments = c
                .pointer("/function/arguments")
                .cloned()
                .or_else(|| {
                    c.get("arguments").map(|a| match a {
                        Value::String(s) => Value::String(s.clone()),
                        other => Value::String(other.to_string()),
                    })
                })
                .unwrap_or(Value::String("{}".into()));
            json!({
                "id": id,
                "name": name,
                "arguments": arguments,
            })
        })
        .collect();
    Value::Array(mapped)
}

/// Disk (flat or OpenAI) → OpenAI `tool_calls` for the agent context.
pub(super) fn openai_tool_calls_from_disk(calls: Value) -> Value {
    let Value::Array(arr) = calls else {
        return calls;
    };
    let mapped: Vec<Value> = arr
        .into_iter()
        .map(|c| {
            if c.get("function").is_some() {
                return c;
            }
            let id = c.get("id").cloned().unwrap_or(Value::String(String::new()));
            let name = c
                .get("name")
                .cloned()
                .unwrap_or(Value::String(String::new()));
            let arguments = match c.get("arguments") {
                Some(Value::String(s)) => Value::String(s.clone()),
                Some(other) => Value::String(other.to_string()),
                None => Value::String("{}".into()),
            };
            json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": arguments,
                }
            })
        })
        .collect();
    Value::Array(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_to_grok_flattens_function_object() {
        let openai = json!([{
            "id": "call_1",
            "type": "function",
            "function": {
                "name": "read_file",
                "arguments": "{\"path\":\"a.txt\"}"
            }
        }]);
        let flat = grok_tool_calls_from_openai(openai);
        assert_eq!(flat[0]["name"], "read_file");
        assert_eq!(flat[0]["id"], "call_1");
        assert_eq!(flat[0]["arguments"], "{\"path\":\"a.txt\"}");
        assert!(flat[0].get("function").is_none());
    }

    #[test]
    fn grok_flat_stays_flat() {
        let already = json!([{
            "id": "c1",
            "name": "bash",
            "arguments": "{}"
        }]);
        let out = grok_tool_calls_from_openai(already.clone());
        assert_eq!(out, already);
    }

    #[test]
    fn disk_flat_to_openai_nested() {
        let flat = json!([{
            "id": "c1",
            "name": "bash",
            "arguments": "{}"
        }]);
        let nested = openai_tool_calls_from_disk(flat);
        assert_eq!(nested[0]["function"]["name"], "bash");
        assert_eq!(nested[0]["type"], "function");
        assert_eq!(nested[0]["id"], "c1");
    }

    #[test]
    fn openai_on_disk_passes_through() {
        let openai = json!([{
            "id": "c1",
            "type": "function",
            "function": { "name": "x", "arguments": "{}" }
        }]);
        let out = openai_tool_calls_from_disk(openai.clone());
        assert_eq!(out, openai);
    }

    #[test]
    fn extract_tool_calls_none_when_empty_or_missing() {
        assert!(extract_tool_calls(&json!({"content": ""})).is_none());
        assert!(extract_tool_calls(&json!({"tool_calls": []})).is_none());
    }

    #[test]
    fn extract_tool_calls_returns_flat_when_present() {
        let content = json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "c1",
                "type": "function",
                "function": { "name": "bash", "arguments": "{}" }
            }]
        });
        let extracted = extract_tool_calls(&content).expect("calls");
        assert_eq!(extracted[0]["name"], "bash");
    }
}
