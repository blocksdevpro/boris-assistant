//! Per-call context for tools (Grok-style `ToolCallContext`, voice-sized).
//!
//! Tools receive this on every `execute` so they can honor cancellation,
//! resolve relative paths against the session cwd, report progress, and tag
//! logs — without closing over host state in the tool constructor.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::runtime::{NullProgressSink, ProgressEvent, ProgressSink};

/// An image a tool hands to the model.
///
/// Chat-completions tool messages are text-only on many providers, so the
/// loop delivers attachments as a following user-role message with an
/// `image_url` data-URL part. They are ephemeral: replaced by
/// `[image omitted]` at the next human turn and never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub call_id: String,
    pub tool_name: String,
    /// MIME type, e.g. `image/png`.
    pub mime: String,
    pub data_base64: String,
    /// Short provenance line shown next to the image.
    pub caption: String,
}

/// Shared queue of attachments produced during a turn (owned by the runtime).
pub type ImageInbox = Arc<Mutex<Vec<ImageAttachment>>>;

/// Context stamped onto every tool invocation by the runtime / loop.
#[derive(Clone, Default)]
pub struct ToolCallContext {
    /// LLM-supplied tool call id (for correlation).
    pub call_id: String,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    /// Working directory for relative path resolution (when known).
    ///
    /// Policy note: hosts/tools must not validate this with a lexical
    /// `starts_with` (case-sensitive, `..`/symlink-blind). Use
    /// `crate::runtime::path_within_root(&candidate, &root)` — the
    /// canonicalizing, case-insensitive-on-Windows helper — for any
    /// session-`cwd` fallback check. See `runtime::policy::paths` docs and the
    /// TODO there for `tools/bash/exec.rs`.
    pub cwd: Option<PathBuf>,
    /// Cooperative cancel for long-running tools (bash, web).
    pub cancel: Option<CancellationToken>,
    /// Optional host progress sink (rate-limited UI events).
    progress: Option<Arc<dyn ProgressSink>>,
    /// Image delivery to the model (set by the runtime; absent in bare tests).
    images: Option<ImageInbox>,
    /// The runtime's user hooks, so a tool that runs its own child runtime
    /// (`spawn_subagent`) applies them too. `None` when none are configured.
    hooks: Option<crate::hooks::Hooks>,
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

    pub fn with_images(mut self, images: Option<ImageInbox>) -> Self {
        self.images = images;
        self
    }

    pub fn with_hooks(mut self, hooks: Option<crate::hooks::Hooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// User hooks of the runtime that invoked this call, if any.
    pub fn hooks(&self) -> Option<&crate::hooks::Hooks> {
        self.hooks.as_ref()
    }

    /// True when this host can deliver images to the model.
    pub fn can_attach_images(&self) -> bool {
        self.images.is_some()
    }

    /// Queue an image for the model. Returns `false` when the host has no
    /// image delivery (the tool should then answer with text only).
    pub fn attach_image(
        &self,
        tool_name: &str,
        mime: &str,
        data_base64: String,
        caption: impl Into<String>,
    ) -> bool {
        let Some(images) = &self.images else {
            return false;
        };
        let Ok(mut queue) = images.lock() else {
            return false;
        };
        queue.push(ImageAttachment {
            call_id: self.call_id.clone(),
            tool_name: tool_name.to_string(),
            mime: mime.to_string(),
            data_base64,
            caption: caption.into(),
        });
        true
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
