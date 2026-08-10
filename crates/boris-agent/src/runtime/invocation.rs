//! Tool invocation request / response types for [`super::invoke::ToolRuntime`].

use serde_json::Value;

use crate::tool_context::ToolCallContext;

use super::pending::PendingToolCall;
use super::progress::ProgressSink;

/// One LLM-requested tool call entering the runtime.
#[derive(Clone)]
pub struct ToolInvocation {
    /// Provider / model tool call id (echoed in tool results).
    pub call_id: String,
    /// Registered tool name.
    pub name: String,
    /// JSON arguments object (non-objects coerced to `{}`).
    pub args: Value,
    /// Optional session id for audit / ctx.
    pub session_id: Option<String>,
    /// Optional turn id for audit / ctx.
    pub turn_id: Option<String>,
    /// Optional working directory for this call.
    pub cwd: Option<std::path::PathBuf>,
    /// Optional cancel token (cloned into [`ToolCallContext`]).
    pub cancel: Option<tokio_util::sync::CancellationToken>,
    /// Optional progress sink (host UI).
    pub progress: Option<std::sync::Arc<dyn ProgressSink>>,
}

impl std::fmt::Debug for ToolInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolInvocation")
            .field("call_id", &self.call_id)
            .field("name", &self.name)
            .field("args", &self.args)
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("cwd", &self.cwd)
            .field("has_progress", &self.progress.is_some())
            .finish()
    }
}

impl ToolInvocation {
    /// Build a minimal invocation (no session / cwd / cancel).
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            args,
            session_id: None,
            turn_id: None,
            cwd: None,
            cancel: None,
            progress: None,
        }
    }

    /// Derive the per-call context passed into [`Tool::execute`](crate::tool::Tool::execute).
    pub fn call_context(&self) -> ToolCallContext {
        ToolCallContext::new(self.call_id.clone())
            .with_session(self.session_id.clone(), self.turn_id.clone())
            .with_cwd(self.cwd.clone())
            .with_cancel(self.cancel.clone())
            .with_progress(self.progress.clone())
    }
}

/// Options for a single invoke.
#[derive(Debug, Clone, Copy)]
pub struct InvokeOptions {
    /// Skip HITL (one-shot grant after user approved).
    pub skip_confirmation: bool,
    /// Confirms already used this turn (for cap).
    pub confirms_used: u32,
}

impl Default for InvokeOptions {
    fn default() -> Self {
        Self {
            skip_confirmation: false,
            confirms_used: 0,
        }
    }
}

/// Result of runtime mediation (before the engine continues the ReAct loop).
#[derive(Debug, Clone)]
pub enum InvokeResult {
    /// Observation string for the model (already truncated + reminder).
    Observation(String),
    /// Pause for host HITL; tool was not executed.
    NeedsConfirmation {
        /// Pending call record for approve/reject.
        pending: PendingToolCall,
        /// Short speakable prompt for the voice UI.
        speak_prompt: String,
    },
    /// Hard deny — engine should feed this as an error observation.
    Denied {
        /// Human-readable deny reason.
        reason: String,
    },
}
