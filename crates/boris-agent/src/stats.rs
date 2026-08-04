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
#[derive(Debug)]
pub struct AgentStats {
    started: Instant,
    turns: AtomicU32,
    tools: AtomicU32,
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
            tools: AtomicU32::new(0),
            confirms: AtomicU32::new(0),
            errors: AtomicU32::new(0),
            total_tool_ms: AtomicU64::new(0),
        }
    }

    /// Closure suitable for [`crate::Agent::subscribe`].
    pub fn handler(self: &Arc<Self>) -> impl Fn(&AgentEvent) + Send + Sync + 'static {
        let this = Arc::clone(self);
        move |event: &AgentEvent| match event {
            AgentEvent::TurnStart { .. } => {
                this.turns.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::ToolExecutionEnd { duration_ms, .. } => {
                this.tools.fetch_add(1, Ordering::Relaxed);
                this.total_tool_ms
                    .fetch_add(*duration_ms, Ordering::Relaxed);
            }
            AgentEvent::NeedsConfirmation { .. } => {
                this.confirms.fetch_add(1, Ordering::Relaxed);
            }
            AgentEvent::Error { .. } => {
                this.errors.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub fn turns(&self) -> u32 {
        self.turns.load(Ordering::Relaxed)
    }

    pub fn tools(&self) -> u32 {
        self.tools.load(Ordering::Relaxed)
    }

    pub fn confirms(&self) -> u32 {
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
             Confirmations: {}\n\
             Errors: {}\n\
             Tool time: {}ms\n",
            self.elapsed_ms(),
            self.turns(),
            self.tools(),
            self.confirms(),
            self.errors(),
            self.total_tool_ms(),
        )
    }
}
