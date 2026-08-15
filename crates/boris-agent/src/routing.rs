//! Fast vs strong model routing from task/round traits (not tool-list presence).
//!
//! # Surface
//!
//! | Item | Role |
//! |------|------|
//! | [`RouteMode`] | Fast / Strong tier |
//! | [`classify_route`] | Pure heuristic from latest user text |
//! | [`RoutingClient`] | Dual-client wrapper; auto-routes on `complete` |
//! | [`apply_route_hint`] | Host no-op (routing is automatic) |
//!
//! Pure helpers (`classify_route`, last-user extraction, tool-unsupported
//! detection) are unit-tested without network.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value;

use boris_ai::{CompleteOptions, LlmClient, LlmError, LlmStreamEvent, RequestStage};

use crate::task::{classify_task, RoundTraits, TaskTraits};

/// Which model tier to use for the next completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RouteMode {
    Fast = 0,
    Strong = 1,
}

impl RouteMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Strong => "strong",
        }
    }
}

/// Heuristic: simple local facts → fast; multi-step / research / skills → strong.
pub fn classify_route(user_text: &str) -> RouteMode {
    let task = classify_task(user_text);
    route_from_traits(task, RoundTraits::first(task))
}

/// Route from structured task + round traits. Tool-list presence is ignored.
pub fn route_from_traits(task: TaskTraits, round: RoundTraits) -> RouteMode {
    if round.should_escalate_strong() || task.needs_strong() {
        RouteMode::Strong
    } else {
        RouteMode::Fast
    }
}

/// Stage-aware reasoning / token budget for this request.
pub fn request_stage_for(task: TaskTraits, round: RoundTraits) -> RequestStage {
    if task.complexity == crate::task::TaskComplexity::Complex
        || task.research_depth >= crate::task::ResearchDepth::Deep
        || round.has_error_evidence
        || (round.tool_rounds > 0 && task.needs_strong())
    {
        RequestStage::Complex
    } else if task.needs_strong() || (round.has_tool_results && !task.is_simple_voice()) {
        RequestStage::ToolPlanning
    } else {
        RequestStage::SimpleVoice
    }
}

/// Infer round traits from the wire messages (tool results / error observations).
pub fn round_traits_from_messages(messages: &Value, user_text: &str) -> RoundTraits {
    round_traits_for_task(messages, classify_task(user_text))
}

/// Infer round traits for one task, ignoring evidence from older user turns.
pub(crate) fn round_traits_for_task(messages: &Value, task: TaskTraits) -> RoundTraits {
    let mut has_tool_results = false;
    let mut has_error_evidence = false;
    let mut tool_rounds = 0u32;
    if let Some(arr) = messages.as_array() {
        let turn_start = current_turn_start(arr).map_or(0, |i| i.saturating_add(1));
        for m in &arr[turn_start..] {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "tool" {
                has_tool_results = true;
                if let Some(c) = m.get("content").and_then(|c| c.as_str()) {
                    let c = c.trim_start().to_ascii_lowercase();
                    if c.starts_with("error") || c.contains("invalid arguments") {
                        has_error_evidence = true;
                    }
                }
            }
            if role == "assistant" && m.get("tool_calls").is_some() {
                tool_rounds = tool_rounds.saturating_add(1);
            }
        }
    }
    RoundTraits {
        task,
        has_tool_results,
        has_error_evidence,
        tool_rounds,
    }
}

fn current_turn_start(messages: &[Value]) -> Option<usize> {
    messages.iter().rposition(|m| {
        if m.get("role").and_then(|r| r.as_str()) != Some("user") {
            return false;
        }
        m.get("content")
            .and_then(Value::as_str)
            .is_some_and(|s| !is_control_user_message(s))
    })
}

fn is_control_user_message(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<system-reminder>") || text.starts_with("<conversation_summary>")
}

