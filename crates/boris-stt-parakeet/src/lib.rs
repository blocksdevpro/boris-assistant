//! Parakeet ONNX speech-to-text adapter ([`SpeechToText`](boris_inference::SpeechToText)).
//!
//! # Model contract
//!
//! Directory layout expected by `transcribe-rs` Parakeet (int8 product path):
//!
//! - `encoder-model.int8.onnx` (or `encoder-model.onnx` for FP32)
//! - `decoder_joint-model.int8.onnx` (or FP32 sibling)
//! - `nemo128.onnx`
//! - `vocab.txt` (must include blank `<blk>`)
//!
//! Product path: construct with [`ParakeetStt::with_model_dir`] pointing at
//! `~/.boris/models/parakeet` (installed via pipeline download).
//!
//! # Load policy
//!
//! Explicit [`SpeechToText::load`] is preferred. [`SpeechToText::transcribe`]
//! also lazy-loads if weights are not yet open. Missing or incomplete model
//! dirs map to [`boris_core::Error::Config`]. Empty audio returns `Ok("")`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use boris_core::{AudioSample, Error, Result};
use boris_inference::SpeechToText;
use transcribe_rs::{
    onnx::{parakeet::ParakeetModel, Quantization},
    SpeechModel, TranscribeOptions,
};

/// Backend id returned by [`SpeechToText::backend_id`].
pub const BACKEND_ID: &str = "parakeet";

/// Default language for transcription.
const DEFAULT_LANGUAGE: &str = "en";

/// Files always required regardless of quantization (preprocessor + vocab).
const REQUIRED_ALWAYS: &[&str] = &["nemo128.onnx", "vocab.txt"];

/// Parakeet STT backend (lazy-loaded ONNX).
///
/// Construct with [`ParakeetStt::with_model_dir`]. There is intentionally no
/// `Default` impl — hosts must supply an explicit model directory.
pub struct ParakeetStt {
    model: Option<ParakeetModel>,
    model_dir: PathBuf,
    language: String,
    quantization: Quantization,
}

impl ParakeetStt {
    /// Explicit model directory (e.g. `~/.boris/models/parakeet`).
    pub fn with_model_dir(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model: None,
            model_dir: model_dir.into(),
            language: DEFAULT_LANGUAGE.to_string(),
            quantization: Quantization::Int8,
        }
    }

    /// Language tag passed to the model (default `"en"`).
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Weight quantization preference (default [`Quantization::Int8`]).
    pub fn with_quantization(mut self, quantization: Quantization) -> Self {
        self.quantization = quantization;
        self
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

    /// Configured language tag.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Whether weights are currently loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }
}

impl fmt::Debug for ParakeetStt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParakeetStt")
            .field("model_dir", &self.model_dir)
            .field("language", &self.language)
            .field("quantization", &format_quantization(&self.quantization))
            .field("loaded", &self.model.is_some())
            .finish()
    }
}

fn format_quantization(q: &Quantization) -> &'static str {
    match q {
        Quantization::Int8 => "int8",
        Quantization::FP32 => "fp32",
        // Forward-compat if transcribe-rs adds more variants.
        _ => "other",
    }
}

/// Encoder / decoder basenames for the selected quantization.
fn quantized_pair(q: &Quantization) -> (&'static str, &'static str) {
    match q {
        Quantization::FP32 => ("encoder-model.onnx", "decoder_joint-model.onnx"),
        // Int8 and any unknown prefer int8 product filenames.
        _ => (
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
        ),
    }
}

