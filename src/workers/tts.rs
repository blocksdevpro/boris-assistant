use std::sync::mpsc::{Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_core::event::Event;
use boris_inference::TextToSpeech;
use boris_tts_kokoro::KokoroTts;

// ── Commands ─────────────────────────────────────────────────────────────────

pub enum TTSCommand {
    LoadModel,
    /// Synthesize the given text and send the audio back for playback.
    Synthesize(String),
}

// ── TTS Worker ────────────────────────────────────────────────────────────────

pub struct TTSWorker {
    _handle: JoinHandle<()>,
}

impl TTSWorker {
    /// Spawn the TTS worker on its own thread.
    pub fn spawn(
        command_rx: Receiver<TTSCommand>,
        event_tx: Sender<Event>,
        mut tts: KokoroTts,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    TTSCommand::LoadModel => {
                        if let Err(e) = tts.load() {
                            tracing::error!("Failed to load TTS model: {e}");
                        }
                    }
                    TTSCommand::Synthesize(text) => {
                        tracing::debug!("TTS synthesizing: \"{}\"", text);
                        
                        match tts.synthesize(&text) {
                            Ok(samples) => {
                                event_tx.send(Event::PlaybackReady(samples)).ok();
                            }
                            Err(e) => {
                                tracing::error!("TTS synthesis error: {e}");
                            }
                        }
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
