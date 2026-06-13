use std::{
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use boris_core::AudioSampleBuffer;

pub struct AudioProcessor {
    _handle: JoinHandle<()>,
}

impl AudioProcessor {
    pub fn spawn(
        audio_rx: Receiver<AudioSampleBuffer>,
        audio_txs: Vec<Sender<AudioSampleBuffer>>,
    ) -> Self {
        let handle = std::thread::spawn(move || {
            loop {
                if let Ok(audio) = audio_rx.recv() {
                    for tx in &audio_txs {
                        tx.send(audio.clone()).ok();
                    }
                }
            }
        });
        Self { _handle: handle }
    }
}
