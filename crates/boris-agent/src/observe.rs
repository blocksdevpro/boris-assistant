//! Per-turn observability types for the agent harness.

use std::time::Duration;

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
    /// `"speak"` or `"silent"`.
    pub outcome: String,
    /// Rough serialized size of the conversation context dump after the turn.
    pub approx_chars_in: usize,
}
