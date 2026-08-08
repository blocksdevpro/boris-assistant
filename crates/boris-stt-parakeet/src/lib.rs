//! Parakeet ONNX speech-to-text adapter (`SpeechToText`).
//!
//! Product path: construct with [`ParakeetStt::with_model_dir`] pointing at
//! `~/.boris/models/parakeet` (installed via pipeline download).

use std::path::{Path, PathBuf};

use boris_core::{AudioSample, Error, Result};
use boris_inference::SpeechToText;
use transcribe_rs::{
    onnx::{parakeet::ParakeetModel, Quantization},
    SpeechModel, TranscribeOptions,
};

/// Default language for transcription.
const DEFAULT_LANGUAGE: &str = "en";

/// Parakeet STT backend (lazy-loaded ONNX).
pub struct ParakeetStt {
    model: Option<ParakeetModel>,
    model_dir: PathBuf,
}

impl ParakeetStt {
    /// Explicit model directory (e.g. `~/.boris/models/parakeet`).
    pub fn with_model_dir(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model: None,
            model_dir: model_dir.into(),
        }
    }

    /// Legacy CWD path `./assets/models/parakeet`.
    ///
    /// Prefer [`Self::with_model_dir`] for product hosts.
    #[deprecated(note = "use ParakeetStt::with_model_dir(~/.boris/models/parakeet)")]
    pub fn new() -> Self {
        Self::with_model_dir(PathBuf::from("./assets/models/parakeet"))
    }

    /// Directory that will be passed to the ONNX loader.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Whether weights are currently loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }
}

impl Default for ParakeetStt {
    fn default() -> Self {
        #[allow(deprecated)]
        Self::new()
    }
}

impl SpeechToText for ParakeetStt {
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        let dir = &self.model_dir;
        if !dir.is_dir() {
            tracing::error!(path = %dir.display(), "parakeet model dir missing");
            return Err(Error::other(format!(
                "parakeet model dir not found: {}",
                dir.display()
            )));
        }

        // Prefer int8 when present (transcribe-rs falls back itself).
        tracing::info!(path = %dir.display(), "loading Parakeet STT");
        let t0 = std::time::Instant::now();
        let model = ParakeetModel::load(dir, &Quantization::Int8).map_err(|e| {
            tracing::error!(
                error = %e,
                path = %dir.display(),
                "ParakeetModel::load failed"
            );
            Error::other(format!("{e} (dir={})", dir.display()))
        })?;
        tracing::info!(
            path = %dir.display(),
            ms = t0.elapsed().as_millis() as u64,
            "Parakeet STT loaded"
        );
        self.model = Some(model);
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }

    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
        if self.model.is_none() {
            self.load()?;
        }

        let model = self
            .model
            .as_mut()
            .ok_or_else(|| Error::other("STT model not loaded"))?;

        let result = model
            .transcribe(
                audio,
                &TranscribeOptions {
                    language: Some(DEFAULT_LANGUAGE.to_string()),
                    ..Default::default()
                },
            )
            .map_err(|e| Error::other(e.to_string()))?;

        Ok(result.text)
    }
}
