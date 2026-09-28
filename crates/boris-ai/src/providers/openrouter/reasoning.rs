//! OpenRouter unified `reasoning` request controls.
//!
//! See <https://openrouter.ai/docs/guides/best-practices/reasoning-tokens>.
//! Models that support thinking (DeepSeek, Gemini thinking, Claude, o-series)
//! use this object; others ignore it.

use serde_json::{json, Value};

/// How hard the model should think before answering / calling tools.
///
/// # Effort vocabulary
///
/// These strings are sent verbatim as OpenRouter `reasoning.effort`.
/// `"low"` / `"medium"` / `"high"` are widely accepted, but `"minimal"`,
/// `"xhigh"`, and `"max"` are model-dependent: some routes reject them with
/// HTTP 400. Prefer `Low`–`High` when talking to an unverified route; only
/// use `Minimal` / `XHigh` / `Max` against models known to accept them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningEffort {
    /// Disable extended reasoning when the model allows it.
    None,
    /// Lightest thinking budget above [`Self::None`]; fastest voice replies.
    Minimal,
    /// Light thinking budget for simple, low-stakes turns.
    Low,
    /// Balanced default for simple voice facts.
    Medium,
    /// Prefer this for agent / tool / multi-step work.
    #[default]
    High,
    /// Heavier budget than [`Self::High`] for harder multi-step planning.
    XHigh,
    /// Maximum thinking budget the provider allows.
    Max,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// Per-request reasoning controls attached to [`super::OpenRouterClient`].
///
/// # Default vs `Complex` budget — intentional divergence
///
/// - [`ReasoningConfig::default`] is `High` + `exclude: true`: the cheap
///   no-trace default. The model still thinks hard, but reasoning text is
///   kept off the wire so voice payloads stay small (the agent only needs
///   `content` / `tool_calls`).
/// - [`crate::RequestStage::Complex::budget()`](crate::RequestStage::budget)
///   is `High` + `exclude: false`: the trace-for-UI budget. Reasoning text
///   is streamed so the desktop UI can show live thinking.
///
/// Both think equally hard; they differ only in whether the trace is
/// returned. Pick `default()` when nobody watches the trace, `Complex` when
/// the UI should stream it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningConfig {
    pub effort: ReasoningEffort,
    /// When true, model still thinks but reasoning text is omitted from the response.
    /// Preferred for voice (keeps payloads small; agent only needs content/tool_calls).
    pub exclude: bool,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        // Always think by default — "dumber without thinking" is worse than latency.
        // Cheap no-trace variant: High effort, trace excluded from the wire.
        // Contrast RequestStage::Complex::budget(), which is High effort with
        // the trace INCLUDED so the UI can stream thinking. See the
        // ReasoningConfig type docs for the full divergence rationale.
        Self {
            effort: ReasoningEffort::High,
            exclude: true,
        }
    }
}

impl ReasoningConfig {
    /// [`ReasoningEffort::High`], reasoning text excluded from the response body.
    pub fn high() -> Self {
        Self {
            effort: ReasoningEffort::High,
            exclude: true,
        }
    }

    /// [`ReasoningEffort::Medium`], reasoning text excluded from the response body.
    pub fn medium() -> Self {
        Self {
            effort: ReasoningEffort::Medium,
            exclude: true,
        }
    }

    /// [`ReasoningEffort::Low`], reasoning text excluded from the response body.
    pub fn low() -> Self {
        Self {
            effort: ReasoningEffort::Low,
            exclude: true,
        }
    }

    /// [`ReasoningEffort::Minimal`], reasoning text excluded from the response body.
    pub fn minimal() -> Self {
        Self {
            effort: ReasoningEffort::Minimal,
            exclude: true,
        }
    }

    /// Include reasoning text in the stream / response so a host can show it.
    ///
    /// Does not change effort — except when effort is [`ReasoningEffort::None`]:
    /// asking for the trace while reasoning is disabled would otherwise be a
    /// silent no-op, so effort is upgraded to [`ReasoningEffort::Minimal`].
    /// The agent still ignores this text for speech and context — only the
    /// SSE consumer should surface it.
    pub fn include_text(mut self) -> Self {
        self.exclude = false;
        if matches!(self.effort, ReasoningEffort::None) {
            self.effort = ReasoningEffort::Minimal;
        }
        self
    }

    /// JSON object for the OpenRouter `reasoning` field.
    ///
    /// Always returns `Some` — OpenRouter's unified `reasoning` object is
    /// sent on every request, including [`ReasoningEffort::None`] (which
    /// explicitly sets `"enabled": false` rather than omitting the field,
    /// so disabling reasoning is unambiguous instead of relying on
    /// provider-default behavior).
    pub fn to_request_value(&self) -> Value {
        if matches!(self.effort, ReasoningEffort::None) {
            return json!({ "effort": "none", "exclude": true, "enabled": false });
        }
        json!({
            "effort": self.effort.as_str(),
            "exclude": self.exclude,
            "enabled": true,
        })
    }
}

/// Default completion token headroom so reasoning + final answer both fit.
///
/// OpenRouter maps `effort` as a fraction of `max_tokens` on some models;
/// without headroom, high effort can starve tool_calls / spoken content.
pub const DEFAULT_MAX_TOKENS: u32 = 24_576;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_config_enables_reasoning() {
        let v = ReasoningConfig::high().to_request_value();
        assert_eq!(v["effort"], "high");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["exclude"], true);
    }

    #[test]
    fn include_text_clears_exclude() {
        let v = ReasoningConfig::high().include_text().to_request_value();
        assert_eq!(v["effort"], "high");
        assert_eq!(v["exclude"], false);
        assert_eq!(v["enabled"], true);
    }

    #[test]
    fn none_effort_disables_reasoning_explicitly() {
        let c = ReasoningConfig {
            effort: ReasoningEffort::None,
            exclude: true,
        };
        let v = c.to_request_value();
        assert_eq!(v["effort"], "none");
        // Must explicitly disable, not just omit `enabled`, so behavior
        // doesn't depend on unverified provider-default assumptions.
        assert_eq!(v["enabled"], false);
    }

    #[test]
    fn include_text_upgrades_none_effort_to_minimal() {
        // Asking for the trace while disabled must not be silently discarded.
        let c = ReasoningConfig {
            effort: ReasoningEffort::None,
            exclude: true,
        }
        .include_text();
        assert_eq!(c.effort, ReasoningEffort::Minimal);
        assert!(!c.exclude);
        let v = c.to_request_value();
        assert_eq!(v["effort"], "minimal");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["exclude"], false);
    }

    #[test]
    fn default_is_cheap_no_trace_high() {
        let d = ReasoningConfig::default();
        assert_eq!(d.effort, ReasoningEffort::High);
        assert!(d.exclude, "default keeps the trace off the wire");
    }
}
