//! Supertone (Supertonic 3) text-to-speech adapter.
//!
//! # Audio
//!
//! Native output is **44.1 kHz mono f32** ([`SUPERTONE_SAMPLE_RATE`]). Hosts
//! resample for the playback device when needed.
//!
//! # Product path
//!
//! [`SupertoneTts::with_paths`] against `~/.boris/models/supertone/...`.
//! Long replies are split into [`text_units::speakable_units`] before synthesis.
//!
//! # Silence
//!
//! Inter-unit silence is owned by this crate ([`SupertoneTts::with_silence_duration`]).
//! st-tts also supports `silence_duration` between *its* internal text chunks;
//! we zero that out when calling the model so gaps are not double-applied.
//!
//! # Threading
//!
//! Synthesis uses a private multi-thread Tokio runtime + `block_on` on a
//! **sync host thread** (the pipeline engine thread). Do not call
//! [`TextToSpeech::synthesize`] from inside an entered Tokio runtime.

mod text_units;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use boris_core::{AudioBuffer, Error, Result};
use boris_inference::TextToSpeech;
use st_tts::{SynthesisParams, Tts};

pub use text_units::{speakable_units, PREFERRED_UNIT_CHARS};

/// Backend id returned by [`TextToSpeech::backend_id`].
pub const BACKEND_ID: &str = "supertone";

/// Supertonic 3 outputs 44.1 kHz mono float PCM.
pub const SUPERTONE_SAMPLE_RATE: u32 = 44_100;

/// Supertonic model label for logs.
pub const SUPERTONE_MODEL_ID: &str = "Supertone 3";

/// Tokio worker threads dedicated to this adapter's runtime.
const RUNTIME_WORKER_THREADS: usize = 2;

/// Default inter-unit silence (seconds) inserted between speakable units.
const DEFAULT_INTER_UNIT_SILENCE: f32 = 0.15;

/// Supertone TTS backend (lazy-loaded; private runtime for async synth).
///
/// Construct with [`SupertoneTts::with_paths`].
pub struct SupertoneTts {
    /// Built lazily so construction never panics.
    runtime: Option<tokio::runtime::Runtime>,
    model: Option<Tts>,
    model_dir: PathBuf,
    voice_dir: PathBuf,
    voice: String,
    lang: String,
    /// Params passed to st-tts (`silence_duration` forced to 0 at synth time).
    params: SynthesisParams,
    /// Silence we insert between our speakable units (not st-tts internal).
    inter_unit_silence: f32,
}

impl SupertoneTts {
    /// Relative `assets/` paths (legacy). Prefer [`Self::with_paths`].
    #[deprecated(note = "use SupertoneTts::with_paths(~/.boris/models/...)")]
    pub fn new() -> Self {
        Self::with_paths(
            PathBuf::from("assets/models/supertone/onnx"),
            PathBuf::from("assets/models/supertone/voices"),
            "M4",
        )
    }

    /// Legacy voice override with default asset paths.
    #[deprecated(note = "use SupertoneTts::with_paths(..., voice)")]
    pub fn with_voice(voice: &str) -> Self {
        Self::with_paths(
            PathBuf::from("assets/models/supertone/onnx"),
            PathBuf::from("assets/models/supertone/voices"),
            voice,
        )
    }

    /// Explicit onnx + voices directories (desktop / `~/.boris`).
    ///
    /// Does **not** build a Tokio runtime yet (no panic on construction).
    /// Runtime is created on first `load` / `synthesize`.
    ///
    /// # Panics
    ///
    /// Does not panic. Invalid voice ids surface as [`Error::Config`] on load.
    pub fn with_paths(
        model_dir: impl Into<PathBuf>,
        voice_dir: impl Into<PathBuf>,
        voice: &str,
    ) -> Self {
        Self {
            runtime: None,
            model: None,
            model_dir: model_dir.into(),
            voice_dir: voice_dir.into(),
            voice: voice.to_string(),
            lang: "en".into(),
            params: default_synthesis_params(),
            inter_unit_silence: DEFAULT_INTER_UNIT_SILENCE,
        }
    }

