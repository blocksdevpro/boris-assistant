//! `complete` paths: stream-first with non-stream fallback.

use std::time::Instant;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::LlmClient;
use crate::error::{truncate_error_body, LlmError, LlmErrorKind};
use crate::message::{message_has_usable_payload, normalize_assistant_message};
use crate::request::CompleteOptions;
use crate::stream::LlmStreamEvent;
use crate::usage::{log_complete, log_complete_failed, TokenUsage};

use super::client::OpenRouterClient;
use super::sse::{
    flush_sse_buffer, push_sse_bytes, tool_delta_is_empty, StreamAssembler, StreamDelta,
};

#[async_trait]
impl LlmClient for OpenRouterClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        self.complete_with_options(messages, tools, CompleteOptions::default())
            .await
    }

    async fn complete_with_options(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
    ) -> Result<Value, LlmError> {
        self.complete_assembled(&messages, &tools, &opts).await
    }

    async fn complete_stream(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        on_event(LlmStreamEvent::ModelSend {
            model: self.model.clone(),
        });
        let started = Instant::now();
        let mut first_delta_emitted = false;
        let mut saw_payload_delta = false;
        let streamed = self
            .complete_streaming_with(&messages, &tools, &opts, |delta| {
                // Fully-empty tool heartbeat fragments (`{"index": 0}` with no
                // id/name/arguments) do not count as payload. Reasoning-only
                // streams intentionally still fall back (reasoning is display
                // text, not a speakable answer or tool call).
                if !delta.content.is_empty()
                    || delta.tool_deltas.iter().any(|t| !tool_delta_is_empty(t))
                {
                    saw_payload_delta = true;
                }
                emit_stream_delta(on_event, delta, started, &mut first_delta_emitted);
            })
            .await;

        match streamed {
            Ok((msg, usage)) if message_has_usable_payload(&msg) => {
                log_complete(
                    &self.model,
                    "stream-events",
                    started.elapsed().as_millis() as u64,
                    usage.as_ref(),
                );
                emit_completed_tool_events(on_event, &msg);
                on_event(LlmStreamEvent::FinalMessage(msg.clone()));
                Ok(msg)
            }
            Ok((_empty, _)) => {
                tracing::warn!(
                    ms = started.elapsed().as_millis() as u64,
                    "typed stream returned empty payload; falling back to non-stream"
                );
                self.complete_blocking_with_events(&messages, &tools, &opts, on_event)
                    .await
            }
            Err(e) if !saw_payload_delta && should_fallback_for_error(&e) => {
                tracing::debug!(
                    error = %e,
                    ms = started.elapsed().as_millis() as u64,
                    "typed stream failed before first payload; falling back to non-stream"
                );
                self.complete_blocking_with_events(&messages, &tools, &opts, on_event)
                    .await
            }
            Err(e) => {
                // Replaying a blocking result after real deltas would duplicate
                // or contradict content already observed by the consumer.
                log_complete_failed(
                    &self.model,
                    "stream-events",
                    started.elapsed().as_millis() as u64,
                    &e,
                );
                Err(e)
            }
        }
    }

    fn model(&self) -> &str {
        OpenRouterClient::model(self)
    }

    fn context_window_tokens(&self) -> Option<u32> {
        Some(OpenRouterClient::context_window_tokens(self))
    }
}

impl OpenRouterClient {
    async fn complete_blocking_with_events(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        let started = Instant::now();
        match self.complete_blocking_inner(messages, tools, opts).await {
            Ok((message, usage)) => {
                log_complete(
                    &self.model,
                    "blocking-stream-fallback",
                    started.elapsed().as_millis() as u64,
                    usage.as_ref(),
                );
                // Event parity with the streaming path: `ModelSend` was already
                // emitted once by `complete_stream` before the fallback, so it
                // is NOT re-emitted here. Delta-only consumers still need
                // `FirstDelta` (+ the full text as one `ContentDelta`) to
                // notice an answer that arrived via fallback.
                emit_blocking_fallback_events(
                    on_event,
                    &message,
                    started.elapsed().as_millis() as u64,
                );
                if let Some(usage) = usage {
                    on_event(LlmStreamEvent::Usage(usage));
                }
                on_event(LlmStreamEvent::FinalMessage(message.clone()));
                Ok(message)
            }
            Err(e) => {
                log_complete_failed(
                    &self.model,
                    "blocking-stream-fallback",
                    started.elapsed().as_millis() as u64,
                    &e,
                );
                Err(e)
            }
        }
    }

