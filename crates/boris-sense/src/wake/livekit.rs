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
        tracing::info!(
            %model_name,
            bytes = model_bytes.len(),
            sample_rate,
            "LivekitWakeWord::new — loading ORT sessions from embedded bytes"
        );
        let model = match WakeWordModel::with_bytes(model_name, model_bytes, sample_rate) {
            Ok(m) => {
                tracing::info!(%model_name, "LivekitWakeWord model ready");
                m
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    %model_name,
                    bytes = model_bytes.len(),
                    sample_rate,
                    "LivekitWakeWord load FAILED (check onnxruntime.dll / DirectML.dll next to exe)"
                );
                panic!("failed to initialise wakeword model from embedded bytes: {e}");
            }
        };
        Self { model }
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
