use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_core::event::Event;
use boris_inference::TextToSpeech;

// ── Commands ──────────────────────────────────────────────────────────────────

pub enum TtsCommand {
    /// Load the TTS model into memory and pre-warm it.
    LoadModel,
    /// Synthesize the given text and emit [`Event::PlaybackReady`] with the PCM.
    Synthesize(String),
}

// ── TTS Worker ────────────────────────────────────────────────────────────────

pub struct TtsWorker {
    _handle: JoinHandle<()>,
}

impl TtsWorker {
    /// Spawn the TTS worker on its own thread.
    ///
    /// Accepts any [`TextToSpeech`] implementation — not tied to a concrete type.
    pub fn spawn(
        command_rx: Receiver<TtsCommand>,
        event_tx: Sender<Event>,
        mut tts: impl TextToSpeech + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    TtsCommand::LoadModel => {
                        if let Err(e) = tts.load() {
                            tracing::error!(error = %e, "TtsWorker: failed to load model");
                            event_tx.send(Event::WorkerError {
                                worker: "TtsWorker",
                                message: e.to_string(),
                            }).ok();
                        }
                    }
                    TtsCommand::Synthesize(text) => {
                        tracing::debug!(text, "TtsWorker: synthesizing");
                        match tts.synthesize(&text) {
                            Ok(samples) => {
                                event_tx.send(Event::PlaybackReady(samples)).ok();
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "TtsWorker: synthesis failed");
                                event_tx.send(Event::WorkerError {
                                    worker: "TtsWorker",
                                    message: e.to_string(),
                                }).ok();
                            }
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