/// Dual-model OpenRouter client with request-local routing.
pub struct RoutingClient {
    fast: Box<dyn LlmClient>,
    strong: Box<dyn LlmClient>,
    /// Stable model label; unlike a last-route lookup this cannot misreport a
    /// concurrent background request as the foreground request's model.
    model_label: String,
    /// Last selected route, retained only for the legacy `route()` / `model()`
    /// inspection surface. Request dispatch never reads this value.
    last_route: AtomicU8,
}

impl RoutingClient {
    pub fn new(fast: Box<dyn LlmClient>, strong: Box<dyn LlmClient>) -> Self {
        let model_label = if fast.model() == strong.model() {
            fast.model().to_string()
        } else {
            format!("routing({}|{})", fast.model(), strong.model())
        };
        Self {
            fast,
            strong,
            model_label,
            last_route: AtomicU8::new(RouteMode::Strong as u8),
        }
    }

    /// Update the compatibility/diagnostic route value.
    ///
    /// Completion routing is automatic and request-local; this method does not
    /// steer an in-flight or future completion.
    pub fn set_route(&self, mode: RouteMode) {
        self.last_route.store(mode as u8, Ordering::Relaxed);
    }

    /// Most recently selected route (diagnostic only under concurrency).
    pub fn route(&self) -> RouteMode {
        match self.last_route.load(Ordering::Relaxed) {
            0 => RouteMode::Fast,
            _ => RouteMode::Strong,
        }
    }

    fn client_for(&self, mode: RouteMode) -> &dyn LlmClient {
        match mode {
            RouteMode::Fast => self.fast.as_ref(),
            RouteMode::Strong => self.strong.as_ref(),
        }
    }
}

impl RoutingClient {
    fn prepare_request(&self, messages: &Value) -> (RouteMode, CompleteOptions) {
        let text = last_user_text(messages).unwrap_or_default();
        let round = round_traits_from_messages(messages, &text);
        let mode = route_from_traits(round.task, round);
        let stage = request_stage_for(round.task, round);
        tracing::debug!(
            route = mode.as_str(),
            stage = ?stage,
            simple_voice = round.task.is_simple_voice(),
            "llm route from task traits"
        );
        (mode, CompleteOptions::for_stage(stage))
    }

    fn merge_options(mut routed: CompleteOptions, opts: CompleteOptions) -> CompleteOptions {
        // A caller may request more headroom, but stale call-site options must
        // never downgrade current-turn error/research escalation. Individual
        // reasoning/token fields remain explicit final overrides.
        let inferred = routed.stage.unwrap_or(RequestStage::SimpleVoice);
        let caller_downgrades = opts
            .stage
            .is_some_and(|requested| stage_rank(requested) < stage_rank(inferred));
        if let Some(requested) = opts.stage {
            let stage = stronger_stage(inferred, requested);
            routed = CompleteOptions::for_stage(stage);
        }
        if !caller_downgrades && opts.reasoning.is_some() {
            routed.reasoning = opts.reasoning;
        }
        if !caller_downgrades && opts.max_tokens.is_some() {
            routed.max_tokens = opts.max_tokens;
        }
        routed
    }

    async fn complete_on(
        &self,
        client: &dyn LlmClient,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
    ) -> Result<Value, LlmError> {
        client.complete_with_options(messages, tools, opts).await
    }
}

fn stronger_stage(a: RequestStage, b: RequestStage) -> RequestStage {
    if stage_rank(a) >= stage_rank(b) {
        a
    } else {
        b
    }
}

fn stage_rank(stage: RequestStage) -> u8 {
    match stage {
        RequestStage::SimpleVoice => 0,
        RequestStage::ToolPlanning => 1,
        RequestStage::Complex => 2,
    }
}

