//! Provider-agnostic LLM client trait.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::LlmError;
use crate::request::CompleteOptions;
use crate::stream::LlmStreamEvent;

/// HTTP transport abstraction so the agent harness stays provider-agnostic.
///
/// # Contract
///
/// - `messages` — JSON array of chat messages (OpenAI shape).
/// - `tools` — JSON array of tool definitions, or `null` / `[]` when none.
/// - Return value — the raw `choices[0].message` object, with:
///   - `content` preferably a **string** (not a parts array / null)
///   - optional `tool_calls` array
///
/// Implementations may stream internally. [`Self::complete`] stays one-shot
/// (assembled message). [`Self::complete_stream`] exposes typed deltas.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send the conversation + tools; return `choices[0].message` JSON.
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError>;

    /// Same as [`Self::complete`] with per-request reasoning / token overrides.
    ///
    /// The default implementation **ignores `opts`** and delegates to
    /// [`Self::complete`], so existing mocks keep compiling and behaving as
    /// before. That also means the default does NOT apply reasoning effort,
    /// `max_tokens`, or stage budgets.
    ///
    /// Mocks used in tests that assert on per-request options **MUST override
    /// this method**; relying on the default silently drops `opts` and will
    /// make such tests vacuously pass.
    async fn complete_with_options(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
    ) -> Result<Value, LlmError> {
        let _ = opts;
        self.complete(messages, tools).await
    }

    /// Stream typed events, then return the assembled message (same as `complete`).
    ///
    /// Default: one-shot [`Self::complete_with_options`], then synthesize the
    /// same event granularity [`OpenRouterClient`](crate::OpenRouterClient)
    /// emits for a real stream:
    ///
    /// - always `ModelSend` first,
    /// - `FirstDelta` when the assembled message carries a payload (non-empty
    ///   `content` string and/or a non-empty `tool_calls` array),
    /// - one `ToolCallDelta` + `ToolCallComplete` pair per assembled tool call,
    /// - always `FinalMessage` last.
    ///
    /// Mock implementations inherit this behavior automatically; they only
    /// need to provide `complete` / `complete_with_options`.
    async fn complete_stream(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        on_event(LlmStreamEvent::ModelSend {
            model: self.model().to_string(),
        });
        let started = std::time::Instant::now();
        let msg = self.complete_with_options(messages, tools, opts).await?;
        emit_default_stream_events(
            on_event,
            &msg,
            started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        );
        on_event(LlmStreamEvent::FinalMessage(msg.clone()));
        Ok(msg)
    }

    /// Model identifier for logging / turn metadata.
    fn model(&self) -> &str {
        "unknown"
    }

    /// Maximum tokens accepted by this client's configured model, including
    /// prompt and completion tokens. Hosts should configure this from provider
    /// model metadata when it differs from the conservative default.
    fn context_window_tokens(&self) -> Option<u32> {
        None
    }
}

/// Synthesize stream events for an already-assembled `choices[0].message`.
///
/// Used by the default [`LlmClient::complete_stream`]. Emits `FirstDelta` iff
/// the message carries a payload, then one `ToolCallDelta` +
/// `ToolCallComplete` pair per entry of a `tool_calls` array. The caller is
/// responsible for the surrounding `ModelSend` / `FinalMessage` events.
fn emit_default_stream_events(
    on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    msg: &Value,
    ttfb_ms: u64,
) {
    let has_content = msg
        .get("content")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let tool_calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if has_content || !tool_calls.is_empty() {
        on_event(LlmStreamEvent::FirstDelta { ttfb_ms });
    }
    for (index, tc) in tool_calls.iter().enumerate() {
        let id = tc
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = tc
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let arguments = tc
            .get("function")
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        on_event(LlmStreamEvent::ToolCallDelta {
            index: index as u32,
            id: if id.is_empty() {
                None
            } else {
                Some(id.clone())
            },
            name: if name.is_empty() {
                None
            } else {
                Some(name.clone())
            },
            arguments_delta: arguments.clone(),
        });
        on_event(LlmStreamEvent::ToolCallComplete {
            index: index as u32,
            id,
            name,
            arguments,
        });
    }
}

