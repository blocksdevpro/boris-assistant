//! Kokoro TTS adapter ([`TextToSpeech`](boris_inference::TextToSpeech)).
//!
//! # Status
//!
//! **Experimental.** Local Candle/Kokoro path via `any-tts` for A/B testing
//! against Supertone. Product default remains Supertone.
//!
//! # Model layout
//!
//! Directory passed to [`KokoroTts::with_model_path`] should contain:
//! - `config.json`
//! - weights: `kokoro-v1_0.pth` or `model.safetensors` (or similar)
//! - `voices/<voice>.pt` for the configured voice
//!
//! Automatic HuggingFace download is **disabled** (`any-tts` built without the
//! `download` feature). Missing files return [`boris_core::Error::Config`].
//!
//! # Output
//!
//! Mono `f32` PCM at [`KOKORO_SAMPLE_RATE`] (24 kHz).
//!
//! # Load policy
//!
//! Prefer explicit [`TextToSpeech::load`]. [`TextToSpeech::synthesize`] also
//! lazy-loads when unloaded. Empty text returns an empty buffer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use any_tts::{load_model, ModelType, SynthesisRequest, TtsConfig, TtsModel};
use boris_core::{AudioBuffer, Error, Result};
use boris_inference::TextToSpeech;

/// Backend id returned by [`TextToSpeech::backend_id`].
pub const BACKEND_ID: &str = "kokoro";

/// Kokoro native sample rate (mono f32).
pub const KOKORO_SAMPLE_RATE: u32 = 24_000;

/// Legacy CWD-relative default path (deprecated constructor only).
#[deprecated(note = "use KokoroTts::with_model_path instead of CWD defaults")]
pub const KOKORO_MODEL_PATH: &str = "./assets/models/kokoro";

const DEFAULT_VOICE: &str = "bm_lewis";
const DEFAULT_LANGUAGE: &str = "English";

/// Kokoro TTS backend (lazy-loaded Candle weights).
pub struct KokoroTts {
    model: Option<Box<dyn TtsModel>>,
    model_path: PathBuf,
    voice: String,
    language: String,
}

impl KokoroTts {
    /// Explicit model directory (preferred product constructor).
    pub fn with_model_path(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model: None,
            model_path: model_path.into(),
            voice: DEFAULT_VOICE.to_string(),
            language: DEFAULT_LANGUAGE.to_string(),
        }
    }

    /// Legacy CWD path `./assets/models/kokoro`.
    ///
    /// Prefer [`Self::with_model_path`].
    #[deprecated(note = "use KokoroTts::with_model_path(~/.boris/models/kokoro or assets path)")]
    pub fn new() -> Self {
        #[allow(deprecated)]
        Self::with_model_path(PathBuf::from(KOKORO_MODEL_PATH))
    }

    /// Voice id / style name passed to any-tts (default `bm_lewis`).
    pub fn with_voice(mut self, voice: impl Into<String>) -> Self {
        self.voice = voice.into();
        self
    }

    /// Language label for synthesis (default `English`).
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Configured model directory.
    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Configured voice id.
    pub fn voice(&self) -> &str {
        &self.voice
    }

    /// Configured language label.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Whether weights are currently loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    /// Native sample rate (always [`KOKORO_SAMPLE_RATE`]).
    pub fn sample_rate(&self) -> u32 {
        KOKORO_SAMPLE_RATE
    }
}

impl Default for KokoroTts {
    #[allow(deprecated)]
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for KokoroTts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KokoroTts")
            .field("model_path", &self.model_path)
            .field("voice", &self.voice)
            .field("language", &self.language)
            .field("loaded", &self.model.is_some())
            .field("sample_rate", &KOKORO_SAMPLE_RATE)
            .finish()
    }
}

