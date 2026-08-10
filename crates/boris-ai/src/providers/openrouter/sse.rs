//! SSE stream assembly for chat completions.
//!
//! Accumulates content + tool-call argument fragments into one assistant message.
//!
//! # Multi-line SSE limitation
//!
//! Only **single-line** `data:` payloads are handled (the common case for
//! OpenAI-compatible chat completions). Events that span multiple `data:` lines
//! (joined with `\n` per the SSE spec) are processed **line-by-line** as
//! independent payloads — multi-line JSON would not reassemble correctly.
//! A full event-boundary parser can be added if a provider needs it.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::message::{
    append_stream_content_delta, assistant_message_from_stream, extract_text_content,
};
use crate::usage::TokenUsage;

/// Incremental tool-call fragment accumulator (index → parts).
#[derive(Default)]
struct ToolCallAcc {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAcc {
    fn apply_delta(&mut self, tc: &Value) {
        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
            if !id.is_empty() {
                self.id = id.to_string();
            }
        }
        if let Some(func) = tc.get("function") {
            if let Some(n) = func.get("name").and_then(|n| n.as_str()) {
                if !n.is_empty() {
                    apply_name_delta(&mut self.name, n);
                }
            }
            if let Some(a) = func.get("arguments").and_then(|a| a.as_str()) {
                self.arguments.push_str(a);
            }
        }
    }

    fn into_json(self, index: u32) -> Value {
        let id = if self.id.is_empty() {
            format!("call_{index}")
        } else {
            self.id
        };
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": self.arguments,
            }
        })
    }
}

/// Merge a tool-call name fragment without doubling a full-name resend.
///
/// - empty sink → set
/// - identical or already-applied fragment → ignore
/// - longer name that starts with current → replace (partial → full)
/// - otherwise → append (true incremental fragment)
fn apply_name_delta(name: &mut String, delta: &str) {
    if name.is_empty() {
        name.push_str(delta);
        return;
    }
    if delta == name.as_str() || name.ends_with(delta) {
        return;
    }
    if delta.starts_with(name.as_str()) {
        name.clear();
        name.push_str(delta);
        return;
    }
    name.push_str(delta);
}

/// Assembles a full assistant message from OpenAI-style SSE deltas.
#[derive(Default)]
pub(super) struct StreamAssembler {
    content: String,
    tools: BTreeMap<u32, ToolCallAcc>,
    role: String,
    last_usage: Option<TokenUsage>,
}

impl StreamAssembler {
    pub(super) fn new() -> Self {
        Self {
            role: "assistant".to_string(),
            ..Default::default()
        }
    }

    pub(super) fn last_usage(&self) -> Option<&TokenUsage> {
        self.last_usage.as_ref()
    }

    /// Ingest one parsed SSE `data:` JSON payload.
    pub(super) fn ingest_event(&mut self, event: &Value) {
        if let Some(usage) = event.get("usage") {
            self.last_usage = Some(TokenUsage::from_usage_value(usage));
        }

        let Some(choice) = event.get("choices").and_then(|c| c.get(0)) else {
            return;
        };

        // Prefer incremental delta; some providers also emit a full `message`.
        let Some(piece) = choice.get("delta").or_else(|| choice.get("message")) else {
            return;
        };

        if let Some(r) = piece.get("role").and_then(|r| r.as_str()) {
            self.role = r.to_string();
        }

        if let Some(c) = piece.get("content") {
            // Full `message` objects sometimes carry trimmed-friendly content;
            // streaming `delta` strings must preserve raw spaces.
            if choice.get("delta").is_some() {
                append_stream_content_delta(&mut self.content, c);
            } else if let Some(s) = c.as_str() {
                // Non-delta full message: replace/set content once.
                if self.content.is_empty() {
                    self.content = s.to_string();
                } else {
                    append_stream_content_delta(&mut self.content, c);
                }
            } else {
                let text = extract_text_content(c);
                if !text.is_empty() {
                    self.content.push_str(&text);
                }
            }
        }

        if let Some(tcs) = piece.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                self.tools.entry(idx).or_default().apply_delta(tc);
            }
        }
    }

    /// Finish assembly into a single assistant message object.
    pub(super) fn finish(self) -> Value {
        let tool_calls: Vec<Value> = self
            .tools
            .into_iter()
            .map(|(idx, acc)| acc.into_json(idx))
            .collect();
        assistant_message_from_stream(&self.role, self.content, tool_calls)
    }
}

