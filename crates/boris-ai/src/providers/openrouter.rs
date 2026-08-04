use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use crate::client::LlmClient;
use crate::error::LlmError;

/// Default TCP connect timeout for OpenRouter requests.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default overall request timeout (connect + TTFB + body) for OpenRouter.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// OpenRouter Chat Completions client.
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

#[async_trait]
impl LlmClient for OpenRouterClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        // Prefer SSE stream path: lower TTFB; tools start as soon as the body is assembled
        // (no waiting on a single giant JSON buffer when the provider streams early).
        match self.complete_streaming(messages.clone(), tools.clone()).await {
            Ok(msg) => Ok(msg),
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
    async fn complete_blocking(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = Self::request_body(&self.model, messages, tools, false);

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(Self::map_request_error)?;

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

    /// SSE chat completions — assemble full assistant message from deltas.
    async fn complete_streaming(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        use futures_util::StreamExt;

        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = Self::request_body(&self.model, messages, tools, true);

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(Self::map_request_error)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::http(format!(
                "OpenRouter stream HTTP error {status}: {body}"
            )));
        }

        let mut content = String::new();
        // index -> (id, name, arguments)
        let mut tool_acc: std::collections::BTreeMap<u32, (String, String, String)> =
            std::collections::BTreeMap::new();
        let mut role = "assistant".to_string();

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LlmError::http(format!("stream read: {e}")))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim_end_matches('\r').to_string();
                buffer.drain(..=pos);
                if !line.starts_with("data:") {
                    continue;
                }
                let data = line[5..].trim();
                if data.is_empty() || data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
                let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else {
                    continue;
                };
                if let Some(delta) = choice.get("delta") {
                    if let Some(r) = delta.get("role").and_then(|r| r.as_str()) {
                        role = r.to_string();
                    }
                    if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
                        content.push_str(c);
                    }
                    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                            let entry = tool_acc.entry(idx).or_insert_with(|| {
                                (
                                    String::new(),
                                    String::new(),
                                    String::new(),
                                )
                            });
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                if !id.is_empty() {
                                    entry.0 = id.to_string();
                                }
                            }
                            if let Some(func) = tc.get("function") {
                                if let Some(n) = func.get("name").and_then(|n| n.as_str()) {
                                    if !n.is_empty() {
                                        entry.1.push_str(n);
                                    }
                                }
                                if let Some(a) = func.get("arguments").and_then(|a| a.as_str()) {
                                    entry.2.push_str(a);
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut message = json!({
            "role": role,
            "content": if content.is_empty() { Value::Null } else { Value::String(content) },
        });
        if !tool_acc.is_empty() {
            let tools: Vec<Value> = tool_acc
                .into_iter()
                .map(|(idx, (id, name, arguments))| {
                    let id = if id.is_empty() {
                        format!("call_{idx}")
                    } else {
                        id
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
            message
                .as_object_mut()
                .unwrap()
                .insert("tool_calls".into(), Value::Array(tools));
        }
        Ok(message)
    }

    fn request_body(model: &str, messages: Value, tools: Value, stream: bool) -> Value {
        let mut body = if tools.is_null() || tools.as_array().is_some_and(|a| a.is_empty()) {
            json!({
                "model": model,
                "messages": messages,
            })
        } else {
            json!({
                "model": model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto",
            })
        };
        if stream {
            body.as_object_mut()
                .unwrap()
                .insert("stream".into(), json!(true));
        }
        body
    }
}