/// Preflight local files so we never fall through to a surprise network path.
fn preflight_model_path(dir: &Path, voice: &str) -> Result<()> {
    if !dir.exists() {
        return Err(Error::config(format!(
            "kokoro model path not found: {}",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(Error::config(format!(
            "kokoro model path is not a directory: {}",
            dir.display()
        )));
    }

    let config = dir.join("config.json");
    if !config.is_file() {
        return Err(Error::config(format!(
            "kokoro model incomplete ({}): missing config.json",
            dir.display()
        )));
    }

    let has_weights = dir.join("kokoro-v1_0.pth").is_file()
        || dir.join("model.safetensors").is_file()
        || dir.join("pytorch_model.bin").is_file()
        || dir
            .read_dir()
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.ends_with(".pth") || s.ends_with(".safetensors")
            });
    if !has_weights {
        return Err(Error::config(format!(
            "kokoro model incomplete ({}): missing weights (.pth / .safetensors)",
            dir.display()
        )));
    }

    // Product uses named voices: if a voice id is set, both voices/ and the
    // specific voices/<voice>.pt file are hard requirements (README:
    // "Missing / incomplete model path → Error::Config"), not a later
    // Error::Other surprise at synthesize() time.
    if !voice.is_empty() {
        let voices_dir = dir.join("voices");
        if !voices_dir.is_dir() {
            return Err(Error::config(format!(
                "kokoro model incomplete ({}): missing voices/ for voice '{voice}'",
                dir.display()
            )));
        }
        let voice_pt = voices_dir.join(format!("{voice}.pt"));
        if !voice_pt.is_file() {
            return Err(Error::config(format!(
                "kokoro model incomplete ({}): missing voice file voices/{voice}.pt",
                dir.display()
            )));
        }
    }

    Ok(())
}

impl TextToSpeech for KokoroTts {
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        let path = &self.model_path;
        preflight_model_path(path, &self.voice)?;

        let path_str = path.to_string_lossy().into_owned();
        tracing::info!(
            path = %path.display(),
            voice = %self.voice,
            language = %self.language,
            sample_rate = KOKORO_SAMPLE_RATE,
            "loading Kokoro TTS"
        );
        let t = Instant::now();

        let model = load_model(TtsConfig::new(ModelType::Kokoro).with_model_path(path_str.clone()))
            .map_err(|e| {
                tracing::error!(error = %e, path = %path.display(), "Kokoro load failed");
                Error::other(format!("kokoro load failed: {e} (path={})", path.display()))
            })?;
        self.model = Some(model);

        tracing::info!(
            path = %path.display(),
            ms = t.elapsed().as_millis() as u64,
            sample_rate = KOKORO_SAMPLE_RATE,
            "Kokoro TTS loaded"
        );
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }

    fn is_loaded(&self) -> bool {
        KokoroTts::is_loaded(self)
    }

    fn backend_id(&self) -> &str {
        BACKEND_ID
    }

    fn sample_rate(&self) -> u32 {
        KOKORO_SAMPLE_RATE
    }

    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        if self.model.is_none() {
            self.load()?;
        }

        let model = self
            .model
            .as_ref()
            .ok_or_else(|| Error::other("TTS model not loaded"))?;

        let start = Instant::now();
        let result = model
            .synthesize(
                &SynthesisRequest::new(text)
                    .with_language(&self.language)
                    .with_voice(&self.voice),
            )
            .map_err(|e| {
                Error::other(format!(
                    "kokoro synthesize failed: {e} (path={}, voice={})",
                    self.model_path.display(),
                    self.voice
                ))
            })?;

        // We always report the fixed KOKORO_SAMPLE_RATE constant (public
        // contract, unchanged) — but cross-check against what any-tts
        // actually produced so a future upstream Kokoro sample-rate change
        // doesn't silently desync the reported rate from the real audio.
        if result.sample_rate != KOKORO_SAMPLE_RATE {
            tracing::debug!(
                actual = result.sample_rate,
                expected = KOKORO_SAMPLE_RATE,
                "kokoro any-tts result.sample_rate differs from KOKORO_SAMPLE_RATE constant"
            );
            debug_assert_eq!(
                result.sample_rate, KOKORO_SAMPLE_RATE,
                "any-tts Kokoro sample rate drifted from the adapter's KOKORO_SAMPLE_RATE constant"
            );
        }

        tracing::info!(
            samples = result.samples.len(),
            sample_rate = KOKORO_SAMPLE_RATE,
            ms = start.elapsed().as_millis() as u64,
            "Kokoro synthesis done"
        );
        Ok(result.samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_inference::TextToSpeech;

    #[test]
    fn stores_path_voice_language() {
        let tts = KokoroTts::with_model_path("/tmp/kokoro")
            .with_voice("af_heart")
            .with_language("English");
        assert_eq!(tts.model_path(), Path::new("/tmp/kokoro"));
        assert_eq!(tts.voice(), "af_heart");
        assert_eq!(tts.language(), "English");
        assert!(!tts.is_loaded());
        assert_eq!(TextToSpeech::sample_rate(&tts), KOKORO_SAMPLE_RATE);
        assert_eq!(TextToSpeech::backend_id(&tts), "kokoro");
    }

    #[test]
    fn empty_text_without_load() {
        let mut tts = KokoroTts::with_model_path("/no/such/kokoro");
        assert!(tts.synthesize("").unwrap().is_empty());
        assert!(tts.synthesize("   ").unwrap().is_empty());
        assert!(!tts.is_loaded());
    }

    #[test]
    fn missing_path_is_config() {
        let mut tts = KokoroTts::with_model_path(
            std::env::temp_dir().join("boris-kokoro-definitely-missing-xyz"),
        );
        let err = tts.load().unwrap_err();
        assert!(
            matches!(err, Error::Config(_)),
            "expected Config, got {err:?}"
        );
        assert!(!tts.is_loaded());
    }

    #[test]
    fn incomplete_dir_preflight() {
        let tmp =
            std::env::temp_dir().join(format!("boris-kokoro-incomplete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("config.json"), "{}").unwrap();
        // no weights

        let mut tts = KokoroTts::with_model_path(&tmp);
        let err = tts.load().unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err:?}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn missing_voice_file_is_config_error() {
        let tmp = std::env::temp_dir().join(format!("boris-kokoro-novoice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("config.json"), "{}").unwrap();
        std::fs::write(tmp.join("model.safetensors"), "fake").unwrap();
        std::fs::create_dir_all(tmp.join("voices")).unwrap();
        // voices/ exists but voices/<voice>.pt does not.

        let mut tts = KokoroTts::with_model_path(&tmp).with_voice("nonexistent_voice");
        let err = tts.load().unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        assert!(!tts.is_loaded());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn debug_ok() {
        let tts = KokoroTts::with_model_path("p");
        assert!(format!("{tts:?}").contains("KokoroTts"));
    }

    #[test]
    fn unload_when_not_loaded() {
        let mut tts = KokoroTts::with_model_path("p");
        tts.unload().unwrap();
    }
}
