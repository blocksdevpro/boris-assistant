//! `complete` paths: stream-first with non-stream fallback.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;

use crate::client::LlmClient;
use crate::error::LlmError;
use crate::message::{message_has_usable_payload, normalize_assistant_message};
use crate::usage::{log_usage, TokenUsage};

use super::client::OpenRouterClient;
use super::request::CHAT_COMPLETIONS_URL;
use super::sse::{push_sse_bytes, StreamAssembler};

#[async_trait]
impl LlmClient for OpenRouterClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        // Prefer SSE: lower TTFB; tools can start as soon as the body is assembled.
        //
        // Some OpenRouter/Gemini streams return HTTP 200 with zero content deltas
        // (thinking-only / empty assembly). Treat that as failure and fall back
        // so the agent does not go Silent with nothing to say.
        match self
            .complete_streaming(messages.clone(), tools.clone())
            .await
        {
            Ok(msg) if message_has_usable_payload(&msg) => Ok(msg),
            Ok(msg) => {
                tracing::warn!(
                    content_preview = %msg.get("content").map(|c| c.to_string()).unwrap_or_default(),
                    has_tools = crate::message::has_tool_calls(&msg),
                    "stream complete returned empty payload; falling back to non-stream"
                );
                self.complete_blocking(messages, tools).await
            }
            Err(e) => {
                tracing::debug!(error = %e, "stream complete failed; falling back to non-stream");
                self.complete_blocking(messages, tools).await
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
        messages: Value,
        tools: Value,
    ) -> Result<Value, LlmError> {
        let body = self.request_body(messages, tools, false);

        let response = self
            .http
            .post(CHAT_COMPLETIONS_URL)
            .header("Authorization", self.authorization_header())
            .json(&body)
            .send()
            .await
            .map_err(LlmError::from)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::http(format!(
                "OpenRouter HTTP error {status}: {body}"
            )));
        }

        let json: Value = response.json().await.map_err(|e| {
            LlmError::parse(format!("failed to parse OpenRouter response JSON: {e}"))
        })?;

        if let Some(usage) = json.get("usage") {
            log_usage(&self.model, &TokenUsage::from_usage_value(usage), "blocking");
        }

        let message = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| {
                LlmError::parse(format!(
                    "OpenRouter response missing choices[0].message: {json}"
                ))
            })?;

        Ok(normalize_assistant_message(message))
    }

    /// SSE chat completions — assemble full assistant message from deltas.
    pub(super) async fn complete_streaming(
        &self,
        messages: Value,
        tools: Value,
    ) -> Result<Value, LlmError> {
        let body = self.request_body(messages, tools, true);

        let mut req = self
            .http
            .post(CHAT_COMPLETIONS_URL)
            .header("Authorization", self.authorization_header())
            .header("Accept", "text/event-stream")
            .json(&body);

        if let Some(sid) = self.session_id.as_deref() {
            // Header form is also supported; body session_id takes precedence if both set.
            req = req.header("x-session-id", sid);
        }

        let response = req.send().await.map_err(LlmError::from)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::http(format!(
                "OpenRouter stream HTTP error {status}: {body}"
            )));
        }

        let mut assembler = StreamAssembler::new();
        let mut line_buf = String::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LlmError::http(format!("stream read: {e}")))?;
            push_sse_bytes(&mut line_buf, &chunk, |data| {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    assembler.ingest_event(&v);
                }
            });
        }

        if let Some(usage) = assembler.last_usage() {
            log_usage(&self.model, usage, "stream");
        }

        Ok(assembler.finish())
    }
}
