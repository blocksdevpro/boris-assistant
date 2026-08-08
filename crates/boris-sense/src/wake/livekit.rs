//! LiveKit open-wake-word ONNX backend (mel → embedding → classifier).

use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, Error, Result};

use crate::pcm::f32_to_pcm16_samples;
use crate::wake::WakeWord;

/// Wake-word scorer backed by embedded ONNX weights.
pub struct LivekitWakeWord {
    model: WakeWordModel,
}

impl LivekitWakeWord {
    /// Load from embedded model bytes (desktop compiles weights into the binary).
    ///
    /// # Panics
    ///
    /// Panics if the model cannot be loaded — wake is required for the product
    /// mic path. Prefer [`Self::try_new`] when you need a recoverable error.
    pub fn new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Self {
        match Self::try_new(model_name, model_bytes, sample_rate) {
            Ok(w) => w,
            Err(e) => panic!("{e}"),
        }
    }

    /// Fallible constructor for tests / alternate hosts.
    pub fn try_new(model_name: &str, model_bytes: &[u8], sample_rate: u32) -> Result<Self> {
        tracing::info!(
            %model_name,
            bytes = model_bytes.len(),
            sample_rate,
            "LivekitWakeWord::try_new — loading ORT sessions from bytes"
        );
        let model = WakeWordModel::with_bytes(model_name, model_bytes, sample_rate).map_err(
            |e| {
                tracing::error!(
                    error = %e,
                    %model_name,
                    bytes = model_bytes.len(),
                    sample_rate,
                    "LivekitWakeWord load FAILED (check onnxruntime.dll / DirectML.dll next to exe)"
                );
                Error::other(format!(
                    "failed to initialise wakeword model from embedded bytes: {e}"
                ))
            },
        )?;
        tracing::info!(%model_name, "LivekitWakeWord model ready");
        Ok(Self { model })
    }
}

impl WakeWord for LivekitWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32> {
        let pcm = f32_to_pcm16_samples(audio);
        let scores = self
            .model
            .predict(&pcm)
            .map_err(|e| Error::other(e.to_string()))?;
        // Multi-label models may return several keys; product uses a single score.
        Ok(scores.values().copied().next().unwrap_or(0.0))
    }
}