    async fn complete_assembled(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
    ) -> Result<Value, LlmError> {
        // Prefer SSE: lower TTFB; tools can start as soon as the body is assembled.
        //
        // Some OpenRouter/Gemini streams return HTTP 200 with zero content deltas
        // (thinking-only / empty assembly). Treat that as failure and fall back
        // so the agent does not go Silent with nothing to say.
        let stream_t = Instant::now();
        match self.complete_streaming(messages, tools, opts).await {
            Ok((msg, usage)) if message_has_usable_payload(&msg) => {
                log_complete(
                    &self.model,
                    "stream",
                    stream_t.elapsed().as_millis() as u64,
                    usage.as_ref(),
                );
                Ok(msg)
            }
            Ok((msg, _)) => {
                tracing::warn!(
                    content_preview = %msg.get("content").map(|c| c.to_string()).unwrap_or_default(),
                    has_tools = crate::message::has_tool_calls(&msg),
                    ms = stream_t.elapsed().as_millis() as u64,
                    "stream complete returned empty payload; falling back to non-stream"
                );
                self.complete_blocking(messages, tools, opts).await
            }
            Err(e) => {
                // No second request for provider rejections (auth / rate limit
                // / bad model): the blocking call would fail the same way and
                // burn rate-limit budget. See `should_fallback_for_error`.
                if !should_fallback_for_error(&e) {
                    tracing::debug!(
                        error = %e,
                        kind = %e.kind(),
                        ms = stream_t.elapsed().as_millis() as u64,
                        "stream complete failed with provider error; not falling back"
                    );
                    return Err(e);
                }
                tracing::debug!(
                    error = %e,
                    ms = stream_t.elapsed().as_millis() as u64,
                    "stream complete failed; falling back to non-stream"
                );
                self.complete_blocking(messages, tools, opts).await
            }
        }
    }

    /// Non-streaming JSON response path.
    pub(super) async fn complete_blocking(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
    ) -> Result<Value, LlmError> {
        let started = Instant::now();
        match self.complete_blocking_inner(messages, tools, opts).await {
            Ok((message, usage)) => {
                log_complete(
                    &self.model,
                    "blocking",
                    started.elapsed().as_millis() as u64,
                    usage.as_ref(),
                );
                Ok(message)
            }
            Err(e) => {
                log_complete_failed(
                    &self.model,
                    "blocking",
                    started.elapsed().as_millis() as u64,
                    &e,
                );
                Err(e)
            }
        }
    }