    /// BCP-47-ish language tag passed to the model (default `en`).
    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = lang.into();
        self
    }

    /// Diffusion / step count (quality vs speed). Values below 1 are clamped to 1.
    pub fn with_total_step(mut self, steps: usize) -> Self {
        self.params.total_step = steps.max(1);
        self
    }

    /// Speaking rate multiplier. Non-finite or non-positive values fall back to 1.0.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.params.speed = if speed.is_finite() && speed > 0.0 {
            speed
        } else {
            1.0
        };
        self
    }

    /// Silence (seconds) inserted **between speakable units** by this adapter.
    ///
    /// Not the same as st-tts internal chunk silence (which we zero out).
    /// Negative values are clamped to 0.
    pub fn with_silence_duration(mut self, secs: f32) -> Self {
        self.inter_unit_silence = if secs.is_finite() && secs > 0.0 {
            secs
        } else {
            0.0
        };
        self
    }

    /// Native sample rate (loaded model, or [`SUPERTONE_SAMPLE_RATE`] before load).
    pub fn sample_rate(&self) -> u32 {
        self.model
            .as_ref()
            .map(|m| m.sample_rate())
            .unwrap_or(SUPERTONE_SAMPLE_RATE)
    }

    /// ONNX model directory.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Voice JSON directory.
    pub fn voice_dir(&self) -> &Path {
        &self.voice_dir
    }

    /// Configured voice id.
    pub fn voice(&self) -> &str {
        &self.voice
    }

    /// Configured language tag.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Inter-unit silence seconds.
    pub fn silence_duration(&self) -> f32 {
        self.inter_unit_silence
    }

    /// Whether weights are loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    fn ensure_runtime(&mut self) -> Result<()> {
        if self.runtime.is_some() {
            return Ok(());
        }
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(RUNTIME_WORKER_THREADS)
            .enable_all()
            .thread_name("boris-supertone")
            .build()
            .map_err(|e| Error::other(format!("supertone tokio runtime build failed: {e}")))?;
        self.runtime = Some(rt);
        Ok(())
    }
}

impl Default for SupertoneTts {
    #[allow(deprecated)]
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SupertoneTts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SupertoneTts")
            .field("model_dir", &self.model_dir)
            .field("voice_dir", &self.voice_dir)
            .field("voice", &self.voice)
            .field("lang", &self.lang)
            .field("loaded", &self.model.is_some())
            .field("inter_unit_silence", &self.inter_unit_silence)
            .field("total_step", &self.params.total_step)
            .field("speed", &self.params.speed)
            .finish()
    }
}

fn default_synthesis_params() -> SynthesisParams {
    // Match official supertonic-py / st-tts defaults for quality knobs.
    // silence_duration is forced to 0 at synthesize time (we own inter-unit gaps).
    SynthesisParams {
        total_step: 8,
        speed: 1.05,
        silence_duration: 0.0,
        rng_seed: None,
    }
}

/// Validate voice id is a single path segment (no traversal).
fn validate_voice_id(voice: &str) -> Result<()> {
    let v = voice.trim();
    if v.is_empty() {
        return Err(Error::config("supertone voice id is empty"));
    }
    if v.contains("..")
        || v.contains('/')
        || v.contains('\\')
        || v.contains('\0')
        || Path::new(v).components().count() != 1
    {
        return Err(Error::config(format!(
            "supertone voice id must be a simple basename (no path separators): {voice:?}"
        )));
    }
    Ok(())
}

/// Reject Supertonic 1 (`opensource-en`) graphs that break under st-tts lang tags.
///
/// Parses `tts.json` when possible; falls back to substring checks.
fn reject_english_only_supertone(model_dir: &Path) -> Result<()> {
    let path = model_dir.join("tts.json");
    if !path.is_file() {
        // README documents tts.json as a hard requirement (ONNX graph +
        // tts.json) and English-only installs as always rejected; a missing
        // file means we cannot verify the model family, so fail closed.
        return Err(Error::config(format!(
            "supertone model dir missing required tts.json: {}",
            path.display()
        )));
    }

    let raw = std::fs::read_to_string(&path)
        .map_err(|e| Error::config(format!("supertone failed to read {}: {e}", path.display())))?;

    if let Some(msg) = english_only_problem_from_tts_json(&raw, model_dir) {
        return Err(Error::config(msg));
    }
    Ok(())
}