#[async_trait]
impl LlmClient for RoutingClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        self.complete_with_options(messages, tools, CompleteOptions::default())
            .await
    }

    async fn complete_with_options(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
    ) -> Result<Value, LlmError> {
        // Route from task/round traits — never from mere tool-list presence.
        let (mode, inferred) = self.prepare_request(&messages);
        let routed = Self::merge_options(inferred, opts);
        // Compatibility telemetry is updated, but the client reference and
        // fallback choice below are both derived from this request's local mode.
        self.set_route(mode);
        let primary = self.client_for(mode);
        tracing::debug!(route = mode.as_str(), model = %primary.model(), "llm route");

        let started = Instant::now();
        match self
            .complete_on(primary, messages.clone(), tools.clone(), routed.clone())
            .await
        {
            Ok(v) => {
                tracing::debug!(
                    route = mode.as_str(),
                    model = %primary.model(),
                    ms = started.elapsed().as_millis() as u64,
                    "llm route done"
                );
                Ok(v)
            }
            Err(e) if tools_requested(&tools) && is_tool_unsupported_error(&e) => {
                // e.g. morph/morph-v3-fast is a code-apply model — no tool endpoints.
                // Fall back to the other tier so voice+tools keep working.
                let fallback = match mode {
                    RouteMode::Strong => self.fast.as_ref(),
                    RouteMode::Fast => self.strong.as_ref(),
                };
                if fallback.model() == primary.model() {
                    return Err(e);
                }
                tracing::warn!(
                    failed_model = %primary.model(),
                    fallback_model = %fallback.model(),
                    error = %e,
                    ms = started.elapsed().as_millis() as u64,
                    "model does not support tools; retrying with other route"
                );
                let retry_t = Instant::now();
                let result = self.complete_on(fallback, messages, tools, routed).await;
                tracing::debug!(
                    model = %fallback.model(),
                    ms = retry_t.elapsed().as_millis() as u64,
                    ok = result.is_ok(),
                    "llm route fallback done"
                );
                result
            }
            Err(e) => {
                tracing::debug!(
                    route = mode.as_str(),
                    model = %primary.model(),
                    ms = started.elapsed().as_millis() as u64,
                    error = %e,
                    "llm route failed"
                );
                Err(e)
            }
        }
    }

    async fn complete_stream(
        &self,
        messages: Value,
        tools: Value,
        opts: CompleteOptions,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<Value, LlmError> {
        let (mode, inferred) = self.prepare_request(&messages);
        let routed = Self::merge_options(inferred, opts);
        self.set_route(mode);
        let primary = self.client_for(mode);
        match primary
            .complete_stream(messages.clone(), tools.clone(), routed.clone(), on_event)
            .await
        {
            Ok(v) => Ok(v),
            Err(e) if tools_requested(&tools) && is_tool_unsupported_error(&e) => {
                let fallback = match mode {
                    RouteMode::Strong => self.fast.as_ref(),
                    RouteMode::Fast => self.strong.as_ref(),
                };
                if fallback.model() == primary.model() {
                    return Err(e);
                }
                fallback
                    .complete_stream(messages, tools, routed, on_event)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    fn model(&self) -> &str {
        &self.model_label
    }
}

fn tools_requested(tools: &Value) -> bool {
    tools.as_array().is_some_and(|a| !a.is_empty())
}

/// OpenRouter 404: "No endpoints found that support tool use…"
fn is_tool_unsupported_error(e: &LlmError) -> bool {
    let m = e.message.to_ascii_lowercase();
    m.contains("no endpoints found that support tool")
        || m.contains("support tool use")
        || m.contains("does not support tool")
        || m.contains("tools are not supported")
        || m.contains("tool use is not supported")
}

/// Latest user text from an OpenAI-style messages array, skipping reminder/summary-only rows.
fn last_user_text(messages: &Value) -> Option<String> {
    let arr = messages.as_array()?;
    let i = current_turn_start(arr)?;
    arr[i]
        .get("content")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

/// Hint helper for hosts that already classified the turn.
pub fn apply_route_hint(client: &dyn LlmClient, user_text: &str) {
    let _ = (client, user_text);
    // Routing is automatic inside RoutingClient::complete.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_time_as_fast() {
        assert_eq!(classify_route("what time is it"), RouteMode::Fast);
    }

    #[test]
    fn classifies_research_as_strong() {
        assert_eq!(
            classify_route("research the latest Rust async runtimes"),
            RouteMode::Strong
        );
    }

    #[test]
    fn classifies_person_lookup_as_strong() {
        assert_eq!(
            classify_route("find my linkedin profile Uttam"),
            RouteMode::Strong
        );
        assert_eq!(classify_route("look for my github"), RouteMode::Strong);
    }

    #[test]
    fn classifies_empty_as_fast() {
        assert_eq!(classify_route(""), RouteMode::Fast);
        assert_eq!(classify_route("   "), RouteMode::Fast);
    }

    #[test]
    fn classifies_greeting_as_fast() {
        assert_eq!(classify_route("hello"), RouteMode::Fast);
        assert_eq!(classify_route("hey there"), RouteMode::Fast);
        assert_eq!(classify_route("thanks"), RouteMode::Fast);
    }

    #[test]
    fn classifies_code_needles_as_strong() {
        assert_eq!(classify_route("please debug this"), RouteMode::Strong);
        assert_eq!(classify_route("how to install rust"), RouteMode::Strong);
        assert_eq!(classify_route("open the file"), RouteMode::Strong);
    }

    #[test]
    fn classifies_long_request_as_strong() {
        let long = (0..20)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(classify_route(&long), RouteMode::Strong);
    }

    #[test]
    fn classifies_medium_ambiguous_as_fast_when_no_strong_traits() {
        // No research/coding/side-effects — do not force strong just for length.
        let mid = "please just do that thing for me now yes";
        assert_eq!(classify_route(mid), RouteMode::Fast);
    }

    #[test]
    fn classifies_short_ambiguous_as_fast() {
        assert_eq!(classify_route("ok sure"), RouteMode::Fast);
    }

    #[test]
    fn route_mode_as_str() {
        assert_eq!(RouteMode::Fast.as_str(), "fast");
        assert_eq!(RouteMode::Strong.as_str(), "strong");
    }

    #[test]
    fn stale_simple_options_cannot_downgrade_complex_inference() {
        let merged = RoutingClient::merge_options(
            CompleteOptions::for_stage(RequestStage::Complex),
            CompleteOptions::for_stage(RequestStage::SimpleVoice),
        );
        assert_eq!(merged, CompleteOptions::for_stage(RequestStage::Complex));
    }

    #[test]
    fn tools_requested_detects_nonempty_array() {
        assert!(!tools_requested(&Value::Null));
        assert!(!tools_requested(&json!([])));
        assert!(tools_requested(&json!([{ "type": "function" }])));
    }

    #[test]
    fn detects_openrouter_tool_unsupported_errors() {
        let e = LlmError::http(
            "OpenRouter HTTP error 404 Not Found: {\"error\":{\"message\":\"No endpoints found that support tool use. Try disabling \\\"get_time\\\".\",\"code\":404}}",
        );
        assert!(is_tool_unsupported_error(&e));
        assert!(is_tool_unsupported_error(&LlmError::http(
            "model does not support tool calling"
        )));
        assert!(is_tool_unsupported_error(&LlmError::http(
            "Tools are not supported"
        )));
        assert!(!is_tool_unsupported_error(&LlmError::http("rate limited")));
    }

    #[test]
    fn last_user_text_picks_latest_user() {
        let messages = json!([
            { "role": "system", "content": "sys" },
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": "ok" },
            { "role": "user", "content": "second" },
        ]);
        assert_eq!(last_user_text(&messages).as_deref(), Some("second"));
    }

    #[test]
    fn last_user_text_skips_system_reminders() {
        let messages = json!([
            { "role": "user", "content": "what time is it" },
            { "role": "user", "content": "<system-reminder>\nkeep going\n</system-reminder>" },
        ]);
        assert_eq!(
            last_user_text(&messages).as_deref(),
            Some("what time is it")
        );
    }

    #[test]
    fn last_user_text_skips_long_system_reminders() {
        let reminder = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            "continue research ".repeat(80)
        );
        let messages = json!([
            { "role": "user", "content": "hello" },
            { "role": "user", "content": reminder },
        ]);
        assert_eq!(last_user_text(&messages).as_deref(), Some("hello"));
    }

    #[test]
    fn last_user_text_skips_conversation_summary() {
        let messages = json!([
            { "role": "user", "content": "real question" },
            {
                "role": "user",
                "content": "<conversation_summary>\nold stuff\n</conversation_summary>"
            },
        ]);
        assert_eq!(last_user_text(&messages).as_deref(), Some("real question"));
    }

    #[test]
    fn last_user_text_none_when_no_users() {
        let messages = json!([
            { "role": "system", "content": "sys" },
            { "role": "assistant", "content": "hi" },
        ]);
        assert!(last_user_text(&messages).is_none());
        assert!(last_user_text(&json!({})).is_none());
    }

    #[test]
    fn old_tool_error_does_not_poison_new_turn() {
        let messages = json!([
            { "role": "user", "content": "old task" },
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "old", "function": {"name": "web_search"}
            }]},
            { "role": "tool", "content": "Error: old failure" },
            { "role": "assistant", "content": "Could not finish." },
            { "role": "user", "content": "hello" },
        ]);
        let round = round_traits_from_messages(&messages, "hello");
        assert!(!round.has_tool_results);
        assert!(!round.has_error_evidence);
        assert_eq!(round.tool_rounds, 0);
        assert_eq!(route_from_traits(round.task, round), RouteMode::Fast);
        assert_eq!(
            request_stage_for(round.task, round),
            RequestStage::SimpleVoice
        );
    }

    #[test]
    fn current_turn_error_escalates_route_and_budget() {
        let messages = json!([
            { "role": "user", "content": "old task" },
            { "role": "tool", "content": "old success" },
            { "role": "user", "content": "hello" },
            { "role": "assistant", "content": null, "tool_calls": [{
                "id": "new", "function": {"name": "get_time"}
            }]},
            { "role": "tool", "content": "Error [invalid_args]: fix it" },
        ]);
        let round = round_traits_from_messages(&messages, "hello");
        assert!(round.has_tool_results);
        assert!(round.has_error_evidence);
        assert_eq!(round.tool_rounds, 1);
        assert_eq!(route_from_traits(round.task, round), RouteMode::Strong);
        assert_eq!(request_stage_for(round.task, round), RequestStage::Complex);
    }

    struct RecordingClient {
        model: &'static str,
        calls: std::sync::Mutex<u32>,
    }

    struct StreamRecordingClient {
        model: &'static str,
        options: std::sync::Arc<std::sync::Mutex<Vec<CompleteOptions>>>,
    }

    #[async_trait]
    impl LlmClient for StreamRecordingClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            Ok(json!({ "role": "assistant", "content": "ok" }))
        }

        async fn complete_stream(
            &self,
            _messages: Value,
            _tools: Value,
            opts: CompleteOptions,
            on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
        ) -> Result<Value, LlmError> {
            self.options.lock().unwrap().push(opts);
            on_event(LlmStreamEvent::ContentDelta { text: "ok".into() });
            Ok(json!({ "role": "assistant", "content": "ok" }))
        }

        fn model(&self) -> &str {
            self.model
        }
    }

    #[async_trait]
    impl LlmClient for RecordingClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            *self.calls.lock().unwrap() += 1;
            Ok(json!({ "role": "assistant", "content": "ok" }))
        }
        fn model(&self) -> &str {
            self.model
        }
    }

    fn nonempty_tools() -> Value {
        json!([{
            "type": "function",
            "function": { "name": "get_time", "parameters": { "type": "object" } }
        }])
    }

    #[tokio::test]
    async fn complete_with_tools_keeps_fast_for_greeting() {
        let fast = RecordingClient {
            model: "fast-model",
            calls: std::sync::Mutex::new(0),
        };
        let strong = RecordingClient {
            model: "strong-model",
            calls: std::sync::Mutex::new(0),
        };
        let client = RoutingClient::new(Box::new(fast), Box::new(strong));
        let messages = json!([{ "role": "user", "content": "hello" }]);
        let _ = client.complete(messages, nonempty_tools()).await.unwrap();
        assert_eq!(client.route(), RouteMode::Fast);
        assert_eq!(client.model(), "routing(fast-model|strong-model)");
    }

    #[tokio::test]
    async fn complete_with_tools_keeps_fast_for_time() {
        let client = RoutingClient::new(
            Box::new(RecordingClient {
                model: "fast-model",
                calls: std::sync::Mutex::new(0),
            }),
            Box::new(RecordingClient {
                model: "strong-model",
                calls: std::sync::Mutex::new(0),
            }),
        );
        let messages = json!([{ "role": "user", "content": "what time is it" }]);
        let _ = client.complete(messages, nonempty_tools()).await.unwrap();
        assert_eq!(client.route(), RouteMode::Fast);
    }

    #[tokio::test]
    async fn complete_with_tools_uses_strong_for_research() {
        let client = RoutingClient::new(
            Box::new(RecordingClient {
                model: "fast-model",
                calls: std::sync::Mutex::new(0),
            }),
            Box::new(RecordingClient {
                model: "strong-model",
                calls: std::sync::Mutex::new(0),
            }),
        );
        let messages = json!([{
            "role": "user",
            "content": "research the latest Rust async runtimes"
        }]);
        let _ = client.complete(messages, nonempty_tools()).await.unwrap();
        assert_eq!(client.route(), RouteMode::Strong);
        assert_eq!(client.model(), "routing(fast-model|strong-model)");
    }

    #[tokio::test]
    async fn complete_escalates_after_error_observation() {
        let client = RoutingClient::new(
            Box::new(RecordingClient {
                model: "fast-model",
                calls: std::sync::Mutex::new(0),
            }),
            Box::new(RecordingClient {
                model: "strong-model",
                calls: std::sync::Mutex::new(0),
            }),
        );
        let messages = json!([
            { "role": "user", "content": "hello" },
            { "role": "assistant", "content": "", "tool_calls": [{"id":"c1","function":{"name":"get_time"}}] },
            { "role": "tool", "content": "Error [missing_required]: missing command" }
        ]);
        let _ = client.complete(messages, nonempty_tools()).await.unwrap();
        assert_eq!(client.route(), RouteMode::Strong);
    }

    #[tokio::test]
    async fn complete_stream_preserves_explicit_stage_and_forwards_events() {
        let fast_options = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let strong_options = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let client = RoutingClient::new(
            Box::new(StreamRecordingClient {
                model: "fast-model",
                options: std::sync::Arc::clone(&fast_options),
            }),
            Box::new(StreamRecordingClient {
                model: "strong-model",
                options: std::sync::Arc::clone(&strong_options),
            }),
        );
        let mut events = Vec::new();
        client
            .complete_stream(
                json!([{ "role": "user", "content": "hello" }]),
                nonempty_tools(),
                CompleteOptions {
                    stage: Some(RequestStage::ToolPlanning),
                    ..CompleteOptions::default()
                },
                &mut |event| events.push(event),
            )
            .await
            .unwrap();

        assert_eq!(
            fast_options.lock().unwrap().as_slice(),
            &[CompleteOptions::for_stage(RequestStage::ToolPlanning)]
        );
        assert!(strong_options.lock().unwrap().is_empty());
        assert!(matches!(
            events.as_slice(),
            [LlmStreamEvent::ContentDelta { text }] if text == "ok"
        ));
    }
}
