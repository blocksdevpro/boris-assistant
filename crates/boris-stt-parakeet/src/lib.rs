use std::path::{Path, PathBuf};

use boris_core::{error::Result, AudioSample};
use boris_inference::SpeechToText;
use transcribe_rs::{
    onnx::{parakeet::ParakeetModel, Quantization},
    SpeechModel, TranscribeOptions,
};

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

    /// Legacy: `./assets/models/parakeet` under CWD.
    pub fn new() -> Self {
        Self::with_model_dir(PathBuf::from("./assets/models/parakeet"))
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }
}

impl Default for ParakeetStt {
    fn default() -> Self {
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
            return Err(boris_core::error::Error::Other(format!(
                "parakeet model dir not found: {}",
                dir.display()
            )));
        }

        // Prefer int8 when present (transcribe-rs falls back itself; we log path clearly).
        tracing::info!(path = %dir.display(), "loading Parakeet STT");
        let t0 = std::time::Instant::now();
        let model = ParakeetModel::load(dir, &Quantization::Int8).map_err(|e| {
            tracing::error!(
                error = %e,
                path = %dir.display(),
                "ParakeetModel::load failed"
            );
            boris_core::error::Error::Other(format!("{e} (dir={})", dir.display()))
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
            .ok_or_else(|| boris_core::error::Error::Other("STT model not loaded".into()))?;

        let result = model
            .transcribe(
                audio,
                &TranscribeOptions {
                    language: Some("en".to_string()),
                    ..Default::default()
                },
            )
            .map_err(|e| boris_core::error::Error::Other(e.to_string()))?;

        Ok(result.text)
    }
}