/// Shared logic for tests + load path.
fn english_only_problem_from_tts_json(raw: &str, model_dir: &Path) -> Option<String> {
    // Prefer structured parse.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        let split = v.get("split").and_then(|s| s.as_str()).unwrap_or("");
        let version = v.get("tts_version").and_then(|s| s.as_str()).unwrap_or("");

        if split.contains("opensource-en") || split == "en" {
            return Some(format!(
                "Supertone install is English-only Supertonic 1 (split={split:?}, tts_version={version:?}) at {}. \
                 st-tts wraps every line as <en>…</en>, which this model cannot read, \
                 so speech collapses to nonsense like \"an an an an\". \
                 Install Supertonic 3 from Hugging Face Supertone/supertonic-3 \
                 (use Install models in the app).",
                model_dir.display()
            ));
        }

        let looks_multi = split.contains("opensource-multilingual")
            || version.starts_with("v1.6")
            || version.starts_with("v1.7")
            || version.starts_with("1.6")
            || version.starts_with("1.7");
        if !looks_multi && !split.is_empty() {
            tracing::warn!(
                path = %model_dir.join("tts.json").display(),
                split = %split,
                tts_version = %version,
                "supertone tts.json does not look like multilingual Supertonic 2/3"
            );
        }
        return None;
    }

    // Fallback: raw substring (malformed JSON).
    if raw.contains("opensource-en") {
        return Some(format!(
            "Supertone install is English-only Supertonic 1 (opensource-en) at {}. \
             Install Supertonic 3 from Hugging Face Supertone/supertonic-3.",
            model_dir.display()
        ));
    }
    None
}

fn preflight_paths(model_dir: &Path, voice_dir: &Path, voice: &str) -> Result<PathBuf> {
    validate_voice_id(voice)?;

    if !model_dir.exists() {
        return Err(Error::config(format!(
            "supertone model dir not found: {}",
            model_dir.display()
        )));
    }
    if !model_dir.is_dir() {
        return Err(Error::config(format!(
            "supertone model path is not a directory: {}",
            model_dir.display()
        )));
    }
    if !voice_dir.is_dir() {
        return Err(Error::config(format!(
            "supertone voice dir not found: {}",
            voice_dir.display()
        )));
    }

    let voice_path = voice_dir.join(format!("{voice}.json"));
    if !voice_path.is_file() {
        return Err(Error::config(format!(
            "supertone voice not found: {}",
            voice_path.display()
        )));
    }

    // Soft check for expected onnx pieces.
    let expected = [
        "text_encoder.onnx",
        "vector_estimator.onnx",
        "vocoder.onnx",
        "duration_predictor.onnx",
    ];
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|n| !model_dir.join(n).is_file())
        .collect();
    if !missing.is_empty() {
        tracing::warn!(
            path = %model_dir.display(),
            ?missing,
            "supertone model dir may be incomplete"
        );
    }

    reject_english_only_supertone(model_dir)?;
    Ok(voice_path)
}

impl TextToSpeech for SupertoneTts {
    fn is_loaded(&self) -> bool {
        SupertoneTts::is_loaded(self)
    }

    fn backend_id(&self) -> &str {
        BACKEND_ID
    }

    fn sample_rate(&self) -> u32 {
        SupertoneTts::sample_rate(self)
    }

    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        // Validate paths before paying for the (real, multi-thread) Tokio
        // runtime build — a bad path shouldn't need a runtime first.
        let model_dir = self.model_dir.clone();
        let voice_dir = self.voice_dir.clone();
        let voice = self.voice.clone();
        let voice_path = preflight_paths(&model_dir, &voice_dir, &voice)?;

        self.ensure_runtime()?;

        tracing::info!(
            model = SUPERTONE_MODEL_ID,
            voice = %voice,
            path = %model_dir.display(),
            "loading Supertone TTS"
        );
        let t = Instant::now();

        let model = Tts::from_local(&model_dir, &voice_path).map_err(|e| {
            Error::other(format!(
                "Supertone load failed: {e} (model_dir={}, voice={})",
                model_dir.display(),
                voice_path.display()
            ))
        })?;