    async fn complete_blocking_inner(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
    ) -> Result<(Value, Option<TokenUsage>), LlmError> {
        let body = self.request_body_with(messages, tools, false, opts);

        let req = self.http.post(self.chat_completions_url()).json(&body);
        let req = self.apply_common_headers(req);

        let response = req.send().await.map_err(LlmError::from)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::from_http_status(status, &body));
        }

        let json: Value = response.json().await.map_err(|e| {
            LlmError::parse(format!(
                "failed to parse chat completion response JSON: {e}"
            ))
        })?;

        // Some gateways return HTTP 200 with a top-level error object.
        if let Some(err) = json.get("error") {
            return Err(LlmError::from_provider_error_value(err));
        }

        let usage = json.get("usage").map(TokenUsage::from_usage_value);

        let message = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| {
                LlmError::parse(format!(
                    "chat completion response missing choices[0].message: {}",
                    truncate_error_body(&json.to_string())
                ))
            })?;

        // Keep parity with the stream-assembled path (`assistant_message_from_stream`
        // omits empty `tool_calls`): strip `"tool_calls": []` so both paths
        // return the same shape for tool-less answers.
        let mut message = normalize_assistant_message(message);
        if message
            .get("tool_calls")
            .and_then(|t| t.as_array())
            .is_some_and(|a| a.is_empty())
        {
            message
                .as_object_mut()
                .and_then(|obj| obj.remove("tool_calls"));
        }
        warn_if_truncated_tool_arguments(&message);
        Ok((message, usage))
    }

    /// SSE chat completions — assemble full assistant message from deltas.
    ///
    /// Returns the message plus optional usage so the caller can log duration
    /// only after deciding the payload is usable (empty streams fall back).
    pub(super) async fn complete_streaming(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
    ) -> Result<(Value, Option<TokenUsage>), LlmError> {
        self.complete_streaming_with(messages, tools, opts, |_| {})
            .await
    }

    async fn complete_streaming_with<F>(
        &self,
        messages: &Value,
        tools: &Value,
        opts: &CompleteOptions,
        mut on_delta: F,
    ) -> Result<(Value, Option<TokenUsage>), LlmError>
    where
        F: FnMut(StreamDelta) + Send,
    {
        let body = self.request_body_with(messages, tools, true, opts);

        let req = self
            .http
            .post(self.chat_completions_url())
            .header("Accept", "text/event-stream")
            .json(&body);
        let req = self.apply_common_headers(req);

        let response = req.send().await.map_err(LlmError::from)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::from_http_status(status, &body));
        }

        let mut assembler = StreamAssembler::new();
        // Raw bytes, not `String` — network chunk boundaries don't align with
        // UTF-8 character boundaries, so decoding must wait for a complete line.
        let mut line_buf: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        let mut stream_error: Option<Value> = None;

        while let Some(chunk) = stream.next().await {
            // `From<reqwest::Error>` preserves the Timeout kind (connect /
            // deadline) instead of collapsing everything into `Http`, so
            // callers and fallback guards can distinguish a stall mid-stream.
            let chunk = chunk.map_err(LlmError::from)?;
            push_sse_bytes(&mut line_buf, &chunk, |data| {
                on_delta(ingest_sse_data(&mut assembler, data));
            });
            if let Some(err) = assembler.take_error() {
                stream_error = Some(err);
                break;
            }
        }
        if stream_error.is_none() {
            // Final event may omit trailing newline.
            flush_sse_buffer(&mut line_buf, |data| {
                on_delta(ingest_sse_data(&mut assembler, data));
            });
            stream_error = assembler.take_error();
        }

        if let Some(err) = stream_error {
            return Err(LlmError::from_provider_error_value(&err));
        }

        let usage = assembler.last_usage().cloned();
        let message = assembler.finish();
        warn_if_truncated_tool_arguments(&message);
        Ok((message, usage))
    }
}

/// Whether a stream failure may fall back to the blocking path.
///
/// Provider errors (auth, rate limit, bad model — HTTP 4xx/5xx surfaced as
/// [`LlmErrorKind::Provider`]) must NOT trigger a second request: the blocking
/// call would fail the same way while doubling load and burning rate-limit
/// budget. Transport / timeout / parse / unknown failures fall back, as does
/// an empty payload with no provider error (handled by the `Ok` arms above).
fn should_fallback_for_error(err: &LlmError) -> bool {
    !matches!(err.kind(), LlmErrorKind::Provider)
}

/// Best-effort truncated-arguments detector (`finish_reason == "length"`).
///
/// `finish_reason` is not always visible on the assembled message, so sniff
/// the tool-call arguments instead: any non-empty `function.arguments` string
/// that is not parseable JSON is suspicious (providers emit a JSON object
/// string for well-formed calls; a `length` stop slices it mid-token).
/// Never fails the turn — just warns so truncation shows in logs instead of
/// surfacing later as a confusing tool-schema error.
fn warn_if_truncated_tool_arguments(msg: &Value) {
    if !arguments_parseable(msg) {
        tracing::warn!("tool arguments unparseable (possibly truncated, finish_reason=length?)");
    }
}

/// True when every tool call's `function.arguments` parses as JSON.
///
/// Empty / missing argument strings are skipped (common for parameter-less
/// tools); only non-empty unparseable strings count as truncated.
fn arguments_parseable(msg: &Value) -> bool {
    let Some(calls) = msg.get("tool_calls").and_then(|t| t.as_array()) else {
        return true;
    };
    calls.iter().all(|tc| {
        match tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(|a| a.as_str())
        {
            None | Some("") => true,
            Some(s) => serde_json::from_str::<Value>(s).is_ok(),
        }
    })
}

