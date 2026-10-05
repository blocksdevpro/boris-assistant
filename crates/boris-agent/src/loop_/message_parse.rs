//! Parse assistant message payloads into loop-friendly forms.
//!
//! Keeps OpenAI-style JSON shape handling out of the ReAct control flow.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::runtime::RawToolCall;
use crate::tool::Tool;

static PARALLEL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Normalize names and expand a model's explicit parallel envelope into
/// individually paired calls before dispatch or context insertion. Invalid
/// envelopes stay intact so the runtime returns a repairable error.
/// Raw provider responses remain in debug.
pub(super) fn normalize_tool_calls(
    mut response: Value,
    tools: &[std::sync::Arc<dyn Tool>],
) -> (Value, Vec<Value>) {
    boris_ai::normalize_tool_call_names(&mut response);
    if !tools
        .iter()
        .any(|tool| tool.name() == crate::tools::parallel::NAME)
    {
        return (response, Vec::new());
    }
    let Some(calls) = response.get_mut("tool_calls").and_then(Value::as_array_mut) else {
        return (response, Vec::new());
    };
    if !calls
        .iter()
        .any(|call| call["function"]["name"].as_str() == Some(crate::tools::parallel::NAME))
    {
        return (response, Vec::new());
    }
    let mut ids: HashSet<String> = calls
        .iter()
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect();
    let mut expanded = Vec::new();
    let mut mappings = Vec::new();
    for call in std::mem::take(calls) {
        if call["function"]["name"].as_str() != Some(crate::tools::parallel::NAME) {
            expanded.push(call);
            continue;
        }
        let raw = parse_one_tool_call(&call);
        let Ok(children) = crate::tools::parallel::parse_tool_uses(raw.args) else {
            expanded.push(call);
            continue;
        };
        let mut mapping = Vec::new();
        for child in children {
            let id = loop {
                let seq = PARALLEL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let id = format!("call_parallel_{stamp:x}_{seq:x}");
                if ids.insert(id.clone()) {
                    break id;
                }
            };
            mapping.push(json!({"call_id": id, "tool_name": child.recipient_name}));
            expanded.push(json!({
                "id": id, "type": "function",
                "function": {"name": child.recipient_name, "arguments": Value::Object(child.parameters).to_string()}
            }));
        }
        mappings.push(json!({"parent_call_id": raw.call_id, "children": mapping}));
    }
    *calls = expanded;
    (response, mappings)
}

/// Parse OpenAI-style `tool_calls` array entries into [`RawToolCall`]s.
///
/// Malformed `arguments` JSON is **not** coerced to `{}`. The raw string is
/// kept so the runtime can return a typed invalid-arguments observation.
pub(super) fn parse_raw_tool_calls(calls: &[Value]) -> Vec<RawToolCall> {
    calls.iter().map(parse_one_tool_call).collect()
}

