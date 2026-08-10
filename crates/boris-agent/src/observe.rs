//! Per-turn observability types for the agent harness.

use std::time::Duration;

/// Coarse outcome label for metrics / UI (no payload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TurnOutcomeKind {
    /// Model produced speakable text.
    Speak,
    /// Model returned no speakable content.
    Silent,
    /// Tool loop paused for HITL confirmation.
    NeedsConfirm,
}

impl TurnOutcomeKind {
    /// Stable lowercase label (logging / JSON).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Speak => "speak",
            Self::Silent => "silent",
            Self::NeedsConfirm => "needs_confirm",
        }
    }
}

impl std::fmt::Display for TurnOutcomeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured summary of one completed agent turn.
///
/// Produced by [`crate::Agent::prompt_with_report`]. Failures do not
/// yield a report (context is rolled back and the error is returned instead).
#[derive(Debug, Clone)]
pub struct TurnReport {
    /// Wall time from turn entry to outcome (or until failure was detected).
    pub duration: Duration,
    /// Number of tool-call rounds that ran (0 if the model replied immediately).
    pub tool_rounds: u32,
    /// Tool names invoked, in order (may contain duplicates if the model
    /// called the same tool more than once).
    pub tools_used: Vec<String>,
    /// Coarse outcome kind (prefer this over stringly labels).
    pub outcome: TurnOutcomeKind,
    /// Rough serialized size of the conversation context dump after the turn.
    pub approx_chars_in: usize,
}