/// Delta-side events for the blocking fallback path.
///
/// Emits `FirstDelta` + one `ContentDelta` with the full text when the final
/// message has non-empty content, plus `ToolCallComplete` per tool call —
/// mirroring the streaming success path's ordering (deltas, then `Usage`,
/// then `FinalMessage`, which the caller emits afterwards). Never emits
/// `ModelSend`: the surrounding `complete_stream` already sent it once.
fn emit_blocking_fallback_events(
    on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    message: &Value,
    elapsed_ms: u64,
) {
    if let Some(content) = message.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
            on_event(LlmStreamEvent::FirstDelta {
                ttfb_ms: elapsed_ms,
            });
            on_event(LlmStreamEvent::ContentDelta {
                text: content.to_string(),
            });
        }
    }
    emit_completed_tool_events(on_event, message);
}

fn emit_stream_delta(
    on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    delta: StreamDelta,
    started: Instant,
    first_delta_emitted: &mut bool,
) {
    // Fully-empty tool heartbeat fragments do not count as payload (see
    // `tool_delta_is_empty`): they must not trigger `FirstDelta` on their own.
    let has_payload = !delta.content.is_empty()
        || !delta.reasoning.is_empty()
        || delta.tool_deltas.iter().any(|t| !tool_delta_is_empty(t));
    if has_payload && !*first_delta_emitted {
        *first_delta_emitted = true;
        on_event(LlmStreamEvent::FirstDelta {
            ttfb_ms: started.elapsed().as_millis() as u64,
        });
    }
    if !delta.reasoning.is_empty() {
        on_event(LlmStreamEvent::ReasoningDelta {
            text: delta.reasoning,
        });
    }
    if !delta.content.is_empty() {
        on_event(LlmStreamEvent::ContentDelta {
            text: delta.content,
        });
    }
    for tool in delta.tool_deltas {
        on_event(LlmStreamEvent::ToolCallDelta {
            index: tool.index,
            id: tool.id,
            name: tool.name,
            arguments_delta: tool.arguments_delta,
        });
    }
    if let Some(usage) = delta.usage {
        on_event(LlmStreamEvent::Usage(usage));
    }
}

fn emit_completed_tool_events(on_event: &mut (dyn FnMut(LlmStreamEvent) + Send), msg: &Value) {
    if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
        for (index, tc) in tcs.iter().enumerate() {
            let id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let arguments = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            on_event(LlmStreamEvent::ToolCallComplete {
                index: index as u32,
                id,
                name,
                arguments,
            });
        }
    }
}

