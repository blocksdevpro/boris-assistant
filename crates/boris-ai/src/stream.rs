//! Lightweight in-process event channel (optional helper).
//!
//! The production voice path uses [`crate::LlmClient::complete`] (OpenRouter
//! may stream **internally** and assemble a full message). This module is a
//! small mpsc wrapper for future UI/partial-token adapters — it is **not**
//! wired into the agent loop today.

use tokio::sync::mpsc;

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
