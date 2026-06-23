use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
};

use boris_core::{AudioBuffer, event::Event};
use boris_inference::SpeechToText;

pub enum STTCommand {
    LoadModel,
    Transcribe(AudioBuffer),
}

pub struct STTWorker {
    _handle: JoinHandle<()>,
}

impl STTWorker {
    pub fn spawn(
        command_rx: Receiver<STTCommand>,
        event_tx: Sender<Event>,
        mut stt: impl SpeechToText + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    STTCommand::LoadModel => {
                        tracing::debug!("Loading STT model into memory...");
                        stt.load().ok();
                        tracing::debug!("STT model loaded successfully.");
                    }
                    STTCommand::Transcribe(audio) => {
                        if let Ok(text) = stt.transcribe(&audio) {
                            event_tx.send(Event::SpeechToTextResult(text)).ok();
                        }
                        tracing::debug!("Unloading STT model from memory...");
                        stt.unload().ok();
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
