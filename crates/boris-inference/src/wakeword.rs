use crate::WakeWordDetector;
use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, error::BorisResult};

pub struct BorisWakeWord {
    model: WakeWordModel,
}

impl BorisWakeWord {
    pub fn new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Self {
        Self {
            model: WakeWordModel::with_bytes(model_name, model_bytes, sample_rate).unwrap(),
        }
    }
}

impl WakeWordDetector for BorisWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<f32> {
        // convert the f32 audio to i16 audio;
        let audio_i16: Vec<i16> = audio.iter().map(|&x| (x * 32767.0) as i16).collect();
        let result = self.model.predict(&audio_i16).unwrap();

        Ok(result.values().copied().next().unwrap_or(0.0))
    }
}
