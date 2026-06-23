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
                        println!("[BORIS] Loading STT model...");
                        stt.load().ok();
                        println!("[BORIS] STT model loaded.");
                    }
                    STTCommand::Transcribe(audio) => {
                        if let Ok(text) = stt.transcribe(&audio) {
                            event_tx.send(Event::SpeechToTextResult(text)).ok();
                        }
                        println!("[BORIS] Unloading STT model...");
                        stt.unload().ok();
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
