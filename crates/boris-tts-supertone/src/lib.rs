//! Supertone (Supertonic 3) text-to-speech adapter.
//!
//! Product path: [`SupertoneTts::with_paths`] against `~/.boris/models/supertone/...`.
//! Long replies are split into [`text_units::speakable_units`] before synthesis.

mod text_units;

use std::path::{Path, PathBuf};
use std::time::Instant;

use boris_core::{AudioBuffer, Error, Result};
use boris_inference::TextToSpeech;
use st_tts::{SynthesisParams, Tts};

pub use text_units::{speakable_units, PREFERRED_UNIT_CHARS};

/// Supertonic 3 outputs 44.1 kHz mono float PCM.
pub const SUPERTONE_SAMPLE_RATE: u32 = 44_100;

/// Supertonic model label for logs.
pub const SUPERTONE_MODEL_ID: &str = "Supertone 3";

/// Tokio worker threads dedicated to this adapter's runtime.
const RUNTIME_WORKER_THREADS: usize = 2;

/// Supertone TTS backend (lazy-loaded; multi-thread runtime for async synth).
pub struct SupertoneTts {
    runtime: tokio::runtime::Runtime,
    model: Option<Tts>,
    model_dir: PathBuf,
    voice_dir: PathBuf,
    voice: String,
    lang: String,
    params: SynthesisParams,
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
    pub fn with_paths(
        model_dir: impl Into<PathBuf>,
        voice_dir: impl Into<PathBuf>,
        voice: &str,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(RUNTIME_WORKER_THREADS)
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for Supertone TTS");

        Self {
            runtime,
            model: None,
            model_dir: model_dir.into(),
            voice_dir: voice_dir.into(),
            voice: voice.to_string(),
            lang: "en".into(),
            params: default_synthesis_params(),
        }
    }

    /// BCP-47-ish language tag passed to the model (default `en`).
    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = lang.into();
        self
    }

    /// Diffusion / step count (quality vs speed).
    pub fn with_total_step(mut self, steps: usize) -> Self {
        self.params.total_step = steps;
        self
    }

    /// Speaking rate multiplier.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.params.speed = speed;
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

    /// Whether weights are loaded.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }
}

fn default_synthesis_params() -> SynthesisParams {
    // Match official supertonic-py / st-tts defaults.
    SynthesisParams {
        total_step: 8,
        speed: 1.05,
        silence_duration: 0.3,
        rng_seed: None,
    }
}

/// Reject Supertonic 1 (`opensource-en`) graphs that break under st-tts lang tags.
fn reject_english_only_supertone(model_dir: &Path) -> Option<String> {
    let path = model_dir.join("tts.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    if raw.contains("opensource-en") {
        return Some(format!(
            "Supertone install is English-only Supertonic 1 (opensource-en) at {}. \
             st-tts wraps every line as <en>…</en>, which this model cannot read, \
             so speech collapses to nonsense like \"an an an an\". \
             Install Supertonic 3 from Hugging Face Supertone/supertonic-3 \
             (use Install models in the app).",
            model_dir.display()
        ));
    }
    if !raw.contains("opensource-multilingual")
        && !raw.contains("\"tts_version\": \"v1.6")
        && !raw.contains("\"tts_version\": \"v1.7")
        && !raw.contains("\"tts_version\":\"v1.6")
        && !raw.contains("\"tts_version\":\"v1.7")
    {
        tracing::warn!(
            path = %path.display(),
            "supertone tts.json does not look like multilingual Supertonic 2/3"
        );
    }
    None
}

impl Default for SupertoneTts {
    fn default() -> Self {
        #[allow(deprecated)]
        Self::new()
    }
}

impl TextToSpeech for SupertoneTts {
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        let model_dir = &self.model_dir;
        let voice_path = self.voice_dir.join(format!("{}.json", self.voice));

        if !model_dir.is_dir() {
            return Err(Error::other(format!(
                "supertone model dir not found: {}",
                model_dir.display()
            )));
        }
        if !voice_path.is_file() {
            return Err(Error::other(format!(
                "supertone voice not found: {}",
                voice_path.display()
            )));
        }
        // st-tts always wraps text as `<lang>…</lang>`. Supertonic 1 English-only
        // indexers map those tags to unknown tokens → "an an an an" garbage speech.
        if let Some(problem) = reject_english_only_supertone(model_dir) {
            return Err(Error::other(problem));
        }

        tracing::info!(
            model = SUPERTONE_MODEL_ID,
            voice = %self.voice,
            path = %model_dir.display(),
            "loading Supertone TTS"
        );
        let t = Instant::now();

        let model = Tts::from_local(model_dir, &voice_path)
            .map_err(|e| Error::other(format!("Supertone load failed: {e}")))?;

        tracing::info!(
            sample_rate = model.sample_rate(),
            "Supertone TTS loaded in {}ms",
            t.elapsed().as_millis()
        );
        self.model = Some(model);
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }

    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
        if self.model.is_none() {
            self.load()?;
        }

        let model = self
            .model
            .as_ref()
            .ok_or_else(|| Error::other("TTS model not loaded"))?;

        let start = Instant::now();
        let lang = self.lang.clone();
        let params = self.params.clone();
        let units = speakable_units(text);

        if units.is_empty() {
            return Ok(Vec::new());
        }

        // One spoken unit per forward pass — see `text_units` module docs.
        let sample_rate = model.sample_rate().max(1);
        let gap = (params.silence_duration * sample_rate as f32).round() as usize;
        let mut full: AudioBuffer = Vec::new();
        let mut total_duration = 0.0f32;

        for (i, unit) in units.iter().enumerate() {
            let result = self
                .runtime
                .block_on(async { model.synthesize(unit, &lang, Some(&params)).await })
                .map_err(|e| {
                    Error::other(format!(
                        "Supertone synthesis failed on unit {}/{} ({:?}): {e}",
                        i + 1,
                        units.len(),
                        unit
                    ))
                })?;

            if i > 0 && gap > 0 {
                full.extend(std::iter::repeat(0.0f32).take(gap));
                total_duration += params.silence_duration;
            }

            let pcm = prefer_full_pcm(&result.audio, result.duration_secs, result.sample_rate);
            full.extend_from_slice(pcm);
            total_duration += result.duration_secs.max(pcm.len() as f32 / sample_rate as f32);

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
            "Supertone synthesis took {}ms",
            start.elapsed().as_millis()
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

    #[test]
    fn prefer_full_pcm_keeps_short_tail() {
        let audio = vec![0.1f32; 1000];
        let kept = prefer_full_pcm(&audio, 1000.0 / 44_100.0 - 0.001, 44_100);
        assert_eq!(kept.len(), 1000);
    }
}
