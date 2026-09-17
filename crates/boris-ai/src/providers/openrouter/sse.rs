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

use crate::error::truncate_error_body;
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

/// Incremental fragments produced by one SSE event (for typed stream callers).
#[derive(Debug, Clone, Default)]
pub(super) struct StreamDelta {
    pub content: String,
    pub reasoning: String,
    pub tool_deltas: Vec<ToolDelta>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(super) struct ToolDelta {
    pub index: u32,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments_delta: String,
}

/// True when a tool delta carries no new information at all.
///
/// Used by the stream caller (`complete.rs`) to keep fully-empty fragments
/// out of payload detection: a bare `{"index": 0}` heartbeat must not mark
/// the stream as "saw payload" or trigger `FirstDelta`.
pub(crate) fn tool_delta_is_empty(delta: &ToolDelta) -> bool {
    delta.id.is_none() && delta.name.is_none() && delta.arguments_delta.is_empty()
}

impl ToolCallAcc {
    fn from_message(tc: &Value) -> Self {
        let function = tc.get("function");
        Self {
            id: tc
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: function
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            arguments: function
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }

    fn apply_delta(&mut self, tc: &Value) -> ToolDelta {
        let mut id_out = None;
        let mut name_out = None;
        let mut arguments_delta = String::new();
        if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
            if !id.is_empty() {
                self.id = id.to_string();
                id_out = Some(id.to_string());
            }
        }
        if let Some(func) = tc.get("function") {
            if let Some(n) = func.get("name").and_then(|n| n.as_str()) {
                if !n.is_empty() {
                    let before = self.name.clone();
                    apply_name_delta(&mut self.name, n);
                    if self.name != before {
                        name_out = Some(self.name.clone());
                    }
                }
            }
            if let Some(a) = func.get("arguments").and_then(|a| a.as_str()) {
                self.arguments.push_str(a);
                arguments_delta = a.to_string();
            }
        }
        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
        ToolDelta {
            index: idx,
            id: id_out,
            name: name_out,
            arguments_delta,
        }
    }