        tracing::info!(
            sample_rate = model.sample_rate(),
            ms = t.elapsed().as_millis() as u64,
            "Supertone TTS loaded"
        );
        self.model = Some(model);
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        // Keep runtime warm — building it is cheap relative to ONNX, but
        // retaining it avoids churn across idle→active cycles.
        Ok(())
    }

    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
        // Nested block_on inside an entered runtime panics — fail clearly,
        // and *before* touching the model, so misuse is rejected without
        // paying for a (possibly slow) lazy load first.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::other(
                "supertone synthesize cannot run inside a Tokio runtime \
                 (nested block_on). Call from a sync host thread only.",
            ));
        }

        if text.trim().is_empty() {
            return Ok(Vec::new());
        }

        if self.model.is_none() {
            self.load()?;
        }

        self.ensure_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| Error::other("supertone runtime missing"))?;

        let model = self
            .model
            .as_ref()
            .ok_or_else(|| Error::other("TTS model not loaded"))?;

        let start = Instant::now();
        let lang = self.lang.as_str();
        // Own inter-unit silence; st-tts internal chunk silence stays 0
        // (default_synthesis_params() sets it and nothing mutates it after).
        let params = self.params.clone();
        let inter_unit = self.inter_unit_silence;
        let units = speakable_units(text);

        if units.is_empty() {
            return Ok(Vec::new());
        }

        let sample_rate = model.sample_rate().max(1);
        let gap = (inter_unit * sample_rate as f32).round() as usize;
        let mut full: AudioBuffer = Vec::new();
        let mut total_duration = 0.0f32;

        for (i, unit) in units.iter().enumerate() {
            let result = runtime
                .block_on(async { model.synthesize(unit, lang, Some(&params)).await })
                .map_err(|e| {
                    Error::other(format!(
                        "Supertone synthesis failed on unit {}/{} ({:?}) path={}: {e}",
                        i + 1,
                        units.len(),
                        unit,
                        self.model_dir.display()
                    ))
                })?;

            if i > 0 && gap > 0 {
                full.extend(std::iter::repeat_n(0.0f32, gap));
                total_duration += inter_unit;
            }

            let pcm = prefer_full_pcm(&result.audio, result.duration_secs, result.sample_rate);
            full.extend_from_slice(pcm);
            total_duration += result
                .duration_secs
                .max(pcm.len() as f32 / sample_rate as f32);

            tracing::debug!(
                unit = i + 1,
                of = units.len(),
                chars = unit.chars().count(),
                samples = pcm.len(),
                text = %unit,
                "tts unit synthesized"
            );
        }

        tracing::info!(
            units = units.len(),
            samples = full.len(),
            duration_secs = total_duration,
            sample_rate,
            ms = start.elapsed().as_millis() as u64,
            "Supertone synthesis done"
        );

        Ok(full)
    }
}

