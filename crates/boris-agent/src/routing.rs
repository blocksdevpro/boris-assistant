//! Fast vs strong model routing (Grok-style cheap/strong split, voice-sized).
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

use async_trait::async_trait;
use serde_json::Value;

use boris_ai::{LlmClient, LlmError};

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

/// Needles that force the strong tier (multi-step / research / code).
const STRONG_NEEDLES: &[&str] = &[
    "research",
    "search",
    "look up",
    "look for",
    "find out",
    "find my",
    "find me",
    "find the",
    "linkedin",
    "linked in",
    "github",
    "profile",
    "who is",
    "where's my",
    "where is my",
    "implement",
    "fix",
    "debug",
    "refactor",
    "write a",
    "create a",
    "build",
    "code",
    "file",
    "project",
    "bash",
    "run ",
    "install",
    "multi",
    "plan",
    "skill",
    "remember",
    "session",
    "memory",
    "todo",
    "handle this",
    "take care",
    "get things done",
    "investigate",
    "analyze",
    "compare",
    "why",
    "how do i",
    "how to",
];

/// Needles that prefer the fast tier (short local facts / greetings).
const FAST_NEEDLES: &[&str] = &[
    "time",
    "date",
    "day",
    "weather",
    "hello",
    "hi ",
    "hey",
    "thanks",
    "thank you",
    "who am i",
    "my name",
    "what time",
    "what's the time",
    "good morning",
    "good night",
];

/// Word-count thresholds used by [`classify_route`].
const LONG_REQUEST_WORDS: usize = 18;
const AMBIGUOUS_STRONG_WORDS: usize = 8;

/// Heuristic: simple local facts → fast; multi-step / research / skills → strong.
pub fn classify_route(user_text: &str) -> RouteMode {
    let t = user_text.trim().to_ascii_lowercase();
    if t.is_empty() {
        return RouteMode::Fast;
    }
    for n in STRONG_NEEDLES {
        if t.contains(n) {
            return RouteMode::Strong;
        }
    }
    // Long requests → strong
    if t.split_whitespace().count() > LONG_REQUEST_WORDS {
        return RouteMode::Strong;
    }
    for n in FAST_NEEDLES {
        if t.contains(n) {
            return RouteMode::Fast;
        }
    }
    // Default strong for safety on ambiguous chores
    if t.split_whitespace().count() > AMBIGUOUS_STRONG_WORDS {
        RouteMode::Strong
    } else {
        RouteMode::Fast
    }
}

/// Dual-model OpenRouter client with a process-local route switch.
pub struct RoutingClient {
    fast: Box<dyn LlmClient>,
    strong: Box<dyn LlmClient>,
    mode: AtomicU8,
}

impl RoutingClient {
    pub fn new(fast: Box<dyn LlmClient>, strong: Box<dyn LlmClient>) -> Self {
        Self {
            fast,
            strong,
            mode: AtomicU8::new(RouteMode::Strong as u8),
        }
    }

    pub fn set_route(&self, mode: RouteMode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
    }

    pub fn route(&self) -> RouteMode {
        match self.mode.load(Ordering::Relaxed) {
            0 => RouteMode::Fast,
            _ => RouteMode::Strong,
        }
    }

    fn active(&self) -> &dyn LlmClient {
        match self.route() {
            RouteMode::Fast => self.fast.as_ref(),
            RouteMode::Strong => self.strong.as_ref(),
        }
    }
}

#[async_trait]
impl LlmClient for RoutingClient {
    async fn complete(&self, messages: Value, tools: Value) -> Result<Value, LlmError> {
        // Tool rounds always use the strong tier (better function-calling).
        if tools_requested(&tools) {
            self.set_route(RouteMode::Strong);
        } else if let Some(text) = last_user_text(&messages) {
            // Auto-route from the latest user text in the payload (no agent downcast).
            self.set_route(classify_route(&text));
        }
        let mode = self.route();
        let primary = self.active();
        tracing::debug!(route = mode.as_str(), model = %primary.model(), "llm route");

        match primary.complete(messages.clone(), tools.clone()).await {
            Ok(v) => Ok(v),
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
                    "model does not support tools; retrying with other route"
                );
                fallback.complete(messages, tools).await
            }
            Err(e) => Err(e),
        }
    }

    fn model(&self) -> &str {
        self.active().model()
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
    for m in arr.iter().rev() {
        if m.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let c = m.get("content")?;
        if let Some(s) = c.as_str() {
            // Skip system-reminder only messages for routing.
            if s.contains("<system-reminder>") && s.len() < 400 {
                continue;
            }
            if s.contains("<conversation_summary>") {
                continue;
            }
            return Some(s.to_string());
        }
    }
    None
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
        assert_eq!(
            classify_route("look for my github"),
            RouteMode::Strong
        );
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
        let long = (0..20).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        assert_eq!(classify_route(&long), RouteMode::Strong);
    }

    #[test]
    fn classifies_medium_ambiguous_as_strong() {
        // >8 words, no strong/fast needles → strong for safety
        let mid = "please just do that thing for me now yes";
        assert!(mid.split_whitespace().count() > AMBIGUOUS_STRONG_WORDS);
        assert_eq!(classify_route(mid), RouteMode::Strong);
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
}
