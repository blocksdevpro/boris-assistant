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
///
/// Supports optional **model-provider** routing (`provider.order`) so hosts can
/// pin to inference endpoints like CoreWeave / Baseten / SiliconFlow — not the
/// LLM author (Google/OpenAI), but the OpenRouter hosting provider for that model.
///
/// When a `session_id` is set, OpenRouter sticky-routes turns to the same
/// endpoint to maximize **prompt-cache hits** (`usage.prompt_tokens_details.cached_tokens`).
pub struct OpenRouterClient {
    api_key: String,
    model: String,
    /// OpenRouter provider slugs tried in order (e.g. `["coreweave", "baseten"]`).
    provider_order: Vec<String>,
    /// When `provider_order` is set: whether to fall back to other providers.
    allow_fallbacks: bool,
    /// Sticky routing key for cache-friendly multi-turn sessions.
    session_id: Option<String>,
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

    /// Prefer specific OpenRouter **model-providers** (inference hosts) in order.
    ///
    /// Empty list → OpenRouter default load-balancing / sticky routing.
    /// Example: `["coreweave", "baseten"]` matches the Provider column on model pages.
    pub fn with_provider_order(mut self, order: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.provider_order = order.into_iter().map(|s| s.into()).collect();
        self
    }

    /// Parse a free-form provider string (comma/space separated) into `provider.order`.
    ///
    /// Accepts slugs as shown on OpenRouter (`coreweave`, `deepinfra/turbo`, `novita`).
    /// Empty / whitespace → no preference.
    pub fn with_provider_pref(mut self, raw: impl AsRef<str>) -> Self {
        self.provider_order = parse_provider_list(raw.as_ref());
        self
    }

    /// Whether OpenRouter may try other providers when the preferred list fails.
    /// Default `true`. Set `false` to hard-pin to `provider_order` only.
    pub fn with_allow_fallbacks(mut self, allow: bool) -> Self {
        self.allow_fallbacks = allow;
        self
    }

