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
    ///
    /// # Budgets
    ///
    /// - `SimpleVoice` → (`Minimal` + exclude, `768`).
    /// - `ToolPlanning` → (`Medium` + include text, `4_096`).
    /// - `Complex` → (`High` + include text, [`DEFAULT_MAX_TOKENS`]).
    ///
    /// Note the intentional divergence from [`ReasoningConfig::default`]:
    /// the type default is `High` + `exclude: true` (cheap: think hard, keep
    /// the trace off the wire), while [`RequestStage::Complex`] budgets
    /// `High` + `exclude: false` (trace kept so the UI can stream thinking).
    /// See [`ReasoningConfig`] docs for the full discussion.
    pub fn budget(self) -> (ReasoningConfig, u32) {
        match self {
            Self::SimpleVoice => (
                ReasoningConfig {
                    effort: ReasoningEffort::Minimal,
                    exclude: true,
                },
                // WARNING: 768 tokens is deliberately tight for short spoken
                // replies. A turn misclassified as `SimpleVoice` that grows
                // tool calls (or a reasoning-heavy model that spends most of
                // the pool thinking) can truncate its answer / tool_calls.
                // Callers that are unsure should prefer `ToolPlanning`.
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
///
/// Note: `Eq` is intentionally **not** derived — `temperature: Option<f32>`
/// cannot be `Eq`. `PartialEq` still holds for tests and option merging.
///
/// Note on [`CompleteOptions::default`]: the default is the expensive end
/// (`High` reasoning, [`DEFAULT_MAX_TOKENS`]) so a bare
/// `CompleteOptions::default()` never starves a thinking model. Prefer
/// [`CompleteOptions::for_stage`] (or [`CompleteOptions::simple`]) so each
/// turn pays only for the budget its stage needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompleteOptions {
    pub reasoning: Option<ReasoningConfig>,
    pub max_tokens: Option<u32>,
    pub stage: Option<RequestStage>,
    /// Optional sampling temperature, passed through as `"temperature"` when
    /// `Some`. `None` (default) omits the key so the provider default applies.
    pub temperature: Option<f32>,
}

impl CompleteOptions {
    /// Build options from a stage budget; explicit fields still win if set later.
    ///
    /// Callers should pass an explicit [`RequestStage`] for every turn rather
    /// than relying on [`CompleteOptions::default`]: the default is the
    /// expensive (`High` / [`DEFAULT_MAX_TOKENS`]) budget, while `for_stage`
    /// picks the cheapest budget that fits the turn.
    #[must_use]
    pub fn for_stage(stage: RequestStage) -> Self {
        let (reasoning, max_tokens) = stage.budget();
        Self {
            reasoning: Some(reasoning),
            max_tokens: Some(max_tokens),
            stage: Some(stage),
            temperature: None,
        }
    }

    /// Cheap short-reply budget (`SimpleVoice`); explicit fields still win.
    #[must_use]
    pub fn simple() -> Self {
        Self::for_stage(RequestStage::SimpleVoice)
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
    ///
    /// Per-request floor is 1. The client-level clamp
    /// (`OpenRouterClient::with_max_tokens`, owned by `client.rs`) agrees on
    /// the same floor of 1, so explicit per-request and client-level caps
    /// compose without one silently raising the other's minimum.
    pub fn resolved_max_tokens(&self, fallback: u32) -> u32 {
        if let Some(n) = self.max_tokens {
            return n.max(1);
        }
        if let Some(stage) = self.stage {
            return stage.budget().1;
        }
        fallback.max(1)
    }

    /// Resolve temperature: explicit per-request value wins, else `fallback`
    /// (provider clients pass `None` — there is no client-level temperature).
    pub fn resolved_temperature(&self, fallback: Option<f32>) -> Option<f32> {
        self.temperature.or(fallback)
    }
}

/// Clamp a resolved `max_tokens` to the model's context window.
///
/// - `None` window → only the per-request floor of 1 applies.
/// - `Some(w)` → `max_tokens.min(w)`, still floored at 1 (saturating).
///
/// Pure helper; the caller (`request_body_with`) emits the `tracing::warn!`
/// when clamping actually reduces the value.
pub fn clamp_to_context_window(max_tokens: u32, context_window: Option<u32>) -> u32 {
    match context_window {
        Some(w) => max_tokens.min(w).max(1),
        None => max_tokens.max(1),
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
            temperature: None,
        };
        assert_eq!(
            opts.resolved_reasoning(ReasoningConfig::high()).effort,
            ReasoningEffort::Low
        );
        assert_eq!(opts.resolved_max_tokens(24_576), 512);
    }

    #[test]
    fn clamp_to_context_window_caps_and_floors() {
        assert_eq!(clamp_to_context_window(24_576, Some(4_096)), 4_096);
        assert_eq!(clamp_to_context_window(1_024, Some(4_096)), 1_024);
        assert_eq!(clamp_to_context_window(24_576, None), 24_576);
        // Floor of 1 holds even against a degenerate window.
        assert_eq!(clamp_to_context_window(0, Some(4_096)), 1);
        assert_eq!(clamp_to_context_window(0, None), 1);
        assert_eq!(clamp_to_context_window(5, Some(0)), 1);
    }

    #[test]
    fn resolved_temperature_prefers_explicit() {
        let opts = CompleteOptions {
            temperature: Some(0.3),
            ..CompleteOptions::default()
        };
        assert_eq!(opts.resolved_temperature(None), Some(0.3));
        assert_eq!(opts.resolved_temperature(Some(0.9)), Some(0.3));

        let unset = CompleteOptions::default();
        assert_eq!(unset.resolved_temperature(None), None);
        assert_eq!(unset.resolved_temperature(Some(0.9)), Some(0.9));
    }

    #[test]
    fn simple_uses_simple_voice_budget() {
        assert_eq!(
            CompleteOptions::simple(),
            CompleteOptions::for_stage(RequestStage::SimpleVoice)
        );
    }
}
