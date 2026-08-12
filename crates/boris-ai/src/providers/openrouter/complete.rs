//! `complete` paths: stream-first with non-stream fallback.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::LlmClient;
use crate::error::{truncate_error_body, LlmError};
use crate::message::{message_has_usable_payload, normalize_assistant_message};
use crate::usage::{log_usage, TokenUsage};

use super::client::OpenRouterClient;
use super::sse::{flush_sse_buffer, push_sse_bytes, StreamAssembler};

#[async_trait]
impl LlmClient for OpenRouterClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        // Prefer SSE: lower TTFB; tools can start as soon as the body is assembled.
        //
        // Some OpenRouter/Gemini streams return HTTP 200 with zero content deltas
        // (thinking-only / empty assembly). Treat that as failure and fall back
        // so the agent does not go Silent with nothing to say.
        //
        // Clone messages/tools only when falling back — streaming takes references.
        match self.complete_streaming(&messages, &tools).await {
            Ok(msg) if message_has_usable_payload(&msg) => Ok(msg),
            Ok(msg) => {
                tracing::warn!(
                    content_preview = %msg.get("content").map(|c| c.to_string()).unwrap_or_default(),
                    has_tools = crate::message::has_tool_calls(&msg),
                    "stream complete returned empty payload; falling back to non-stream"
                );
                self.complete_blocking(&messages, &tools).await
            }
            Err(e) => {
                tracing::debug!(error = %e, "stream complete failed; falling back to non-stream");
                self.complete_blocking(&messages, &tools).await
            }
        }
    }

    fn model(&self) -> &str {
        OpenRouterClient::model(self)
    }
}

impl OpenRouterClient {
    /// Non-streaming JSON response path.
    pub(super) async fn complete_blocking(
        &self,
        messages: &Value,
        tools: &Value,
    ) -> Result<Value, LlmError> {
        let body = self.request_body(messages, tools, false);

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

        if let Some(usage) = json.get("usage") {
            log_usage(
                &self.model,
                &TokenUsage::from_usage_value(usage),
                "blocking",
            );
        }

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

        Ok(normalize_assistant_message(message))
    }

    /// SSE chat completions — assemble full assistant message from deltas.
    pub(super) async fn complete_streaming(
        &self,
        messages: &Value,
        tools: &Value,
    ) -> Result<Value, LlmError> {
        let body = self.request_body(messages, tools, true);

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
            let chunk = chunk.map_err(|e| LlmError::http(format!("stream read: {e}")))?;
            push_sse_bytes(&mut line_buf, &chunk, |data| {
                ingest_sse_data(&mut assembler, data);
            });
            if let Some(err) = assembler.take_error() {
                stream_error = Some(err);
                break;
            }
        }
        if stream_error.is_none() {
            // Final event may omit trailing newline.
            flush_sse_buffer(&mut line_buf, |data| {
                ingest_sse_data(&mut assembler, data);
            });
            stream_error = assembler.take_error();
        }

        if let Some(err) = stream_error {
            return Err(LlmError::from_provider_error_value(&err));
        }

        if let Some(usage) = assembler.last_usage() {
            log_usage(&self.model, usage, "stream");
        }

        Ok(assembler.finish())
    }
}

fn ingest_sse_data(assembler: &mut StreamAssembler, data: &str) {
    match serde_json::from_str::<Value>(data) {
        Ok(v) => assembler.ingest_event(&v),
        Err(e) => {
            tracing::debug!(
                error = %e,
                payload = %truncate_error_body(data),
                "skipping invalid SSE JSON payload"
            );
        }
    }
}
