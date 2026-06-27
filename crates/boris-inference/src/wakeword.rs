use std::thread::JoinHandle;

use crate::{WakeWord, f32_to_pcm16_samples};
use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, error::Result};

pub struct LivekitWakeWord {
    model: WakeWordModel,
}

pub enum WakeWordCommand {
    StartListening,
    StopListening,
}

impl LivekitWakeWord {
    pub fn new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Self {
        Self {
            model: WakeWordModel::with_bytes(model_name, model_bytes, sample_rate).unwrap(),
        }
    }
}

impl WakeWord for LivekitWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32> {
        // convert the f32 audio to i16 audio;
        let pcm_samples = f32_to_pcm16_samples(audio);
        let result = self.model.predict(&pcm_samples).unwrap();

        Ok(result.values().copied().next().unwrap_or(0.0))
    }
}

pub struct WakeWordWorker {
    _handle: JoinHandle<()>,
}