    /// Session id for OpenRouter sticky routing (prompt-cache hits across turns).
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        let s = session_id.into();
        self.session_id = if s.trim().is_empty() {
            None
        } else {
            Some(s)
        };
        self
    }

    /// Model id configured for this client.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Configured OpenRouter model-provider order (may be empty).
    pub fn provider_order(&self) -> &[String] {
        &self.provider_order
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
            provider_order: Vec::new(),
            allow_fallbacks: true,
            session_id: None,
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

/// Split a user-facing provider preference into OpenRouter slugs.
///
/// Accepts comma and/or whitespace separated lists:
/// `coreweave, baseten` → `["coreweave", "baseten"]`.
pub fn parse_provider_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Optional split of `model@provider` / `model|provider` into (model, provider_pref).
///
/// Provider-only fields in settings take precedence when both are set; this is a
/// convenience so a single string can carry both.
pub fn split_model_and_provider(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // Prefer `@` (common "model@host" form); also accept `|`.
    for sep in ['@', '|'] {
        if let Some((model, provider)) = raw.split_once(sep) {
            let model = model.trim();
            let provider = provider.trim();
            if !model.is_empty() && !provider.is_empty() {
                return (model.to_string(), Some(provider.to_string()));
            }
        }
    }
    (raw.to_string(), None)
}

/// Token usage extracted from an OpenRouter response (cache-aware).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    /// Prompt tokens served from provider cache (`prompt_tokens_details.cached_tokens`).
    pub cached_tokens: u64,
    /// Tokens written into cache this request (when reported).
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    pub fn from_usage_value(usage: &Value) -> Self {
        let prompt_tokens = usage
            .get("prompt_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let completion_tokens = usage
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total_tokens = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(prompt_tokens.saturating_add(completion_tokens));
        let details = usage.get("prompt_tokens_details");
        let cached_tokens = details
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_write_tokens = details
            .and_then(|d| d.get("cache_write_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            cache_write_tokens,
        }
    }

    pub fn cache_hit(&self) -> bool {
        self.cached_tokens > 0
    }
}

fn log_usage(model: &str, usage: &TokenUsage, path: &str) {
    if usage.total_tokens == 0 && usage.cached_tokens == 0 {
        return;
    }
    if usage.cache_hit() {
        tracing::info!(
            model = %model,
            path,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            cached_tokens = usage.cached_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            "OpenRouter usage (cache hit)"
        );
    } else {
        tracing::debug!(
            model = %model,
            path,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            "OpenRouter usage"
        );
    }
}

impl OpenRouterClient {
    async fn complete_blocking(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = self.request_body(messages, tools, false);

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

        if let Some(usage) = json.get("usage") {
            log_usage(&self.model, &TokenUsage::from_usage_value(usage), "blocking");
        }

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
        let body = self.request_body(messages, tools, true);

        let mut req = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Accept", "text/event-stream")
            .json(&body);
        if let Some(sid) = self.session_id.as_deref() {
            // Header form is also supported; body session_id takes precedence if both set.
            req = req.header("x-session-id", sid);
        }

        let response = req.send().await.map_err(Self::map_request_error)?;

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
        let mut last_usage: Option<TokenUsage> = None;

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
                if let Some(usage) = v.get("usage") {
                    last_usage = Some(TokenUsage::from_usage_value(usage));
                }
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
                                (String::new(), String::new(), String::new())
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

        if let Some(usage) = last_usage.as_ref() {
            log_usage(&self.model, usage, "stream");
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

    fn request_body(&self, messages: Value, tools: Value, stream: bool) -> Value {
        let mut body = if tools.is_null() || tools.as_array().is_some_and(|a| a.is_empty()) {
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
        let obj = body.as_object_mut().unwrap();
        if stream {
            obj.insert("stream".into(), json!(true));
            // Final SSE event includes usage (incl. cached_tokens) when supported.
            obj.insert(
                "stream_options".into(),
                json!({ "include_usage": true }),
            );
        }
        if !self.provider_order.is_empty() {
            obj.insert(
                "provider".into(),
                json!({
                    "order": self.provider_order,
                    "allow_fallbacks": self.allow_fallbacks,
                }),
            );
        }
        if let Some(sid) = self.session_id.as_deref() {
            if !sid.is_empty() {
                obj.insert("session_id".into(), json!(sid));
            }
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_list_comma_and_space() {
        assert_eq!(
            parse_provider_list("coreweave, Baseten  siliconflow"),
            vec![
                "coreweave".to_string(),
                "baseten".to_string(),
                "siliconflow".to_string()
            ]
        );
        assert!(parse_provider_list("  ").is_empty());
    }

    #[test]
    fn split_model_at_provider() {
        let (m, p) = split_model_and_provider("google/gemini-2.5-flash-lite@coreweave");
        assert_eq!(m, "google/gemini-2.5-flash-lite");
        assert_eq!(p.as_deref(), Some("coreweave"));

        let (m, p) = split_model_and_provider("openai/gpt-4o|deepinfra/turbo");
        assert_eq!(m, "openai/gpt-4o");
        assert_eq!(p.as_deref(), Some("deepinfra/turbo"));

        let (m, p) = split_model_and_provider("google/gemini-2.5-flash-lite");
        assert_eq!(m, "google/gemini-2.5-flash-lite");
        assert!(p.is_none());
    }

    #[test]
    fn request_body_includes_provider_and_session() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_provider_pref("coreweave, baseten")
            .with_allow_fallbacks(false)
            .with_session_id("sess-1");
        let body = client.request_body(json!([]), Value::Null, true);
        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(
            body["provider"]["order"],
            json!(["coreweave", "baseten"])
        );
        assert_eq!(body["provider"]["allow_fallbacks"], false);
        assert_eq!(body["session_id"], "sess-1");
    }

    #[test]
    fn token_usage_parses_cached_tokens() {
        let usage = json!({
            "prompt_tokens": 1000,
            "completion_tokens": 50,
            "total_tokens": 1050,
            "prompt_tokens_details": {
                "cached_tokens": 900,
                "cache_write_tokens": 100
            }
        });
        let u = TokenUsage::from_usage_value(&usage);
        assert_eq!(u.cached_tokens, 900);
        assert_eq!(u.cache_write_tokens, 100);
        assert!(u.cache_hit());
    }
}
