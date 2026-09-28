//! Agent statistics collected via the event subscriber pattern (tau-inspired).
//!
//! ```ignore
//! let stats = AgentStats::new();
//! let unsub = agent.subscribe(stats.handler());
//! agent.prompt("hello").await?;
//! println!("{}", stats.summary());
//! drop(unsub);
//! ```

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::types::AgentEvent;

/// Zero-overhead counters filled by [`AgentStats::handler`].
///
/// `turns` counts LLM rounds (one per `TurnStart` event), not user turns: a
/// single user turn that calls tools runs several LLM rounds. Prefer
/// [`Self::llm_rounds`] for new code; [`Self::turns`] is kept as a compat
/// alias returning the same value.
#[derive(Debug)]
pub struct AgentStats {
    started: Instant,
    turns: AtomicU32,
    llm_rounds: AtomicU32,
    tools: AtomicU32,
    /// All HITL pauses: `NeedsConfirmation` + `NeedsInput`.
    confirms: AtomicU32,
    errors: AtomicU32,
    total_tool_ms: AtomicU64,
}

impl Default for AgentStats {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentStats {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            turns: AtomicU32::new(0),
            llm_rounds: AtomicU32::new(0),
            tools: AtomicU32::new(0),
            confirms: AtomicU32::new(0),
            errors: AtomicU32::new(0),
            total_tool_ms: AtomicU64::new(0),
        }
    }

    /// Closure suitable for [`crate::Agent::subscribe`].
    ///
    /// Each `TurnStart` is one LLM round (tool-loop iterations re-emit it, so
    /// one user turn contributes several). Both HITL pause events —
    /// `NeedsConfirmation` and `NeedsInput` — count toward the HITL rate.
    pub fn handler(self: &Arc<Self>) -> impl Fn(&AgentEvent) + Send + Sync + 'static {
        let this = Arc::clone(self);
        move |event: &AgentEvent| match event {
            AgentEvent::TurnStart { .. } => {
                this.turns.fetch_add(1, Ordering::Relaxed);
                this.llm_rounds.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::ToolExecutionEnd { duration_ms, .. } => {
                this.tools.fetch_add(1, Ordering::Relaxed);
                this.total_tool_ms
                    .fetch_add(*duration_ms, Ordering::Relaxed);
            }
            AgentEvent::NeedsConfirmation { .. } | AgentEvent::NeedsInput { .. } => {
                this.confirms.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::Error { .. } => {
                this.errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// LLM rounds observed (one per `TurnStart`). Compat name for
    /// [`Self::llm_rounds`]; both return the same counter.
    pub fn turns(&self) -> u32 {
        self.turns.load(Ordering::Relaxed)
    }

    /// LLM rounds observed (one per `TurnStart`, so several per user turn
    /// when tools run). Same counter as [`Self::turns`].
    pub fn llm_rounds(&self) -> u32 {
        self.llm_rounds.load(Ordering::Relaxed)
    }

    pub fn tools(&self) -> u32 {
        self.tools.load(Ordering::Relaxed)
    }

    /// HITL pauses observed (`NeedsConfirmation` + `NeedsInput`).
    pub fn confirms(&self) -> u32 {
        self.confirms.load(Ordering::Relaxed)
    }

    /// Alias for [`Self::confirms`]: all HITL pauses (confirm + typed input).
    pub fn hitl_pauses(&self) -> u32 {
        self.confirms.load(Ordering::Relaxed)
    }

    pub fn errors(&self) -> u32 {
        self.errors.load(Ordering::Relaxed)
    }

    pub fn total_tool_ms(&self) -> u64 {
        self.total_tool_ms.load(Ordering::Relaxed)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn summary(&self) -> String {
        format!(
            "=== Agent Statistics ===\n\
             Elapsed: {}ms\n\
             LLM rounds: {}\n\
             Tool executions: {}\n\
             HITL pauses (confirm/input): {}\n\
             Errors: {}\n\
             Tool time: {}ms\n",
            self.elapsed_ms(),
            self.llm_rounds(),
            self.tools(),
            self.hitl_pauses(),
            self.errors(),
            self.total_tool_ms(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::PendingToolCall;
    use crate::tool::ToolRisk;

    fn pending(name: &str) -> PendingToolCall {
        PendingToolCall::new(
            format!("{name}-id"),
            name,
            serde_json::json!({}),
            name,
            ToolRisk::Safe,
            format!("{name}-call"),
        )
    }

    #[test]
    fn turn_start_counts_llm_rounds_not_user_turns() {
        let stats = Arc::new(AgentStats::new());
        let handle = stats.handler();
        // One user turn with two tool-loop iterations emits two TurnStarts.
        handle(&AgentEvent::TurnStart { round: 0 });
        handle(&AgentEvent::TurnStart { round: 1 });
        assert_eq!(stats.turns(), 2);
        assert_eq!(stats.llm_rounds(), 2);
    }

    #[test]
    fn hitl_counts_confirm_and_input_pauses() {
        let stats = Arc::new(AgentStats::new());
        let handle = stats.handler();
        handle(&AgentEvent::NeedsConfirmation {
            pending: pending("bash"),
        });
        handle(&AgentEvent::NeedsInput {
            pending: pending("collect_input"),
        });
        assert_eq!(stats.confirms(), 2);
        assert_eq!(stats.hitl_pauses(), 2);
    }

    #[test]
    fn summary_labels_llm_rounds_and_hitl() {
        let stats = Arc::new(AgentStats::new());
        let handle = stats.handler();
        handle(&AgentEvent::TurnStart { round: 0 });
        handle(&AgentEvent::NeedsInput {
            pending: pending("collect_input"),
        });
        let summary = stats.summary();
        assert!(summary.contains("LLM rounds: 1"), "{summary}");
        assert!(
            summary.contains("HITL pauses (confirm/input): 1"),
            "{summary}"
        );
    }
}
