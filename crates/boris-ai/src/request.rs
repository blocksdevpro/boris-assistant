//! Per-request completion options (reasoning, token cap, stage).

use crate::providers::openrouter::{ReasoningConfig, ReasoningEffort, DEFAULT_MAX_TOKENS};

/// Coarse request stage used to pick reasoning effort and output-token ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStage {
    /// Greeting, time/date, or other short spoken reply.
    SimpleVoice,
    /// First-pass tool planning / ordinary tool round.
    ToolPlanning,
    /// Research, coding, multi-step, or escalated work.
    Complex,
}

impl RequestStage {
    /// Reasoning + `max_tokens` for this stage.
    pub fn budget(self) -> (ReasoningConfig, u32) {
        match self {
            Self::SimpleVoice => (
                ReasoningConfig {
                    effort: ReasoningEffort::Minimal,
                    exclude: true,
                },
                768,
            ),
            Self::ToolPlanning => (ReasoningConfig::medium().include_text(), 4_096),
            Self::Complex => (ReasoningConfig::high().include_text(), DEFAULT_MAX_TOKENS),
        }
    }
}

/// Optional per-call overrides for [`crate::LlmClient::complete_with_options`].
///
/// Empty fields fall back to the client's configured defaults (preserving
/// provider fallback behavior).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompleteOptions {
    pub reasoning: Option<ReasoningConfig>,
    pub max_tokens: Option<u32>,
    pub stage: Option<RequestStage>,
}

impl CompleteOptions {
    /// Build options from a stage budget; explicit fields still win if set later.
    pub fn for_stage(stage: RequestStage) -> Self {
        let (reasoning, max_tokens) = stage.budget();
        Self {
            reasoning: Some(reasoning),
            max_tokens: Some(max_tokens),
            stage: Some(stage),
        }
    }

    /// Resolve reasoning, applying the stage budget when unset.
    pub fn resolved_reasoning(&self, fallback: ReasoningConfig) -> ReasoningConfig {
        if let Some(r) = self.reasoning.clone() {
            return r;
        }
        if let Some(stage) = self.stage {
            return stage.budget().0;
        }
        fallback
    }

    /// Resolve max_tokens, applying the stage budget when unset.
    pub fn resolved_max_tokens(&self, fallback: u32) -> u32 {
        if let Some(n) = self.max_tokens {
            return n.max(1);
        }
        if let Some(stage) = self.stage {
            return stage.budget().1;
        }
        fallback.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_voice_uses_minimal_and_small_cap() {
        let (r, n) = RequestStage::SimpleVoice.budget();
        assert_eq!(r.effort, ReasoningEffort::Minimal);
        assert!(r.exclude, "short voice replies keep reasoning off the wire");
        assert!(n < 2_048);
    }

    #[test]
    fn complex_keeps_high_headroom() {
        let (r, n) = RequestStage::Complex.budget();
        assert_eq!(r.effort, ReasoningEffort::High);
        assert!(!r.exclude, "strong route must stream thinking for the UI");
        assert_eq!(n, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn tool_planning_includes_reasoning_text() {
        let (r, _) = RequestStage::ToolPlanning.budget();
        assert!(!r.exclude);
    }

    #[test]
    fn options_prefer_explicit_over_stage() {
        let opts = CompleteOptions {
            reasoning: Some(ReasoningConfig::low()),
            max_tokens: Some(512),
            stage: Some(RequestStage::Complex),
        };
        assert_eq!(
            opts.resolved_reasoning(ReasoningConfig::high()).effort,
            ReasoningEffort::Low
        );
        assert_eq!(opts.resolved_max_tokens(24_576), 512);
    }
}
