use std::{
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
    thread::JoinHandle,
};

use boris_core::{AudioBuffer, AudioSample};

pub struct AudioProcessor {
    _handle: JoinHandle<()>,
}

impl AudioProcessor {
    pub fn spawn(
        audio_rx: Receiver<AudioBuffer>,
        audio_txs: Vec<Sender<Arc<[AudioSample]>>>,
    ) -> Self {
        let handle = std::thread::spawn(move || {
            while let Ok(audio) = audio_rx.recv() {
                let shared_audio: Arc<[AudioSample]> = Arc::from(audio);
                for tx in &audio_txs {
                    tx.send(shared_audio.clone()).ok();
                }
            }
        });
        Self { _handle: handle }
    }
}