/// Validate `messages` / `tools` payload shapes before sending to a provider.
///
/// - `messages` must be a JSON array (OpenAI chat shape); participant and
///   historical function names must match `^[a-zA-Z0-9_-]+$`.
/// - `tools` must be `null` or a JSON array; each entry must be an object
///   with `"type": "function"` and a `function` object carrying a valid name.
///
/// This is intentionally **not** called from the [`LlmClient`] trait defaults:
/// concrete clients validate the canonical wire payload immediately before
/// sending it. Trait defaults may delegate to another client.
pub(crate) fn validate_messages_tools(messages: &Value, tools: &Value) -> Result<(), LlmError> {
    let messages = messages.as_array().ok_or_else(|| {
        LlmError::invalid_request("`messages` must be a JSON array of chat messages")
    })?;
    for (i, message) in messages.iter().enumerate() {
        if let Some(name) = message.get("name") {
            validate_name(name, &format!("messages[{i}].name"))?;
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (j, call) in calls.iter().enumerate() {
                validate_name(
                    &call["function"]["name"],
                    &format!("messages[{i}].tool_calls[{j}].function.name"),
                )?;
            }
        }
    }
    match tools {
        Value::Null => Ok(()),
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let obj = item.as_object().ok_or_else(|| {
                    LlmError::invalid_request(format!("`tools[{i}]` must be an object"))
                })?;
                let ty = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if ty != "function" {
                    return Err(LlmError::invalid_request(format!(
                        "`tools[{i}].type` must be \"function\""
                    )));
                }
                validate_name(
                    &item["function"]["name"],
                    &format!("tools[{i}].function.name"),
                )?;
            }
            Ok(())
        }
        _ => Err(LlmError::invalid_request(
            "`tools` must be null or a JSON array of tool definitions",
        )),
    }
}

