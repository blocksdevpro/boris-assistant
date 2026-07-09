use reqwest::blocking::Client;
use serde_json::{json, Value};

use crate::error::LlmError;

// ── Trait ────────────────────────────────────────────────────────────────────

/// Abstracts the HTTP transport so Engine stays provider-agnostic and testable.
pub trait LlmClient: Send {
    /// Send the full conversation + tool definitions and return the raw
    /// `choices[0].message` JSON object.
    fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;
}

// ── OpenRouter implementation ─────────────────────────────────────────────────

pub struct OpenRouterClient {
    api_key: String,
    model: String,
    client: Client,
}

impl OpenRouterClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            model: "google/gemini-2.5-flash-lite".to_string(),
            client: Client::new(),
        }
    }

    /// Override the default model (builder-style).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

impl LlmClient for OpenRouterClient {
    fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        });

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .map_err(|e| LlmError::new(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(LlmError::new(format!(
                "OpenRouter returned {status}: {body}"
            )));
        }

        let json: Value = response
            .json()
            .map_err(|e| LlmError::new(format!("failed to parse response JSON: {e}")))?;

        let message = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| LlmError::new(format!("missing choices[0].message: {json}")))?;

        Ok(message)
    }
}