/// Keep as much real PCM as the model produced.
///
/// st-tts slices with `sample_rate * duration_secs`, which can round down and
/// clip the last phonemes. If the buffer is only slightly longer than the
/// predicted length, keep the full buffer.
fn prefer_full_pcm(audio: &[f32], duration_secs: f32, sample_rate: u32) -> &[f32] {
    if audio.is_empty() || sample_rate == 0 {
        return audio;
    }
    let predicted = (sample_rate as f32 * duration_secs).round() as usize;
    if predicted == 0 || predicted >= audio.len() {
        return audio;
    }
    // If prediction is within ~80ms of the buffer, trust the buffer (tail).
    let slack = (sample_rate as usize) / 12; // ~83ms
    if audio.len() - predicted <= slack {
        audio
    } else {
        // Large overshoot is usually padding — keep predicted length + slack.
        let end = (predicted + slack).min(audio.len());
        &audio[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_inference::TextToSpeech;

    #[test]
    fn stores_paths_and_defaults() {
        let tts = SupertoneTts::with_paths("/m", "/v", "M4");
        assert_eq!(tts.model_dir(), Path::new("/m"));
        assert_eq!(tts.voice_dir(), Path::new("/v"));
        assert_eq!(tts.voice(), "M4");
        assert_eq!(tts.lang(), "en");
        assert!(!tts.is_loaded());
        assert_eq!(TextToSpeech::backend_id(&tts), "supertone");
        assert_eq!(TextToSpeech::sample_rate(&tts), SUPERTONE_SAMPLE_RATE);
        assert!((tts.silence_duration() - DEFAULT_INTER_UNIT_SILENCE).abs() < f32::EPSILON);
    }

    #[test]
    fn builder_clamps_speed_and_steps() {
        let tts = SupertoneTts::with_paths("m", "v", "M4")
            .with_speed(-1.0)
            .with_total_step(0)
            .with_silence_duration(-0.5);
        assert!((tts.params.speed - 1.0).abs() < f32::EPSILON);
        assert_eq!(tts.params.total_step, 1);
        assert_eq!(tts.silence_duration(), 0.0);
    }

    #[test]
    fn empty_text_without_load() {
        let mut tts = SupertoneTts::with_paths("/no/such/onnx", "/no/voices", "M4");
        assert!(tts.synthesize("").unwrap().is_empty());
        assert!(tts.synthesize("   ").unwrap().is_empty());
        assert!(!tts.is_loaded());
    }

    #[test]
    fn missing_model_dir_is_config() {
        let mut tts = SupertoneTts::with_paths(
            std::env::temp_dir().join("boris-supertone-missing-xyz"),
            std::env::temp_dir().join("boris-supertone-voices-xyz"),
            "M4",
        );
        let err = tts.load().unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
    }

    /// Guard must reject nested `synthesize()` calls made from inside an
    /// already-entered Tokio runtime *before* touching the model — so this
    /// works even with fake, nonexistent paths (README safety claim).
    #[tokio::test]
    async fn synthesize_inside_tokio_runtime_is_rejected() {
        let mut tts = SupertoneTts::with_paths(
            std::env::temp_dir().join("boris-supertone-nested-rt-xyz"),
            std::env::temp_dir().join("boris-supertone-nested-rt-voices-xyz"),
            "M4",
        );
        let err = tts.synthesize("hello world").unwrap_err();
        assert!(matches!(err, Error::Other(_)), "got {err:?}");
        // Rejected before load() ran — model must still be unloaded.
        assert!(!tts.is_loaded());
    }

    #[test]
    fn missing_tts_json_is_config_error() {
        let tmp =
            std::env::temp_dir().join(format!("boris-supertone-notts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let model_dir = tmp.join("onnx");
        let voice_dir = tmp.join("voices");
        std::fs::create_dir_all(&model_dir).unwrap();
        std::fs::create_dir_all(&voice_dir).unwrap();
        std::fs::write(voice_dir.join("M4.json"), "{}").unwrap();
        // Intentionally no tts.json in model_dir.

        let err = preflight_paths(&model_dir, &voice_dir, "M4").unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("tts.json"), "{msg}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn voice_path_traversal_rejected() {
        assert!(validate_voice_id("../etc/passwd").is_err());
        assert!(validate_voice_id("a/b").is_err());
        assert!(validate_voice_id("a\\b").is_err());
        assert!(validate_voice_id("").is_err());
        assert!(validate_voice_id("M4").is_ok());
    }

    #[test]
    fn english_only_json_detect() {
        let raw = r#"{
            "tts_version": "v1.5.0",
            "split": "opensource-en"
        }"#;
        let msg = english_only_problem_from_tts_json(raw, Path::new("/models")).unwrap();
        assert!(msg.contains("English-only") || msg.contains("opensource-en"));
    }

    #[test]
    fn multilingual_json_ok() {
        let raw = r#"{
            "tts_version": "v1.7.0",
            "split": "opensource-multilingual"
        }"#;
        assert!(english_only_problem_from_tts_json(raw, Path::new("/m")).is_none());
    }

    #[test]
    fn english_only_raw_fallback() {
        let raw = "not json but opensource-en appears";
        assert!(english_only_problem_from_tts_json(raw, Path::new("/m")).is_some());
    }

    #[test]
    fn prefer_full_pcm_keeps_short_tail() {
        let audio = vec![0.1f32; 1000];
        let kept = prefer_full_pcm(&audio, 1000.0 / 44_100.0 - 0.001, 44_100);
        assert_eq!(kept.len(), 1000);
    }

    #[test]
    fn prefer_full_pcm_empty_and_zero_rate() {
        assert!(prefer_full_pcm(&[], 1.0, 44_100).is_empty());
        let audio = [0.1f32; 10];
        assert_eq!(prefer_full_pcm(&audio, 1.0, 0).len(), 10);
    }

    #[test]
    fn prefer_full_pcm_trims_large_overshoot() {
        // predicted much smaller than buffer → trim to predicted + slack
        let audio = vec![0.2f32; 20_000];
        let duration = 0.1; // ~4410 samples at 44.1k
        let kept = prefer_full_pcm(&audio, duration, 44_100);
        let predicted = (44_100.0f32 * duration).round() as usize;
        let slack = 44_100 / 12;
        assert_eq!(kept.len(), (predicted + slack).min(audio.len()));
    }

    #[test]
    fn debug_ok() {
        let tts = SupertoneTts::with_paths("m", "v", "M4");
        assert!(format!("{tts:?}").contains("SupertoneTts"));
    }
}
