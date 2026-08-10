//! LiveKit open-wake-word ONNX backend (mel → embedding → classifier).
//!
//! The type name is `LivekitWakeWord` (historical); it is the LiveKit adapter.
//! Prefer the [`LiveKitWakeWord`] alias when writing new code.

use std::collections::HashMap;
use std::fmt;

use livekit_wakeword::WakeWordModel;

use boris_core::{AudioSample, Error, Result};

use crate::pcm::f32_to_pcm16_samples_into;
use crate::wake::WakeWord;

/// Wake-word scorer backed by embedded ONNX weights (LiveKit open-wake-word).
///
/// Construct with [`Self::try_new`] after [`crate::init_onnx_runtime`].
/// `model_bytes` must be a complete open-wake-word classifier ONNX blob; the
/// mel + embedding graphs are bundled inside `livekit-wakeword`.
pub struct LivekitWakeWord {
    model: WakeWordModel,
    /// Classifier key registered with the model (used for multi-label score pick).
    model_name: String,
    /// Reused PCM scratch for the f32 → i16 conversion on the hot path.
    pcm_scratch: Vec<i16>,
}

/// Canonical branding alias for [`LivekitWakeWord`] (LiveKit adapter).
pub type LiveKitWakeWord = LivekitWakeWord;

impl LivekitWakeWord {
    /// Load from embedded model bytes (desktop compiles weights into the binary).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if ORT cannot build the mel / embedding / classifier
    /// sessions (missing native ORT DLL, corrupt bytes, unsupported sample rate).
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
        Ok(Self {
            model,
            model_name: model_name.to_string(),
            pcm_scratch: Vec::new(),
        })
    }
}

impl fmt::Debug for LivekitWakeWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LivekitWakeWord")
            .field("model_name", &self.model_name)
            .finish_non_exhaustive()
    }
}

impl WakeWord for LivekitWakeWord {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32> {
        f32_to_pcm16_samples_into(audio, &mut self.pcm_scratch);
        let scores = self
            .model
            .predict(&self.pcm_scratch)
            .map_err(|e| Error::other(e.to_string()))?;
        Ok(select_wake_score(&scores, &self.model_name))
    }
}

/// Pick a single wake confidence from a multi-label score map.
///
/// Prefers the entry whose key matches `preferred_name` (exact, then
/// case-insensitive). Otherwise returns the **maximum** score so multi-label
/// models never depend on `HashMap` iteration order.
pub(crate) fn select_wake_score(scores: &HashMap<String, f32>, preferred_name: &str) -> f32 {
    if scores.is_empty() {
        return 0.0;
    }
    if let Some(&score) = scores.get(preferred_name) {
        return score;
    }
    if let Some((_, &score)) = scores
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(preferred_name))
    {
        return score;
    }
    scores
        .values()
        .copied()
        .reduce(f32::max)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_named_key() {
        let mut m = HashMap::new();
        m.insert("other".into(), 0.9);
        m.insert("boris".into(), 0.4);
        assert_eq!(select_wake_score(&m, "boris"), 0.4);
    }

    #[test]
    fn case_insensitive_name_match() {
        let mut m = HashMap::new();
        m.insert("Boris".into(), 0.55);
        m.insert("noise".into(), 0.99);
        assert_eq!(select_wake_score(&m, "boris"), 0.55);
    }

    #[test]
    fn falls_back_to_max_score() {
        let mut m = HashMap::new();
        m.insert("a".into(), 0.2);
        m.insert("b".into(), 0.8);
        m.insert("c".into(), 0.5);
        assert_eq!(select_wake_score(&m, "missing"), 0.8);
    }

    #[test]
    fn empty_map_is_zero() {
        assert_eq!(select_wake_score(&HashMap::new(), "boris"), 0.0);
    }
}
