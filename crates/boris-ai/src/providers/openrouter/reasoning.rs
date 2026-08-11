//! OpenRouter unified `reasoning` request controls.
//!
//! See <https://openrouter.ai/docs/guides/best-practices/reasoning-tokens>.
//! Models that support thinking (DeepSeek, Gemini thinking, Claude, o-series)
//! use this object; others ignore it.

use serde_json::{json, Value};

/// How hard the model should think before answering / calling tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningEffort {
    /// Disable extended reasoning when the model allows it.
    None,
    Minimal,
    Low,
    /// Balanced default for simple voice facts.
    Medium,
    /// Prefer this for agent / tool / multi-step work.
    #[default]
    High,
    XHigh,
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
        Self {
            effort: ReasoningEffort::High,
            exclude: true,
        }
    }
}

impl ReasoningConfig {
    pub fn high() -> Self {
        Self {
            effort: ReasoningEffort::High,
            exclude: true,
        }
    }

    pub fn medium() -> Self {
        Self {
            effort: ReasoningEffort::Medium,
            exclude: true,
        }
    }

    pub fn low() -> Self {
        Self {
            effort: ReasoningEffort::Low,
            exclude: true,
        }
    }

    /// JSON object for the OpenRouter `reasoning` field (None when effort is None).
    pub fn to_request_value(&self) -> Option<Value> {
        if matches!(self.effort, ReasoningEffort::None) {
            return Some(json!({ "effort": "none", "exclude": true }));
        }
        Some(json!({
            "effort": self.effort.as_str(),
            "exclude": self.exclude,
            "enabled": true,
        }))
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
        let v = ReasoningConfig::high().to_request_value().unwrap();
        assert_eq!(v["effort"], "high");
        assert_eq!(v["enabled"], true);
        assert_eq!(v["exclude"], true);
    }

    #[test]
    fn none_effort_sends_none() {
        let c = ReasoningConfig {
            effort: ReasoningEffort::None,
            exclude: true,
        };
        let v = c.to_request_value().unwrap();
        assert_eq!(v["effort"], "none");
    }
}
