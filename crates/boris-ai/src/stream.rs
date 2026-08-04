//! Lightweight event stream primitives (tau-inspired).
//!
//! Production voice path uses [`crate::LlmClient::complete`]. Streaming adapters
//! can push partial events through [`EventStream`] without changing the agent API.

use tokio::sync::mpsc;

/// Unbounded event stream; ends when the sender is dropped or a terminal event is pushed.
pub struct EventStream<T> {
    rx: mpsc::UnboundedReceiver<T>,
}

impl<T> EventStream<T> {
    pub async fn next(&mut self) -> Option<T> {
        self.rx.recv().await
    }
}

/// Sender half; call [`EventStreamSender::push`] for each event.
pub struct EventStreamSender<T> {
    tx: mpsc::UnboundedSender<T>,
}

impl<T> EventStreamSender<T> {
    pub fn push(&self, event: T) {
        let _ = self.tx.send(event);
    }
}

/// Create a paired event stream. Terminal detection is left to the consumer.
pub fn event_stream<T>() -> (EventStreamSender<T>, EventStream<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (EventStreamSender { tx }, EventStream { rx })
}
