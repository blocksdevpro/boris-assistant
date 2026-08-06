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
        //
        // Critical: some OpenRouter/Gemini streams return HTTP 200 with zero content
        // deltas (thinking-only / empty assembly). Treat that as failure and fall back
        // to the non-stream path so the agent does not go Silent with nothing to say.
        match self.complete_streaming(messages.clone(), tools.clone()).await {
            Ok(msg) if message_has_usable_payload(&msg) => Ok(msg),
            Ok(msg) => {
                tracing::warn!(
                    content_preview = %msg.get("content").map(|c| c.to_string()).unwrap_or_default(),
                    has_tools = msg.get("tool_calls").and_then(|t| t.as_array()).map(|a| !a.is_empty()).unwrap_or(false),
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

/// True when an assistant message has speakable text and/or tool calls.
fn message_has_usable_payload(msg: &Value) -> bool {
    let has_tools = msg
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty());
    if has_tools {
        return true;
    }
    !extract_text_content(msg.get("content").unwrap_or(&Value::Null)).is_empty()
}

/// Pull plain text from OpenAI-style `content` (string or multimodal parts array).
fn extract_text_content(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => {
            let mut out = String::new();
            for p in parts {
                if let Some(s) = p.as_str() {
                    out.push_str(s);
                    continue;
                }
                if let Some(s) = p.get("text").and_then(|t| t.as_str()) {
                    out.push_str(s);
                    continue;
                }
                // Gemini-style: { "type": "text", "text": "..." }
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(s) = p.get("text").and_then(|t| t.as_str()) {
                        out.push_str(s);
                    }
                }
            }
            out.trim().to_string()
        }
        Value::Null => String::new(),
        other => {
            // Last resort: ignore objects/bools that are not speakable.
            if other.is_object() || other.is_boolean() || other.is_number() {
                String::new()
            } else {
                other.to_string().trim().to_string()
            }
        }
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

        let mut message = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .cloned()
            .ok_or_else(|| {
                LlmError::parse(format!(
                    "OpenRouter response missing choices[0].message (parse error): {json}"
                ))
            })?;

        // Normalize content parts array → plain string so the agent loop can always
        // read `response["content"].as_str()`.
        if let Some(obj) = message.as_object_mut() {
            if let Some(c) = obj.get("content").cloned() {
                if !c.is_string() && !c.is_null() {
                    let text = extract_text_content(&c);
                    obj.insert("content".into(), Value::String(text));
                } else if c.is_null() {
                    obj.insert("content".into(), Value::String(String::new()));
                }
            }
        }

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
                // Prefer incremental delta; some providers also emit a full `message`.
                let piece = choice.get("delta").or_else(|| choice.get("message"));
                if let Some(delta) = piece {
                    if let Some(r) = delta.get("role").and_then(|r| r.as_str()) {
                        role = r.to_string();
                    }
                    // content may be a string or a parts array (Gemini / multimodal).
                    if let Some(c) = delta.get("content") {
                        let chunk = extract_text_content(c);
                        // extract_text_content trims; for streaming pieces preserve raw string
                        // when it is a plain string so we don't drop intentional spaces.
                        if let Some(s) = c.as_str() {
                            content.push_str(s);
                        } else if !chunk.is_empty() {
                            content.push_str(&chunk);
                        }
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

        // Prefer empty string over null: several OpenRouter providers reject
        // `content: null` on assistant messages (including tool-call turns).
        let mut message = json!({
            "role": role,
            "content": content,
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
