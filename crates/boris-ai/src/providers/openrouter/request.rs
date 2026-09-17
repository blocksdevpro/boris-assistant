//! Request body construction for OpenRouter chat completions.

use serde_json::{json, Value};

use super::client::{OpenRouterClient, DEFAULT_BASE_URL};

use crate::request::{clamp_to_context_window, CompleteOptions};

impl OpenRouterClient {
    /// True when this client targets a non-OpenRouter OpenAI-compatible
    /// endpoint (`base_url != [`DEFAULT_BASE_URL`]` after normalizing
    /// trailing slashes).
    ///
    /// When true, [`Self::request_body_with`] strips OpenRouter-only
    /// extensions (`reasoning`, `provider`, `session_id`, `stream_options`,
    /// `parallel_tool_calls`) so plain OpenAI-compatible gateways (e.g.
    /// local `llama.cpp` / Ollama / vLLM servers) don't 400 on unknown
    /// fields. `tool_choice: "auto"` is kept — it is OpenAI-standard.
    /// Default-base-URL behavior is unchanged.
    pub fn should_strip_openrouter_extensions(&self) -> bool {
        trim_trailing_slash(&self.base_url) != trim_trailing_slash(DEFAULT_BASE_URL)
    }

    /// Build the JSON body for a chat completion request.
    ///
    /// Clones `messages` / `tools` into the body once per call.
    #[allow(dead_code)]
    pub(super) fn request_body(&self, messages: &Value, tools: &Value, stream: bool) -> Value {
        self.request_body_with(messages, tools, stream, &CompleteOptions::default())
    }

    pub(super) fn request_body_with(
        &self,
        messages: &Value,
        tools: &Value,
        stream: bool,
        opts: &CompleteOptions,
    ) -> Value {
        let strip = self.should_strip_openrouter_extensions();
        let mut body = base_body(&self.model, messages, tools, strip);
        let obj = body
            .as_object_mut()
            .expect("chat completion body is always a JSON object");

        let raw_max = opts.resolved_max_tokens(self.max_tokens);
        let max_tokens =
            clamp_to_context_window(raw_max, Some(OpenRouterClient::context_window_tokens(self)));
        if max_tokens != raw_max {
            tracing::warn!(
                requested = raw_max,
                clamped = max_tokens,
                context_window = OpenRouterClient::context_window_tokens(self),
                "max_tokens exceeds model context window; clamping"
            );
        }
        let reasoning = opts.resolved_reasoning(self.reasoning.clone());

        // Headroom for reasoning + final answer / tool_calls.
        // Dual-send both keys: OpenRouter accepts either, and plain
        // OpenAI-compatible gateways only know `max_completion_tokens`
        // (unknown keys are ignored by OpenAI-style servers).
        obj.insert("max_tokens".into(), json!(max_tokens));
        obj.insert("max_completion_tokens".into(), json!(max_tokens));

        // Unified OpenRouter reasoning (DeepSeek / Gemini thinking / Claude / o-series).
        // Omitted for non-OpenRouter endpoints (unknown field there).
        if !strip {
            obj.insert("reasoning".into(), reasoning.to_request_value());
        }

        if let Some(temperature) = opts.resolved_temperature(None) {
            obj.insert("temperature".into(), json!(temperature));
        }

        if stream {
            obj.insert("stream".into(), json!(true));
            // Final SSE event includes usage (incl. cached_tokens) when supported.
            // OpenRouter-only; stripped for plain OpenAI-compatible endpoints.
            if !strip {
                obj.insert("stream_options".into(), json!({ "include_usage": true }));
            }
        }

        if !strip {
            if !self.provider_order.is_empty() {
                obj.insert(
                    "provider".into(),
                    json!({
                        "order": self.provider_order,
                        "allow_fallbacks": self.allow_fallbacks,
                    }),
                );
            }

            if let Some(sid) = self.session_id.as_deref().filter(|s| !s.is_empty()) {
                obj.insert("session_id".into(), json!(sid));
            }
        }

        body
    }
}

fn trim_trailing_slash(url: &str) -> &str {
    url.trim_end_matches('/')
}

fn base_body(model: &str, messages: &Value, tools: &Value, strip_openrouter_extensions: bool) -> Value {
    if tools_absent_or_empty(tools) {
        json!({
            "model": model,
            "messages": messages,
        })
    } else if strip_openrouter_extensions {
        // `tool_choice: "auto"` is OpenAI-standard and kept; the non-standard
        // `parallel_tool_calls` nudge is dropped for third-party gateways.
        json!({
            "model": model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
        })
    } else {
        // parallel_tool_calls nudges OpenAI-compatible providers to emit multi-tool messages.
        json!({
            "model": model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "parallel_tool_calls": true,
        })
    }
}