/// Feed raw HTTP body bytes into an SSE line buffer; invoke `on_data` for each
/// complete newline-terminated non-empty `data:` payload (excluding `[DONE]`).
pub(super) fn push_sse_bytes(buffer: &mut String, chunk: &[u8], mut on_data: impl FnMut(&str)) {
    buffer.push_str(&String::from_utf8_lossy(chunk));
    while let Some(pos) = buffer.find('\n') {
        let line = buffer[..pos].trim_end_matches('\r').to_string();
        buffer.drain(..=pos);
        dispatch_sse_line(&line, &mut on_data);
    }
}

/// After the byte stream ends, treat any remaining buffer as a final line
/// (providers sometimes omit the trailing newline on the last event).
pub(super) fn flush_sse_buffer(buffer: &mut String, mut on_data: impl FnMut(&str)) {
    if buffer.is_empty() {
        return;
    }
    let line = std::mem::take(buffer);
    let line = line.trim_end_matches(['\r', '\n']);
    if !line.is_empty() {
        dispatch_sse_line(line, &mut on_data);
    }
}

fn dispatch_sse_line(line: &str, on_data: &mut impl FnMut(&str)) {
    let Some(data) = sse_data_payload(line) else {
        return;
    };
    if data.is_empty() || data == "[DONE]" {
        return;
    }
    on_data(data);
}

fn sse_data_payload(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("data:") {
        return Some(rest.trim());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assembles_content_and_tools() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({
            "choices": [{ "delta": { "role": "assistant", "content": "Hel" } }]
        }));
        a.ingest_event(&json!({
            "choices": [{ "delta": { "content": "lo" } }]
        }));
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": { "name": "bash", "arguments": "{\"c\"" }
                    }]
                }
            }]
        }));
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": ":\"ls\"}" }
                    }]
                }
            }]
        }));
        a.ingest_event(&json!({
            "usage": { "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3 }
        }));

        let msg = a.finish();
        assert_eq!(msg["role"], "assistant");
        assert_eq!(msg["content"], "Hello");
        assert_eq!(msg["tool_calls"][0]["id"], "call_1");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "bash");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"c\":\"ls\"}");
    }

    #[test]
    fn push_sse_bytes_splits_lines() {
        let mut buf = String::new();
        let mut payloads = Vec::new();
        push_sse_bytes(&mut buf, b"data: {\"a\":1}\n\ndata: [DONE]\n", |p| {
            payloads.push(p.to_string());
        });
        assert_eq!(payloads, vec![r#"{"a":1}"#]);
        assert!(buf.is_empty() || !buf.contains('\n'));
    }

    #[test]
    fn flush_sse_buffer_without_trailing_newline() {
        let mut buf = String::new();
        let mut payloads = Vec::new();
        push_sse_bytes(&mut buf, br#"data: {"choices":[{"delta":{"content":"hi"}}]}"#, |p| {
            payloads.push(p.to_string());
        });
        // No newline yet — payload stays buffered.
        assert!(payloads.is_empty());
        assert!(!buf.is_empty());

        flush_sse_buffer(&mut buf, |p| {
            payloads.push(p.to_string());
        });
        assert_eq!(payloads.len(), 1);
        assert!(payloads[0].contains("hi"));
        assert!(buf.is_empty());
    }

    #[test]
    fn flush_empty_buffer_is_noop() {
        let mut buf = String::new();
        let mut called = false;
        flush_sse_buffer(&mut buf, |_| called = true);
        assert!(!called);
    }

    #[test]
    fn tool_name_resend_does_not_double() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "c1",
                        "function": { "name": "get_time", "arguments": "" }
                    }]
                }
            }]
        }));
        // Provider resends full name with argument fragment.
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "name": "get_time", "arguments": "{}" }
                    }]
                }
            }]
        }));
        let msg = a.finish();
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "get_time");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{}");
    }

    #[test]
    fn tool_name_incremental_fragments_append() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "name": "get_", "arguments": "" }
                    }]
                }
            }]
        }));
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "name": "time", "arguments": "" }
                    }]
                }
            }]
        }));
        let msg = a.finish();
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "get_time");
    }

    #[test]
    fn apply_name_delta_unit() {
        let mut n = String::new();
        apply_name_delta(&mut n, "bash");
        assert_eq!(n, "bash");
        apply_name_delta(&mut n, "bash"); // resend
        assert_eq!(n, "bash");
        n.clear();
        apply_name_delta(&mut n, "ba");
        apply_name_delta(&mut n, "bash"); // expands partial
        assert_eq!(n, "bash");
    }
}