fn validate_name(value: &Value, path: &str) -> Result<(), LlmError> {
    if !value.as_str().is_some_and(crate::is_valid_tool_name) {
        return Err(LlmError::invalid_request(format!(
            "`{path}` must be a string matching ^[a-zA-Z0-9_-]+$ (got {value})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::CompleteOptions;
    use serde_json::json;

    struct StaticMock {
        message: Value,
    }

    #[async_trait::async_trait]
    impl LlmClient for StaticMock {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            Ok(self.message.clone())
        }

        fn model(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn default_stream_emits_first_delta_for_content() {
        let mock = StaticMock {
            message: json!({"content": "hello"}),
        };
        let mut events = Vec::new();
        let mut sink = |e: LlmStreamEvent| events.push(e);
        let msg = mock
            .complete_stream(
                json!([]),
                Value::Null,
                CompleteOptions::default(),
                &mut sink,
            )
            .await
            .unwrap();
        assert_eq!(msg["content"], "hello");
        assert!(matches!(
            events.first(),
            Some(LlmStreamEvent::ModelSend { .. })
        ));
        assert!(events
            .iter()
            .any(|e| matches!(e, LlmStreamEvent::FirstDelta { .. })));
        assert!(matches!(
            events.last(),
            Some(LlmStreamEvent::FinalMessage(_))
        ));
    }

    #[tokio::test]
    async fn default_stream_emits_tool_events_before_final() {
        let mock = StaticMock {
            message: json!({
                "content": "",
                "tool_calls": [
                    {"id": "c1", "type": "function",
                     "function": {"name": "get_time", "arguments": "{}"}}
                ],
            }),
        };
        let mut events = Vec::new();
        let mut sink = |e: LlmStreamEvent| events.push(e);
        mock.complete_stream(
            json!([]),
            Value::Null,
            CompleteOptions::default(),
            &mut sink,
        )
        .await
        .unwrap();
        let delta = events
            .iter()
            .position(|e| matches!(e, LlmStreamEvent::ToolCallDelta { .. }))
            .expect("ToolCallDelta");
        let complete = events
            .iter()
            .position(|e| matches!(e, LlmStreamEvent::ToolCallComplete { .. }))
            .expect("ToolCallComplete");
        let final_msg = events
            .iter()
            .position(|e| matches!(e, LlmStreamEvent::FinalMessage(_)))
            .expect("FinalMessage");
        assert!(delta < complete && complete < final_msg);
        assert!(matches!(
            &events[complete],
            LlmStreamEvent::ToolCallComplete { id, name, .. }
                if id == "c1" && name == "get_time"
        ));
    }

    #[tokio::test]
    async fn default_stream_skips_first_delta_when_empty() {
        let mock = StaticMock {
            message: json!({"content": ""}),
        };
        let mut events = Vec::new();
        let mut sink = |e: LlmStreamEvent| events.push(e);
        mock.complete_stream(
            json!([]),
            Value::Null,
            CompleteOptions::default(),
            &mut sink,
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events.as_slice(),
            [
                LlmStreamEvent::ModelSend { .. },
                LlmStreamEvent::FinalMessage(_)
            ]
        ));
    }

    #[test]
    fn validate_accepts_null_and_function_tools() {
        let tools = json!([
            {"type": "function", "function": {"name": "get_time"}},
            {"type": "function", "function": {"name": "x", "arguments": "{}"}},
        ]);
        assert!(validate_messages_tools(&json!([]), &Value::Null).is_ok());
        assert!(validate_messages_tools(&json!([]), &json!([])).is_ok());
        assert!(validate_messages_tools(&json!([{"role": "user"}]), &tools).is_ok());
    }

    #[test]
    fn validate_rejects_bad_shapes() {
        // messages must be an array
        assert!(validate_messages_tools(&json!({}), &Value::Null).is_err());
        assert!(validate_messages_tools(&Value::Null, &Value::Null).is_err());
        // tools must be null or an array
        assert!(validate_messages_tools(&json!([]), &json!({})).is_err());
        assert!(validate_messages_tools(&json!([]), &json!("x")).is_err());
        // entries must be type=function objects with a named function
        assert!(validate_messages_tools(&json!([]), &json!([{"type": "function"}])).is_err());
        assert!(validate_messages_tools(
            &json!([]),
            &json!([{"type": "other", "function": {"name": "x"}}])
        )
        .is_err());
        assert!(validate_messages_tools(
            &json!([]),
            &json!([{"type": "function", "function": {"name": "  "}}])
        )
        .is_err());
        assert!(validate_messages_tools(&json!([]), &json!(["nope"])).is_err());
    }

    #[test]
    fn validate_checks_historical_names_even_without_advertised_tools() {
        for name in [
            json!(""),
            json!("functions.web_search"),
            json!("web search"),
            json!("web/search"),
            json!("réad"),
            Value::Null,
            json!(123),
        ] {
            let messages = json!([
                {"role":"user", "content":"check"},
                {"role":"assistant", "tool_calls":[{"id":"c1", "function":{"name":name, "arguments":"{}"}}]}
            ]);
            let error = validate_messages_tools(&messages, &Value::Null).unwrap_err();
            assert!(error
                .message()
                .contains("messages[1].tool_calls[0].function.name"));
            assert!(error.message().contains("^[a-zA-Z0-9_-]+$"));
            let tools = json!([{"type":"function", "function":{"name":name}}]);
            let error = validate_messages_tools(&json!([]), &tools).unwrap_err();
            assert!(error.message().contains("tools[0].function.name"));
        }
        assert!(validate_messages_tools(&json!([
            {"role":"assistant", "tool_calls":[{"id":"old", "function":{"name":"retired_tool-2", "arguments":"{}"}}]},
            {"role":"tool", "tool_call_id":"old", "content":"old result"}
        ]), &Value::Null).is_ok());
    }

    #[test]
    fn validate_reports_invalid_optional_participant_name() {
        let error = validate_messages_tools(
            &json!([{"role":"user", "name":"User Name", "content":"hi"}]),
            &Value::Null,
        )
        .unwrap_err();
        assert!(error.message().contains("messages[0].name"));
    }
}