/// Preflight: dir exists and required files are present for `quantization`.
fn preflight_model_dir(dir: &Path, quantization: &Quantization) -> Result<()> {
    if !dir.exists() {
        return Err(Error::config(format!(
            "parakeet model dir not found: {}",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(Error::config(format!(
            "parakeet model path is not a directory: {}",
            dir.display()
        )));
    }

    let mut missing = Vec::new();
    for name in REQUIRED_ALWAYS {
        if !dir.join(name).is_file() {
            missing.push(*name);
        }
    }
    let (enc, dec) = quantized_pair(quantization);
    // Accept either exact quantized name or the alternate (transcribe-rs may
    // resolve variants itself); require at least one encoder + one decoder.
    let enc_ok = dir.join(enc).is_file()
        || dir.join("encoder-model.onnx").is_file()
        || dir.join("encoder-model.int8.onnx").is_file();
    let dec_ok = dir.join(dec).is_file()
        || dir.join("decoder_joint-model.onnx").is_file()
        || dir.join("decoder_joint-model.int8.onnx").is_file();
    if !enc_ok {
        missing.push(enc);
    }
    if !dec_ok {
        missing.push(dec);
    }

    if !missing.is_empty() {
        return Err(Error::config(format!(
            "parakeet model dir incomplete ({}): missing {}",
            dir.display(),
            missing.join(", ")
        )));
    }
    Ok(())
}

impl SpeechToText for ParakeetStt {
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        let dir = &self.model_dir;
        preflight_model_dir(dir, &self.quantization)?;

        tracing::info!(
            path = %dir.display(),
            quant = format_quantization(&self.quantization),
            lang = %self.language,
            "loading Parakeet STT"
        );
        let t0 = Instant::now();
        let model = ParakeetModel::load(dir, &self.quantization).map_err(|e| {
            tracing::error!(
                error = %e,
                path = %dir.display(),
                "ParakeetModel::load failed"
            );
            Error::other(format!("parakeet load failed: {e} (dir={})", dir.display()))
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

    fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    fn backend_id(&self) -> &str {
        BACKEND_ID
    }

    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
        if audio.is_empty() {
            tracing::debug!("parakeet transcribe: empty audio → \"\"");
            return Ok(String::new());
        }

        if self.model.is_none() {
            self.load()?;
        }

        let model = self
            .model
            .as_mut()
            .ok_or_else(|| Error::other("STT model not loaded"))?;

        let samples = audio.len();
        let t0 = Instant::now();
        tracing::debug!(
            path = %self.model_dir.display(),
            samples,
            lang = %self.language,
            "parakeet transcribe start"
        );

        let result = model
            .transcribe(
                audio,
                &TranscribeOptions {
                    language: Some(self.language.clone()),
                    ..Default::default()
                },
            )
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    path = %self.model_dir.display(),
                    samples,
                    "parakeet transcribe failed"
                );
                Error::other(format!(
                    "parakeet transcribe failed: {e} (dir={}, samples={samples})",
                    self.model_dir.display()
                ))
            })?;

        let text = result.text.trim().to_string();
        tracing::info!(
            path = %self.model_dir.display(),
            samples,
            chars = text.chars().count(),
            ms = t0.elapsed().as_millis() as u64,
            "parakeet transcribe done"
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_inference::SpeechToText;

    #[test]
    fn stores_model_dir_and_defaults() {
        let stt = ParakeetStt::with_model_dir("/tmp/parakeet-test");
        assert_eq!(stt.model_dir(), Path::new("/tmp/parakeet-test"));
        assert_eq!(stt.language(), "en");
        assert!(!stt.is_loaded());
        assert_eq!(SpeechToText::backend_id(&stt), "parakeet");
        assert_eq!(SpeechToText::is_loaded(&stt), false);
    }

    #[test]
    fn with_language_override() {
        let stt = ParakeetStt::with_model_dir("x").with_language("es");
        assert_eq!(stt.language(), "es");
    }

    #[test]
    fn missing_dir_is_config_error() {
        let mut stt = ParakeetStt::with_model_dir(
            std::env::temp_dir().join("boris-parakeet-definitely-missing-xyz"),
        );
        let err = stt.load().unwrap_err();
        assert!(
            matches!(err, Error::Config(_)),
            "expected Config, got {err:?}"
        );
        assert!(!stt.is_loaded());
    }

    #[test]
    fn empty_audio_ok_without_load() {
        let mut stt = ParakeetStt::with_model_dir("/no/such/parakeet");
        let text = stt.transcribe(&[]).unwrap();
        assert_eq!(text, "");
        assert!(!stt.is_loaded());
    }

    #[test]
    fn debug_does_not_panic() {
        let stt = ParakeetStt::with_model_dir("dir");
        let s = format!("{stt:?}");
        assert!(s.contains("ParakeetStt"));
        assert!(s.contains("dir"));
    }

    #[test]
    fn incomplete_dir_preflight() {
        let tmp = std::env::temp_dir().join(format!(
            "boris-parakeet-incomplete-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Only vocab — missing onnx files.
        std::fs::write(tmp.join("vocab.txt"), "a 0\n").unwrap();

        let mut stt = ParakeetStt::with_model_dir(&tmp);
        let err = stt.load().unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("incomplete") || msg.contains("missing"), "{msg}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unload_when_not_loaded() {
        let mut stt = ParakeetStt::with_model_dir("x");
        stt.unload().unwrap();
        assert!(!stt.is_loaded());
    }
}
