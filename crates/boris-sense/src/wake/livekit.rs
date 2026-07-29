use livekit_wakeword::WakeWordModel;

use boris_core::error::Result;
use boris_core::AudioSample;

use crate::pcm::f32_to_pcm16_samples;
use crate::wake::WakeWord;

pub struct LivekitWakeWord {
    model: WakeWordModel,
}

impl LivekitWakeWord {
    pub fn new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Self {
        Self {
            model: WakeWordModel::with_bytes(model_name, model_bytes, sample_rate)
                .expect("failed to initialise wakeword model from embedded bytes"),
        }
    }
}

impl WakeWord for LivekitWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32> {
        let pcm = f32_to_pcm16_samples(audio);
        let scores = self
            .model
            .predict(&pcm)
            .map_err(|e| boris_core::error::Error::Other(e.to_string()))?;
        Ok(scores.values().copied().next().unwrap_or(0.0))
    }
}