    fn into_json(self, index: u32) -> Value {
        // Synthetic `call_{index}` ids fill gaps when a provider streams no
        // tool-call id. The id scheme is load-bearing for transcripts/tests, so
        // it stays as-is; the blocking path is normalized the same way (A4:
        // empty `tool_calls` omitted, `content` always a string) for parity.
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
    /// Whether each payload kind has already been exposed as an incremental
    /// event. A later full `message` is a canonical snapshot, not another
    /// delta, so it must not be emitted as duplicate content/tool fragments.
    emitted_content: bool,
    emitted_reasoning: bool,
    emitted_tools: bool,
    /// Set when an event carries a top-level `error` object instead of a
    /// normal delta (e.g. a mid-stream provider error). Callers should check
    /// this after each ingested chunk and abort the stream if set.
    error: Option<Value>,
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

    /// Take a pending provider error surfaced by an ingested SSE event, if any.
    pub(super) fn take_error(&mut self) -> Option<Value> {
        self.error.take()
    }

    /// Ingest one parsed SSE `data:` JSON payload.
    #[allow(dead_code)]
    pub(super) fn ingest_event(&mut self, event: &Value) {
        let _ = self.ingest_event_delta(event);
    }

    /// Same as [`Self::ingest_event`] but returns newly appended fragments.
    /// A provider-supplied full `message` replaces the assembled snapshot; it
    /// is only surfaced as an incremental event when no earlier delta of that
    /// payload kind was emitted.
    pub(super) fn ingest_event_delta(&mut self, event: &Value) -> StreamDelta {
        let mut delta = StreamDelta::default();
        // A well-formed SSE payload can carry a top-level `error` object
        // instead of `choices` (e.g. `data: {"error":{"message":"..."}}`).
        // Surface it instead of silently dropping the event, which used to
        // leave the stream to "succeed" with an empty assembled message.
        if let Some(err) = event.get("error") {
            // Abort even when the event also carries valid choices: mixing a
            // provider error with a partial draft risks acting on a payload
            // the provider already disowned.
            let choices_len = event
                .get("choices")
                .and_then(|c| c.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            tracing::warn!(
                error = %truncate_error_body(&err.to_string()),
                choices_len,
                "SSE event carries top-level error; aborting stream assembly"
            );
            self.error = Some(err.clone());
            return delta;
        }

        if let Some(usage) = event.get("usage") {
            let u = TokenUsage::from_usage_value(usage);
            self.last_usage = Some(u.clone());
            delta.usage = Some(u);
        }

        if let Some(choices) = event.get("choices").and_then(|c| c.as_array()) {
            if choices.len() > 1 {
                tracing::warn!(
                    choices_len = choices.len(),
                    "SSE event carries multiple choices; only choices[0] is assembled"
                );
            }
        }
        let Some(choice) = event.get("choices").and_then(|c| c.get(0)) else {
            return delta;
        };

        // Some OpenAI-compatible providers finish a delta stream with a full
        // `message` snapshot. Treat that snapshot as canonical. Appending it
        // after prior deltas glues two answer drafts together (for example,
        // `first answer?Second answer`).
        //
        // When one choice carries BOTH a `delta` and a `message` whose
        // `tool_calls` are non-empty, the snapshot is ingested FIRST and the
        // delta merged on top: ignoring the delta would drop argument bytes,
        // while ignoring the snapshot would drop the canonical tool list.
        let message_with_tools = choice.get("message").filter(|m| {
            !m.is_null()
                && m.get("tool_calls")
                    .and_then(|t| t.as_array())
                    .is_some_and(|a| !a.is_empty())
        });
        match (choice.get("delta"), message_with_tools) {
            (Some(piece), Some(message)) if !piece.is_null() => {
                self.ingest_message_snapshot(message, &mut delta);
                self.ingest_incremental_piece(piece, &mut delta);
            }
            (Some(piece), _) if !piece.is_null() => {
                self.ingest_incremental_piece(piece, &mut delta);
            }
            (_, Some(message)) => {
                self.ingest_message_snapshot(message, &mut delta);
            }
            _ => {
                if let Some(message) = choice.get("message") {
                    if !message.is_null() {
                        self.ingest_message_snapshot(message, &mut delta);
                    }
                }
            }
        }
        delta
    }

    fn ingest_incremental_piece(&mut self, piece: &Value, delta: &mut StreamDelta) {
        if let Some(r) = piece.get("role").and_then(|r| r.as_str()) {
            self.role = r.to_lowercase();
        }

        append_reasoning_delta(&mut delta.reasoning, piece);
        if !delta.reasoning.is_empty() {
            self.emitted_reasoning = true;
        }

        if let Some(c) = piece.get("content") {
            let before = self.content.len();
            append_stream_content_delta(&mut self.content, c);
            if self.content.len() > before {
                debug_assert!(
                    self.content.is_char_boundary(before),
                    "content is only extended with &str chunks, so `before` is a char boundary"
                );
                delta.content = self.content[before..].to_string();
                self.emitted_content = true;
            }
        }

        if let Some(tcs) = piece.get("tool_calls").and_then(|t| t.as_array()) {
            for tc in tcs {
                let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                let mut td = self.tools.entry(idx).or_default().apply_delta(tc);
                td.index = idx;
                delta.tool_deltas.push(td);
            }
            if !delta.tool_deltas.is_empty() {
                self.emitted_tools = true;
            }
        }
    }

    fn ingest_message_snapshot(&mut self, message: &Value, delta: &mut StreamDelta) {
        if let Some(r) = message.get("role").and_then(|r| r.as_str()) {
            self.role = r.to_lowercase();
        }

        let mut reasoning = String::new();
        append_reasoning_delta(&mut reasoning, message);
        if !self.emitted_reasoning && !reasoning.is_empty() {
            delta.reasoning = reasoning;
            self.emitted_reasoning = true;
        }

        // `message.content` is the provider's complete canonical value. It
        // replaces any draft assembled from `delta.content` chunks.
        self.content = message
            .get("content")
            .map(|content| {
                content
                    .as_str()
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| extract_text_content(content))
            })
            .unwrap_or_default();
        if !self.emitted_content && !self.content.is_empty() {
            delta.content = self.content.clone();
            self.emitted_content = true;
        }

        // Full assistant messages do not carry streaming indexes reliably, so
        // rebuild tool state by array position instead of applying them as
        // fragments to previously accumulated calls.
        //
        // Replacement semantics are intentional (pinned by tests): a canonical
        // snapshot supersedes earlier fragments. Shrinking the set drops prior
        // drafts, which is worth a warning for diagnosis.
        let tools_before = self.tools.len();
        self.tools.clear();
        if let Some(tcs) = message.get("tool_calls").and_then(|t| t.as_array()) {
            for (position, tc) in tcs.iter().enumerate() {
                let idx = tc
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(position as u64) as u32;
                let call = ToolCallAcc::from_message(tc);
                if !self.emitted_tools {
                    delta.tool_deltas.push(ToolDelta {
                        index: idx,
                        id: (!call.id.is_empty()).then(|| call.id.clone()),
                        name: (!call.name.is_empty()).then(|| call.name.clone()),
                        arguments_delta: call.arguments.clone(),
                    });
                }
                self.tools.insert(idx, call);
            }
            if tools_before > 0 && self.tools.len() < tools_before {
                tracing::warn!(
                    before = tools_before,
                    after = self.tools.len(),
                    "message snapshot dropped previously accumulated tool calls"
                );
            }
            if !delta.tool_deltas.is_empty() {
                self.emitted_tools = true;
            }
        }
    }

    /// Snapshot completed tool calls (id + name + full arguments string).
    ///
    /// Uses the same synthetic `call_{index}` fallback as [`finish`](Self::finish);
    /// see [`ToolCallAcc::into_json`] for why the scheme is frozen.
    #[allow(dead_code)]
    pub(super) fn completed_tools(&self) -> Vec<(u32, String, String, String)> {
        self.tools
            .iter()
            .map(|(idx, acc)| {
                let id = if acc.id.is_empty() {
                    format!("call_{idx}")
                } else {
                    acc.id.clone()
                };
                (*idx, id, acc.name.clone(), acc.arguments.clone())
            })
            .collect()
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
///
/// `buffer` holds **raw, possibly-incomplete UTF-8 bytes** across calls —
/// network chunk boundaries have no relationship to UTF-8 character
/// boundaries, so a multi-byte character can legitimately arrive split
/// across two chunks. Decoding each chunk independently (the previous
/// implementation) replaced both halves of a split character with U+FFFD.
/// Instead we only decode once a complete line has accumulated: the `\n`
/// (`0x0A`) delimiter we split on can never appear as a byte inside a
/// multi-byte UTF-8 sequence, so splitting on raw bytes here is always safe.
pub(super) fn push_sse_bytes(buffer: &mut Vec<u8>, chunk: &[u8], mut on_data: impl FnMut(&str)) {
    buffer.extend_from_slice(chunk);
    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
        let line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
        let line = String::from_utf8_lossy(&line_bytes);
        let line = line.trim_end_matches(['\r', '\n']);
        dispatch_sse_line(line, &mut on_data);
    }
}

/// After the byte stream ends, treat any remaining buffer as a final line
/// (providers sometimes omit the trailing newline on the last event).
pub(super) fn flush_sse_buffer(buffer: &mut Vec<u8>, mut on_data: impl FnMut(&str)) {
    if buffer.is_empty() {
        return;
    }
    let line_bytes = std::mem::take(buffer);
    let line = String::from_utf8_lossy(&line_bytes);
    let line = line.trim_end_matches(['\r', '\n']);
    if !line.is_empty() {
        dispatch_sse_line(line, &mut on_data);
    }
}

/// Pull incremental thinking text from OpenRouter / OpenAI-compatible deltas.
///
/// Prefers the string `reasoning` field, then `reasoning_content`, then
/// `reasoning_details[].text` (Anthropic-style via OpenRouter). Never mixed
/// into assembled `content` — this is display-only.
fn append_reasoning_delta(sink: &mut String, piece: &Value) {
    if let Some(s) = string_delta(piece.get("reasoning")) {
        sink.push_str(s);
        return;
    }
    if let Some(s) = string_delta(piece.get("reasoning_content")) {
        sink.push_str(s);
        return;
    }
    let Some(details) = piece.get("reasoning_details").and_then(|v| v.as_array()) else {
        return;
    };
    for detail in details {
        // `text` + `summary` fragments arrive without separators; join with a
        // space so words from adjacent details are not glued together.
        let s = string_delta(detail.get("text")).or_else(|| string_delta(detail.get("summary")));
        if let Some(s) = s {
            push_reasoning_fragment(sink, s);
        }
    }
}

/// Append one reasoning fragment, separating it from prior text with a space
/// unless whitespace already separates them.
fn push_reasoning_fragment(sink: &mut String, fragment: &str) {
    if !sink.is_empty()
        && !sink.ends_with(char::is_whitespace)
        && !fragment.starts_with(char::is_whitespace)
    {
        sink.push(' ');
    }
    sink.push_str(fragment);
}

fn string_delta(value: Option<&Value>) -> Option<&str> {
    value.and_then(|v| v.as_str()).filter(|s| !s.is_empty())
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
        assert_eq!(
            msg["tool_calls"][0]["function"]["arguments"],
            "{\"c\":\"ls\"}"
        );
    }

    #[test]
    fn full_message_replaces_streamed_draft_instead_of_concatenating() {
        let mut a = StreamAssembler::new();
        let first = a.ingest_event_delta(&json!({
            "choices": [{ "delta": { "role": "assistant", "content": "First answer?" } }]
        }));
        assert_eq!(first.content, "First answer?");

        let snapshot = a.ingest_event_delta(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Final answer."
                }
            }]
        }));

        assert!(
            snapshot.content.is_empty(),
            "a canonical snapshot must not be replayed as another delta"
        );
        assert_eq!(a.finish()["content"], "Final answer.");
    }

    #[test]
    fn full_message_without_prior_deltas_is_exposed_once() {
        let mut a = StreamAssembler::new();
        let snapshot = a.ingest_event_delta(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Only answer."
                }
            }]
        }));
        assert_eq!(snapshot.content, "Only answer.");

        let repeated = a.ingest_event_delta(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Only answer."
                }
            }]
        }));
        assert!(repeated.content.is_empty());
        assert_eq!(a.finish()["content"], "Only answer.");
    }

    #[test]
    fn full_message_replaces_streamed_tool_fragments() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "draft-call",
                        "function": { "name": "todo_", "arguments": "{\"draft\":" }
                    }]
                }
            }]
        }));
        let snapshot = a.ingest_event_delta(&json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "final-call",
                        "type": "function",
                        "function": { "name": "todo_read", "arguments": "{}" }
                    }]
                }
            }]
        }));

        assert!(snapshot.tool_deltas.is_empty());
        let msg = a.finish();
        assert_eq!(msg["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(msg["tool_calls"][0]["id"], "final-call");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "todo_read");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{}");
    }

    #[test]
    fn push_sse_bytes_splits_lines() {
        let mut buf = Vec::new();
        let mut payloads = Vec::new();
        push_sse_bytes(&mut buf, b"data: {\"a\":1}\n\ndata: [DONE]\n", |p| {
            payloads.push(p.to_string());
        });
        assert_eq!(payloads, vec![r#"{"a":1}"#]);
        assert!(buf.is_empty() || !buf.contains(&b'\n'));
    }

    #[test]
    fn flush_sse_buffer_without_trailing_newline() {
        let mut buf = Vec::new();
        let mut payloads = Vec::new();
        push_sse_bytes(
            &mut buf,
            br#"data: {"choices":[{"delta":{"content":"hi"}}]}"#,
            |p| {
                payloads.push(p.to_string());
            },
        );
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
        let mut buf = Vec::new();
        let mut called = false;
        flush_sse_buffer(&mut buf, |_| called = true);
        assert!(!called);
    }

    #[test]
    fn push_sse_bytes_does_not_corrupt_multibyte_char_split_across_chunks() {
        // "café" — the 'é' (U+00E9) encodes as the 2 bytes 0xC3 0xA9 in UTF-8.
        // Simulate a network chunk boundary landing between those two bytes.
        let full = "data: {\"a\":\"café\"}\n".as_bytes().to_vec();
        let split_at = full.len() - 2; // splits inside the 2-byte 'é' sequence
        let (chunk1, chunk2) = full.split_at(split_at);

        let mut buf = Vec::new();
        let mut payloads = Vec::new();
        push_sse_bytes(&mut buf, chunk1, |p| payloads.push(p.to_string()));
        assert!(
            payloads.is_empty(),
            "line not complete yet, nothing should decode"
        );
        push_sse_bytes(&mut buf, chunk2, |p| payloads.push(p.to_string()));

        assert_eq!(payloads.len(), 1);
        assert!(
            payloads[0].contains("café"),
            "multi-byte char split across chunks must decode correctly, got: {:?}",
            payloads[0]
        );
    }

    #[test]
    fn ingest_event_surfaces_top_level_error() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({ "error": { "message": "rate limited", "code": 429 } }));
        let err = a.take_error().expect("error should be surfaced");
        assert_eq!(err["message"], "rate limited");
        assert_eq!(err["code"], 429);
        // Error events must not be treated as content.
        assert_eq!(a.finish()["content"], "");
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
    fn reasoning_string_does_not_enter_content() {
        let mut a = StreamAssembler::new();
        let delta = a.ingest_event_delta(&json!({
            "choices": [{ "delta": { "reasoning": "Need to look this up. " } }]
        }));
        assert_eq!(delta.reasoning, "Need to look this up. ");
        assert!(delta.content.is_empty());

        let delta = a.ingest_event_delta(&json!({
            "choices": [{ "delta": { "reasoning_content": "Then search. ", "content": "On it." } }]
        }));
        // First field (`reasoning`) is absent, so `reasoning_content` is used.
        assert_eq!(delta.reasoning, "Then search. ");
        assert_eq!(delta.content, "On it.");
        assert_eq!(a.finish()["content"], "On it.");
    }

    #[test]
    fn reasoning_details_text_is_collected() {
        let mut a = StreamAssembler::new();
        let delta = a.ingest_event_delta(&json!({
            "choices": [{
                "delta": {
                    "reasoning_details": [{
                        "type": "reasoning.text",
                        "text": "Step one."
                    }]
                }
            }]
        }));
        assert_eq!(delta.reasoning, "Step one.");
        assert!(delta.content.is_empty());
    }

    #[test]
    fn delta_and_message_with_tools_merge() {
        let mut a = StreamAssembler::new();
        let delta = a.ingest_event_delta(&json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{\"extra\":1}" }
                    }]
                },
                "message": {
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": "snap-call",
                        "type": "function",
                        "function": { "name": "todo_read", "arguments": "{}" }
                    }]
                }
            }]
        }));
        // Snapshot ingested first, delta arguments merged on top.
        assert!(!delta.tool_deltas.is_empty());
        let msg = a.finish();
        assert_eq!(msg["tool_calls"][0]["id"], "snap-call");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "todo_read");
        assert!(
            msg["tool_calls"][0]["function"]["arguments"]
                .as_str()
                .unwrap()
                .contains("extra"),
            "delta arguments must merge onto the snapshot, got: {}",
            msg["tool_calls"][0]["function"]["arguments"]
        );
    }

    #[test]
    fn delta_without_snapshot_tools_keeps_delta_only_behavior() {
        let mut a = StreamAssembler::new();
        a.ingest_event_delta(&json!({
            "choices": [{
                "delta": { "content": "Hi" },
                "message": { "role": "assistant", "content": "Ignored draft" }
            }]
        }));
        // No tool_calls in the message → legacy path: delta wins, snapshot ignored.
        assert_eq!(a.finish()["content"], "Hi");
    }

    #[test]
    fn extra_choices_are_ignored_but_first_assembles() {
        let mut a = StreamAssembler::new();
        let delta = a.ingest_event_delta(&json!({
            "choices": [
                { "delta": { "content": "one" } },
                { "delta": { "content": "two" } }
            ]
        }));
        assert_eq!(delta.content, "one");
        assert_eq!(a.finish()["content"], "one");
    }

    #[test]
    fn role_is_lowercased() {
        let mut a = StreamAssembler::new();
        a.ingest_event(&json!({
            "choices": [{ "delta": { "role": "ASSISTANT", "content": "hi" } }]
        }));
        assert_eq!(a.finish()["role"], "assistant");

        let mut b = StreamAssembler::new();
        b.ingest_event(&json!({
            "choices": [{ "message": { "role": "Assistant", "content": "hi" } }]
        }));
        assert_eq!(b.finish()["role"], "assistant");
    }

    #[test]
    fn reasoning_details_text_and_summary_join_with_space() {
        let mut a = StreamAssembler::new();
        let delta = a.ingest_event_delta(&json!({
            "choices": [{
                "delta": {
                    "reasoning_details": [
                        { "type": "reasoning.text", "text": "Step one." },
                        { "type": "reasoning.summary", "summary": "Step two." }
                    ]
                }
            }]
        }));
        assert_eq!(delta.reasoning, "Step one. Step two.");
        assert!(delta.content.is_empty());
    }

    #[test]
    fn tool_delta_is_empty_unit() {
        assert!(tool_delta_is_empty(&ToolDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: String::new(),
        }));
        assert!(!tool_delta_is_empty(&ToolDelta {
            index: 0,
            id: None,
            name: None,
            arguments_delta: "{}".into(),
        }));
        assert!(!tool_delta_is_empty(&ToolDelta {
            index: 0,
            id: Some("c1".into()),
            name: None,
            arguments_delta: String::new(),
        }));
        assert!(!tool_delta_is_empty(&ToolDelta {
            index: 0,
            id: None,
            name: Some("bash".into()),
            arguments_delta: String::new(),
        }));
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
