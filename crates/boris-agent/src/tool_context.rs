//! Per-call context for tools (Grok-style `ToolCallContext`, voice-sized).
//!
//! Tools receive this on every `execute` so they can honor cancellation,
//! resolve relative paths against the session cwd, report progress, and tag
//! logs — without closing over host state in the tool constructor.

use std::path::PathBuf;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::runtime::{NullProgressSink, ProgressEvent, ProgressSink};

/// Context stamped onto every tool invocation by the runtime / loop.
#[derive(Clone)]
pub struct ToolCallContext {
    /// LLM-supplied tool call id (for correlation).
    pub call_id: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Working directory for relative path resolution (when known).
    pub cwd: Option<PathBuf>,
    /// Cooperative cancel for long-running tools (bash, web).
    pub cancel: Option<CancellationToken>,
    /// Optional host progress sink (rate-limited UI events).
    progress: Option<Arc<dyn ProgressSink>>,
}

impl Default for ToolCallContext {
    fn default() -> Self {
        Self {
            call_id: String::new(),
            session_id: None,
            turn_id: None,
            cwd: None,
            cancel: None,
            progress: None,
        }
    }
}

impl std::fmt::Debug for ToolCallContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolCallContext")
            .field("call_id", &self.call_id)
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("cwd", &self.cwd)
            .field("has_cancel", &self.cancel.is_some())
            .field("has_progress", &self.progress.is_some())
            .finish()
    }
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

    pub fn with_progress(mut self, progress: Option<Arc<dyn ProgressSink>>) -> Self {
        self.progress = progress;
        self
    }

    /// True when the host has requested cancellation.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.as_ref().is_some_and(|c| c.is_cancelled())
    }

    /// Emit a progress event to the host (no-op if no sink).
    pub fn report(&self, event: ProgressEvent) {
        if let Some(sink) = &self.progress {
            sink.emit(event);
        } else {
            NullProgressSink.emit(event);
        }
    }

    /// Convenience: report a short text progress line.
    pub fn report_text(&self, text: impl Into<String>) {
        self.report(ProgressEvent::Text { text: text.into() });
    }
}