fn ingest_sse_data(assembler: &mut StreamAssembler, data: &str) -> StreamDelta {
    match serde_json::from_str::<Value>(data) {
        Ok(v) => assembler.ingest_event_delta(&v),
        Err(e) => {
            tracing::debug!(
                error = %e,
                payload = %truncate_error_body(data),
                "skipping invalid SSE JSON payload"
            );
            StreamDelta::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openrouter::sse::ToolDelta;

    #[test]
    fn typed_delta_events_are_incremental_and_first_delta_is_once() {
        let mut events = Vec::new();
        let mut first = false;
        let started = Instant::now();
        emit_stream_delta(
            &mut |event| events.push(event),
            StreamDelta {
                content: "Hel".into(),
                reasoning: String::new(),
                tool_deltas: vec![],
                usage: None,
            },
            started,
            &mut first,
        );
        emit_stream_delta(
            &mut |event| events.push(event),
            StreamDelta {
                content: "lo".into(),
                reasoning: String::new(),
                tool_deltas: vec![ToolDelta {
                    index: 0,
                    id: Some("c1".into()),
                    name: Some("get_time".into()),
                    arguments_delta: "{}".into(),
                }],
                usage: Some(TokenUsage {
                    prompt_tokens: 2,
                    completion_tokens: 1,
                    total_tokens: 3,
                    ..Default::default()
                }),
            },
            started,
            &mut first,
        );

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, LlmStreamEvent::FirstDelta { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.get(1),
            Some(LlmStreamEvent::ContentDelta { text }) if text == "Hel"
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            LlmStreamEvent::ToolCallDelta { name: Some(name), arguments_delta, .. }
                if name == "get_time" && arguments_delta == "{}"
        )));
        assert!(events
            .iter()
            .any(|event| matches!(event, LlmStreamEvent::Usage(u) if u.total_tokens == 3)));
    }

    #[test]
    fn usage_only_event_is_not_mislabeled_as_first_content_delta() {
        let mut events = Vec::new();
        let mut first = false;
        emit_stream_delta(
            &mut |event| events.push(event),
            StreamDelta {
                content: String::new(),
                reasoning: String::new(),
                tool_deltas: vec![],
                usage: Some(TokenUsage {
                    total_tokens: 1,
                    ..Default::default()
                }),
            },
            Instant::now(),
            &mut first,
        );
        assert!(!first);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], LlmStreamEvent::Usage(_)));
    }

    #[test]
    fn provider_errors_do_not_fall_back() {
        assert!(!should_fallback_for_error(&LlmError::provider(
            "rate limited"
        )));
        assert!(!should_fallback_for_error(&LlmError::from_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "slow down"
        )));
        assert!(should_fallback_for_error(&LlmError::http("conn reset")));
        assert!(should_fallback_for_error(&LlmError::timeout("deadline")));
        assert!(should_fallback_for_error(&LlmError::parse("bad json")));
        assert!(should_fallback_for_error(&LlmError::new("misc")));
    }

    #[test]
    fn arguments_parseable_sniffs_truncation() {
        assert!(arguments_parseable(&serde_json::json!({ "content": "hi" })));
        assert!(arguments_parseable(&serde_json::json!({
            "tool_calls": [{
                "function": { "name": "bash", "arguments": "{\"c\":\"ls\"}" }
            }]
        })));
        // Empty args (parameter-less tools) are not truncation.
        assert!(arguments_parseable(&serde_json::json!({
            "tool_calls": [{ "function": { "name": "get_time", "arguments": "" } }]
        })));
        // Sliced mid-token JSON is suspicious.
        assert!(!arguments_parseable(&serde_json::json!({
            "tool_calls": [{ "function": { "name": "bash", "arguments": "{\"c\":" } }]
        })));
    }

    #[test]
    fn empty_tool_heartbeat_does_not_trigger_first_delta() {
        let mut events = Vec::new();
        let mut first = false;
        emit_stream_delta(
            &mut |event| events.push(event),
            StreamDelta {
                content: String::new(),
                reasoning: String::new(),
                tool_deltas: vec![ToolDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments_delta: String::new(),
                }],
                usage: None,
            },
            Instant::now(),
            &mut first,
        );
        assert!(!first);
        assert!(!events
            .iter()
            .any(|event| matches!(event, LlmStreamEvent::FirstDelta { .. })));
    }

    #[test]
    fn blocking_fallback_emits_first_delta_content_and_tools_without_model_send() {
        let mut events = Vec::new();
        emit_blocking_fallback_events(
            &mut |event| events.push(event),
            &serde_json::json!({
                "role": "assistant",
                "content": "fallback text",
                "tool_calls": [{
                    "id": "c1",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            }),
            42,
        );
        assert!(matches!(
            events.as_slice(),
            [
                LlmStreamEvent::FirstDelta { ttfb_ms: 42 },
                LlmStreamEvent::ContentDelta { .. },
                LlmStreamEvent::ToolCallComplete { .. },
            ]
        ));
        assert!(events
            .iter()
            .all(|event| !matches!(event, LlmStreamEvent::ModelSend { .. })));
        assert!(matches!(
            events.get(1),
            Some(LlmStreamEvent::ContentDelta { text }) if text == "fallback text"
        ));
    }

    #[test]
    fn blocking_fallback_with_empty_content_emits_no_first_delta() {
        let mut events = Vec::new();
        emit_blocking_fallback_events(
            &mut |event| events.push(event),
            &serde_json::json!({ "role": "assistant", "content": "" }),
            7,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn reasoning_delta_counts_as_first_byte() {
        let mut events = Vec::new();
        let mut first = false;
        emit_stream_delta(
            &mut |event| events.push(event),
            StreamDelta {
                content: String::new(),
                reasoning: "Let me think.".into(),
                tool_deltas: vec![],
                usage: None,
            },
            Instant::now(),
            &mut first,
        );
        assert!(first);
        assert!(matches!(
            events.as_slice(),
            [
                LlmStreamEvent::FirstDelta { .. },
                LlmStreamEvent::ReasoningDelta { text }
            ] if text == "Let me think."
        ));
    }
}
