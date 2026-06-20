use std::{
    sync::mpsc::{Receiver, Sender},
    thread::{self, JoinHandle},
};

use boris_core::{AudioBuffer, event::Event};
use boris_inference::SpeechToText;

pub struct STTWorker {
    _handle: JoinHandle<()>,
}

impl STTWorker {
    pub fn spawn(
        audio_rx: Receiver<AudioBuffer>,
        event_tx: Sender<Event>,
        mut stt: impl SpeechToText + 'static,
    ) -> Self {
        let handle = thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                if let Ok(text) = stt.transcribe(&audio) {
                    event_tx.send(Event::SpeechToTextResult(text)).ok();
                }
            }
        });
        Self { _handle: handle }
    }
}
