//! Typed LLM stream events plus a small in-process event channel.

use serde_json::Value;
use tokio::sync::mpsc;

use crate::usage::TokenUsage;

/// Incremental events from [`crate::LlmClient::complete_stream`].
///
/// The existing blocking [`crate::LlmClient::complete`] path stays compatible:
/// implementations may still assemble internally and return only the final message.
#[derive(Debug, Clone, PartialEq)]
pub enum LlmStreamEvent {
    /// Request is about to be sent.
    ModelSend {
        model: String,
    },
    /// First content or tool-call delta arrived (time-to-first-byte).
    FirstDelta {
        ttfb_ms: u64,
    },
    /// Incremental assistant text.
    ContentDelta {
        text: String,
    },
    /// Incremental tool-call fragment. Never execute until [`Self::ToolCallComplete`].
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: String,
    },
    /// One tool call has a complete `id`, `name`, and arguments string.
    /// Arguments may still fail JSON / schema validation — do not execute yet.
    ToolCallComplete {
        index: u32,
        id: String,
        name: String,
        arguments: String,
    },
    Usage(TokenUsage),
    /// Fully assembled `choices[0].message` (same shape as `complete()`).
    FinalMessage(Value),
}

impl LlmStreamEvent {
    pub fn is_toolish(&self) -> bool {
        matches!(
            self,
            Self::ToolCallDelta { .. } | Self::ToolCallComplete { .. }
        )
    }
}

/// Receiver half of an unbounded event stream.
///
/// Ends when the sender is dropped (and the queue drains).
pub struct EventStream<T> {
    rx: mpsc::UnboundedReceiver<T>,
}

impl<T> EventStream<T> {
    /// Wait for the next event, or `None` if the sender was dropped.
    pub async fn next(&mut self) -> Option<T> {
        self.rx.recv().await
    }
}

/// Sender half; [`EventStreamSender::push`] enqueues one event.
pub struct EventStreamSender<T> {
    tx: mpsc::UnboundedSender<T>,
}

impl<T> EventStreamSender<T> {
    /// Push an event. Silently drops if the receiver is gone.
    pub fn push(&self, event: T) {
        let _ = self.tx.send(event);
    }

    /// Whether the receiver is still alive.
    pub fn is_open(&self) -> bool {
        !self.tx.is_closed()
    }
}

/// Create a paired unbounded event stream.
pub fn event_stream<T>() -> (EventStreamSender<T>, EventStream<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (EventStreamSender { tx }, EventStream { rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn push_and_recv() {
        let (tx, mut rx) = event_stream::<u32>();
        assert!(tx.is_open());
        tx.push(1);
        tx.push(2);
        drop(tx);
        assert_eq!(rx.next().await, Some(1));
        assert_eq!(rx.next().await, Some(2));
        assert_eq!(rx.next().await, None);
    }
}
