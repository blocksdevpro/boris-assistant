use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::{json, Value};

use crate::error::LlmError;

// ── Timeouts ─────────────────────────────────────────────────────────────────

/// Default TCP connect timeout for OpenRouter requests.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default overall request timeout (connect + TTFB + body) for OpenRouter.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

// ── Trait ────────────────────────────────────────────────────────────────────

/// Abstracts the HTTP transport so Engine stays provider-agnostic and testable.
pub trait LlmClient: Send {
    /// Send the full conversation + tool definitions and return the raw
    /// `choices[0].message` JSON object.
    fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;

    /// Model identifier used for this client (for logging / turn metadata).
    /// Defaults to `"unknown"` when a mock or adapter does not override it.
    fn model(&self) -> &str {
        "unknown"
    }
}

// ── OpenRouter implementation ─────────────────────────────────────────────────

pub struct OpenRouterClient {
    api_key: String,
    model: String,
    client: Client,
}

impl OpenRouterClient {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self::build(api_key, model, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT)
    }

    /// Override connect and overall request timeouts (builder-style).
    ///
    /// Rebuilds the underlying `reqwest` client. Falls back to
    /// `Client::new()` (no custom timeouts) only if builder construction fails.
    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.client = Client::builder()
            .connect_timeout(connect)
            .timeout(total)
            .build()
            .unwrap_or_else(|_| Client::new());
        self
    }

    /// Override the default model (builder-style).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Model id configured for this client.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn build(
        api_key: String,
        model: Option<String>,
        connect_timeout: Duration,
        timeout: Duration,
    ) -> Self {
        // Explicit connect/read timeouts so a stalled OpenRouter call cannot
        // leave the session FSM stuck in Thinking forever.
        let client = Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            api_key,
            model: model.unwrap_or_else(|| "google/gemini-2.5-flash-lite".to_string()),
            client,
        }
    }

    /// Map reqwest errors into clear, classified `LlmError` kinds.
    fn map_request_error(err: reqwest::Error) -> LlmError {
        if err.is_timeout() {
            return LlmError::timeout(format!(
                "OpenRouter request timed out (connect or overall timeout): {err}"
            ));
        }
        if err.is_connect() {
            return LlmError::http(format!(
                "OpenRouter connection failed (connect timeout or network): {err}"
            ));
        }
        LlmError::http(format!("HTTP request failed: {err}"))
    }
}

impl LlmClient for OpenRouterClient {
    fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";

        // Only advertise tools when the engine actually registered some.
        // Empty `tools: []` + `tool_choice: auto` confuses some providers.
        let body = if tools.is_null() || tools.as_array().is_some_and(|a| a.is_empty()) {
            json!({
                "model": self.model,
                "messages": messages,
            })
        } else {
            json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto",
            })
        };

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .map_err(Self::map_request_error)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(LlmError::http(format!(
                "OpenRouter HTTP error {status}: {body}"
            )));
        }

        let json: Value = response.json().map_err(|e| {
            LlmError::parse(format!("failed to parse OpenRouter response JSON: {e}"))
        })?;

        let message = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| {
                LlmError::parse(format!(
                    "OpenRouter response missing choices[0].message (parse error): {json}"
                ))
            })?;

        Ok(message)
    }

    fn model(&self) -> &str {
        OpenRouterClient::model(self)
    }
}