fn tools_absent_or_empty(tools: &Value) -> bool {
    tools.is_null() || tools.as_array().is_some_and(|a| a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openrouter::OpenRouterClient;

    #[test]
    fn request_body_includes_provider_session_and_stream_opts() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_provider_pref("coreweave, baseten")
            .with_allow_fallbacks(false)
            .with_session_id("sess-1");
        let body = client.request_body(&json!([]), &Value::Null, true);
        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["provider"]["order"], json!(["coreweave", "baseten"]));
        assert_eq!(body["provider"]["allow_fallbacks"], false);
        assert_eq!(body["session_id"], "sess-1");
        assert!(body.get("tools").is_none());
        // Default: high reasoning + completion headroom.
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["enabled"], true);
        assert!(body["max_tokens"].as_u64().unwrap() >= 8_000);
    }

    #[test]
    fn request_body_includes_tools_when_present() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()));
        let tools = json!([{ "type": "function", "function": { "name": "x" } }]);
        let body = client.request_body(&json!([]), &tools, false);
        assert!(body.get("stream").is_none());
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert!(body["tools"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn request_body_respects_reasoning_override() {
        use crate::providers::openrouter::ReasoningConfig;
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_reasoning(ReasoningConfig::medium())
            .with_max_tokens(8_192);
        let body = client.request_body(&json!([]), &Value::Null, false);
        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["max_tokens"], 8_192);
    }

    #[test]
    fn strip_helper_matches_default_base_url_after_normalization() {
        use crate::providers::openrouter::client::DEFAULT_BASE_URL;
        let default_client = OpenRouterClient::new("k".into(), Some("m".into()));
        assert!(!default_client.should_strip_openrouter_extensions());

        let slashed = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url(format!("{DEFAULT_BASE_URL}/"));
        assert!(
            !slashed.should_strip_openrouter_extensions(),
            "trailing slash must not trigger stripping"
        );

        let custom = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("http://127.0.0.1:11434/v1");
        assert!(custom.should_strip_openrouter_extensions());
    }

    #[test]
    fn stripped_body_omits_openrouter_extensions_but_keeps_tool_choice() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("http://127.0.0.1:11434/v1")
            .with_provider_pref("coreweave")
            .with_allow_fallbacks(false)
            .with_session_id("sess-1");
        let tools = json!([{ "type": "function", "function": { "name": "x" } }]);
        let body = client.request_body(&json!([]), &tools, true);

        assert_eq!(body["model"], "m");
        assert_eq!(body["stream"], true);
        // OpenAI-standard keys survive stripping.
        assert_eq!(body["tool_choice"], "auto");
        assert!(body.get("tools").is_some());
        // OpenRouter-only keys are gone.
        assert!(body.get("reasoning").is_none());
        assert!(body.get("provider").is_none());
        assert!(body.get("session_id").is_none());
        assert!(body.get("stream_options").is_none());
        assert!(body.get("parallel_tool_calls").is_none());
    }

    #[test]
    fn non_stripped_body_keeps_openrouter_extensions() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_provider_pref("coreweave")
            .with_session_id("sess-1");
        assert!(!client.should_strip_openrouter_extensions());
        let tools = json!([{ "type": "function", "function": { "name": "x" } }]);
        let body = client.request_body(&json!([]), &tools, true);

        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], true);
        assert!(body.get("reasoning").is_some());
        assert!(body.get("provider").is_some());
        assert_eq!(body["session_id"], "sess-1");
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn body_dual_sends_max_tokens_and_max_completion_tokens() {
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_max_tokens(8_192);
        let body = client.request_body(&json!([]), &Value::Null, false);
        assert_eq!(body["max_tokens"], 8_192);
        assert_eq!(body["max_completion_tokens"], 8_192);
    }

    #[test]
    fn body_sends_temperature_only_when_set() {
        use crate::request::CompleteOptions;
        let client = OpenRouterClient::new("k".into(), Some("m".into()));

        let body = client.request_body(&json!([]), &Value::Null, false);
        assert!(body.get("temperature").is_none());

        let opts = CompleteOptions {
            temperature: Some(0.3),
            ..CompleteOptions::default()
        };
        let body =
            client.request_body_with(&json!([]), &Value::Null, false, &opts);
        let sent = body["temperature"].as_f64().expect("temperature is numeric");
        assert!((sent - 0.3).abs() < 1e-6, "temperature passthrough, got {sent}");
    }

    #[test]
    fn body_clamps_max_tokens_to_context_window() {
        // `with_context_window_tokens` floors at 1024, well below the
        // 24576 default cap, so the clamp must engage.
        let client = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_context_window_tokens(10);
        assert_eq!(client.context_window_tokens(), 1_024);
        let body = client.request_body(&json!([]), &Value::Null, false);
        assert_eq!(body["max_tokens"], 1_024);
        assert_eq!(body["max_completion_tokens"], 1_024);
    }
}
