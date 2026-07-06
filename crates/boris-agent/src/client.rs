use reqwest::blocking::Client;
use serde_json::{Value, json};

// ── Trait ────────────────────────────────────────────────────────────────────

/// Abstracts the HTTP transport so Engine stays provider-agnostic and testable.
pub trait LlmClient: Send {
    /// Send the full conversation + tool definitions and return the raw
    /// `choices[0].message` JSON object.
    fn complete(&self, messages: Value, tools: Value) -> Value;
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
    fn complete(&self, messages: Value, tools: Value) -> Value {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            // "auto" lets the model decide when to call a tool vs reply directly.
            "tool_choice": "auto",
        });

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .unwrap();

        let json: Value = response.json().unwrap();
        json["choices"][0]["message"].clone()
    }
}
