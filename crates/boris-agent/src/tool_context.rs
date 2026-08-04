//! Per-call context for tools (Grok-style `ToolCallContext`, voice-sized).
//!
//! Tools receive this on every `execute` so they can honor cancellation,
//! resolve relative paths against the session cwd, and tag logs — without
//! closing over host state in the tool constructor.

use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

/// Context stamped onto every tool invocation by the runtime / loop.
#[derive(Debug, Clone, Default)]
pub struct ToolCallContext {
    /// LLM-supplied tool call id (for correlation).
    pub call_id: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Working directory for relative path resolution (when known).
    pub cwd: Option<PathBuf>,
    /// Cooperative cancel for long-running tools (bash, web).
    pub cancel: Option<CancellationToken>,
}

impl ToolCallContext {
    pub fn new(call_id: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            ..Default::default()
        }
    }

    pub fn with_session(mut self, session_id: Option<String>, turn_id: Option<String>) -> Self {
        self.session_id = session_id;
        self.turn_id = turn_id;
        self
    }

    pub fn with_cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }

    pub fn with_cancel(mut self, cancel: Option<CancellationToken>) -> Self {
        self.cancel = cancel;
        self
    }

    /// True when the host has requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.is_cancelled())
    }
}