fn parse_one_tool_call(call: &Value) -> RawToolCall {
    let call_id = call["id"].as_str().unwrap_or("").to_string();
    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
    let raw = call["function"]["arguments"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let args = match serde_json::from_str::<Value>(&raw) {
        Ok(v) => v,
        // Preserve every parse failure, including a missing/empty arguments
        // field, as a non-object value. Central validation then returns a
        // typed, model-repairable invalid-arguments observation without ever
        // entering Tool::execute.
        Err(_) => Value::String(raw),
    };
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

/// Display-only note attached to a tool-call response. Never sent to TTS.
pub(super) fn extract_tool_note(response: &Value) -> Option<String> {
    let content = extract_reply_text(response);
    if content.is_empty() || crate::speech_sanitize::contains_tool_markup(&content) {
        return None;
    }
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let note = log_preview(&compact, 180);
    (!note.is_empty()).then_some(note)
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
    fn functions_namespace_is_removed_before_dispatch_and_history() {
        let response = json!({"role":"assistant", "content":"Checking.", "tool_calls":[
            {"id":"search", "type":"function", "function":{"name":"functions.web_search", "arguments":"{\"query\":\"example\"}"}},
            {"id":"fetch", "type":"function", "function":{"name":"functions.web_fetch", "arguments":"{\"url\":\"https://example.com\"}"}}
        ]});
        let (normalized, mappings) = normalize_tool_calls(response.clone(), &[]);
        let calls = parse_raw_tool_calls(normalized["tool_calls"].as_array().unwrap());
        assert_eq!(calls[0].name, "web_search");
        assert_eq!(calls[1].name, "web_fetch");
        assert_eq!(calls[0].call_id, "search");
        assert_eq!(calls[0].args, json!({"query":"example"}));
        assert_eq!(normalized["content"], response["content"]);
        assert!(mappings.is_empty());
    }

    #[test]
    fn parallel_normalizes_namespaced_envelope_and_children() {
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(crate::tools::parallel::ParallelTool)];
        let response = json!({"role":"assistant", "tool_calls":[
            {"id":"outer", "function":{"name":"functions.parallel", "arguments":json!({"tool_uses":[
                {"recipient_name":"functions.web_search", "parameters":{"query":"example"}},
                {"recipient_name":"functions.web_fetch", "parameters":{"url":"https://example.com"}}
            ]}).to_string()}}
        ]});
        let (expanded, mappings) = normalize_tool_calls(response, &tools);
        let calls = parse_raw_tool_calls(expanded["tool_calls"].as_array().unwrap());
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "web_search");
        assert_eq!(calls[1].name, "web_fetch");
        assert_eq!(calls[0].args, json!({"query":"example"}));
        assert_eq!(mappings[0]["parent_call_id"], "outer");
    }

    #[test]
    fn parallel_expansion_pairs_leaf_calls_and_preserves_native_siblings() {
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(crate::tools::parallel::ParallelTool)];
        let response = json!({"role":"assistant", "content":"Checking both facts.", "tool_calls":[
            {"id":"outer", "function":{"name":"parallel", "arguments":json!({"tool_uses":[
                {"recipient_name":"get_time", "parameters":{}},
                {"recipient_name":"get_date", "parameters":{}}
            ]}).to_string()}},
            {"id":"native", "function":{"name":"get_system_info", "arguments":"{}"}}
        ]});
        let (expanded, mappings) = normalize_tool_calls(response.clone(), &tools);
        let calls = parse_raw_tool_calls(expanded["tool_calls"].as_array().unwrap());
        assert_eq!(
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["get_time", "get_date", "get_system_info"]
        );
        assert_eq!(expanded["content"], response["content"]);
        assert_eq!(calls[2].call_id, "native");
        assert_ne!(calls[0].call_id, calls[1].call_id);
        assert_eq!(mappings[0]["parent_call_id"], "outer");
        assert_eq!(mappings[0]["children"][0]["call_id"], calls[0].call_id);
        assert_eq!(expanded["tool_calls"][2], response["tool_calls"][1]);
    }

    #[test]
    fn invalid_or_unregistered_parallel_is_not_partially_expanded() {
        let response = json!({"tool_calls":[{"id":"outer", "function":{"name":"parallel", "arguments":json!({"tool_uses":[
            {"recipient_name":"get_time", "parameters":{}},
            {"recipient_name":"parallel", "parameters":{}}
        ]}).to_string()}}]});
        let tools: Vec<std::sync::Arc<dyn Tool>> =
            vec![std::sync::Arc::new(crate::tools::parallel::ParallelTool)];
        for registry in [&tools[..], &[][..]] {
            let (expanded, mapping) = normalize_tool_calls(response.clone(), registry);
            assert_eq!(expanded, response);
            assert!(mapping.is_empty());
        }
    }

    #[test]
    fn tool_note_uses_same_response_content_without_speech() {
        let response = json!({
            "content": "I'll check the status path, then update the overlay.\n",
            "tool_calls": [{"id": "c1", "function": {"name": "file_read", "arguments": "{}"}}]
        });
        assert_eq!(
            extract_tool_note(&response).as_deref(),
            Some("I'll check the status path, then update the overlay.")
        );
        assert!(extract_tool_note(&json!({"content": null})).is_none());
        assert!(extract_tool_note(&json!({"content": "<invoke>file_read</invoke>"})).is_none());
    }

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
    fn parse_tool_calls_preserves_missing_empty_and_bad_args_as_invalid() {
        let calls = vec![
            json!({}),
            json!({
                "id": "c-empty",
                "function": { "name": "x", "arguments": "" }
            }),
            json!({
                "id": "c2",
                "function": { "name": "x", "arguments": "not-json" }
            }),
        ];
        let parsed = parse_raw_tool_calls(&calls);
        assert_eq!(parsed[0].call_id, "");
        assert_eq!(parsed[0].name, "");
        assert_eq!(parsed[0].args.as_str(), Some(""));
        assert_eq!(parsed[1].args.as_str(), Some(""));
        assert!(
            parsed[2].args.is_string(),
            "malformed args must not coerce to {{}}"
        );
        assert_eq!(parsed[2].args.as_str(), Some("not-json"));
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
