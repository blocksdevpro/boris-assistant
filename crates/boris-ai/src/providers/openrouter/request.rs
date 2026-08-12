//! Request body construction for OpenRouter chat completions.

use serde_json::{json, Value};

use super::client::OpenRouterClient;

impl OpenRouterClient {
    /// Build the JSON body for a chat completion request.
    ///
    /// Clones `messages` / `tools` into the body once per call.
    pub(super) fn request_body(&self, messages: &Value, tools: &Value, stream: bool) -> Value {
        let mut body = base_body(&self.model, messages, tools);
        let obj = body
            .as_object_mut()
            .expect("chat completion body is always a JSON object");

        // Headroom for reasoning + final answer / tool_calls.
        obj.insert("max_tokens".into(), json!(self.max_tokens));

        // Unified OpenRouter reasoning (DeepSeek / Gemini thinking / Claude / o-series).
        obj.insert("reasoning".into(), self.reasoning.to_request_value());

        if stream {
            obj.insert("stream".into(), json!(true));
            // Final SSE event includes usage (incl. cached_tokens) when supported.
            obj.insert("stream_options".into(), json!({ "include_usage": true }));
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

        if let Some(sid) = self.session_id.as_deref().filter(|s| !s.is_empty()) {
            obj.insert("session_id".into(), json!(sid));
        }

        body
    }
}

fn base_body(model: &str, messages: &Value, tools: &Value) -> Value {
    if tools_absent_or_empty(tools) {
        json!({
            "model": model,
            "messages": messages,
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
}
