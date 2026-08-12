//! In-process tool progress (Grok-lite streaming without ToolStream rewrite).
//!
//! Tools call [`ToolCallContext::report`]; the sink maps to slim
//! [`crate::types::AgentEvent::ToolProgress`] for the host UI.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::{AgentEvent, EmitFn as TypesEmitFn};

/// Alias for the shared emit handle.
pub type EmitFn = TypesEmitFn;

/// Internal progress payload from a tool body.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Text {
        text: String,
    },
    Chunk {
        delta: String,
        total_bytes: u64,
        truncated: bool,
    },
    Custom {
        subkind: String,
        payload: serde_json::Value,
    },
}

/// Host/runtime sink for progress events.
pub trait ProgressSink: Send + Sync {
    fn emit(&self, event: ProgressEvent);
}

/// No-op sink (tests / tools without a host).
#[derive(Debug, Default, Clone, Copy)]
pub struct NullProgressSink;

impl ProgressSink for NullProgressSink {
    fn emit(&self, _event: ProgressEvent) {}
}

/// Maps progress → slim `AgentEvent::ToolProgress` with rate limiting.
pub struct EventProgressSink {
    emit: EmitFn,
    call_id: String,
    tool_name: String,
    /// Min interval between UI events (ms).
    min_interval_ms: u64,
    last_emit_ms: AtomicU64,
    message_max_chars: usize,
}

impl EventProgressSink {
    pub fn new(emit: EmitFn, call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            emit,
            call_id: call_id.into(),
            tool_name: tool_name.into(),
            min_interval_ms: 150, // ~6–7/s max
            last_emit_ms: AtomicU64::new(0),
            message_max_chars: 120,
        }
    }

    pub fn into_arc(self) -> Arc<dyn ProgressSink> {
        Arc::new(self)
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn should_emit(&self) -> bool {
        let now = Self::now_ms();
        let last = self.last_emit_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) < self.min_interval_ms {
            return false;
        }
        self.last_emit_ms.store(now, Ordering::Relaxed);
        true
    }

    fn truncate_msg(s: &str, max: usize) -> String {
        let count = s.chars().count();
        if count <= max {
            return s.to_string();
        }
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }

    fn map_message(event: &ProgressEvent, max: usize) -> (String, Option<u64>) {
        match event {
            ProgressEvent::Text { text } => {
                let line = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or(text);
                (Self::truncate_msg(line.trim(), max), None)
            }
            ProgressEvent::Chunk {
                delta,
                total_bytes,
                truncated,
            } => {
                let line = delta
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or(delta);
                let mut msg = Self::truncate_msg(line.trim(), max);
                if *truncated && !msg.is_empty() {
                    msg.push('…');
                }
                (msg, Some(*total_bytes))
            }
            ProgressEvent::Custom { subkind, .. } => (Self::truncate_msg(subkind, max), None),
        }
    }
}

impl ProgressSink for EventProgressSink {
    fn emit(&self, event: ProgressEvent) {
        if !self.should_emit() {
            return;
        }
        let (message, byte_total) = Self::map_message(&event, self.message_max_chars);
        if message.is_empty() {
            return;
        }
        (self.emit)(AgentEvent::ToolProgress {
            call_id: self.call_id.clone(),
            tool_name: self.tool_name.clone(),
            message,
            byte_total,
        });
    }
}
