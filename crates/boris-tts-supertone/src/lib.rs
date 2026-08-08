use std::path::{Path, PathBuf};
use std::time::Instant;

use boris_core::{
    error::{Error, Result},
    AudioBuffer,
};
use boris_inference::TextToSpeech;
use st_tts::{SynthesisParams, Tts};

/// Supertonic 3 outputs 44.1 kHz mono float PCM.
pub const SUPERTONE_SAMPLE_RATE: u32 = 44_100;

/// Supertonic Model ID.
pub const SUPERTONE_MODEL_ID: &str = "Supertone 3";

/// Soft per-unit char budget. Supertonic is much more reliable on short
/// complete sentences than on multi-sentence / multi-clause monologues.
const PREFERRED_UNIT_CHARS: usize = 180;

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
    pub fn new() -> Self {
        Self::with_paths(
            PathBuf::from("assets/models/supertone/onnx"),
            PathBuf::from("assets/models/supertone/voices"),
            "M4",
        )
    }

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
            .worker_threads(2)
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
            params: SynthesisParams {
                // Match st-tts / Supertonic defaults more closely. Aggressive
                // speed + low step counts made multi-sentence replies skip
                // clauses or sound incomplete.
                total_step: 8,
                speed: 1.08,
                // Pause between auto-chunks / our sentence units.
                silence_duration: 0.22,
                rng_seed: None,
            },
        }
    }

    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = lang.into();
        self
    }

    pub fn with_total_step(mut self, steps: usize) -> Self {
        self.params.total_step = steps;
        self
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.params.speed = speed;
        self
    }

    pub fn sample_rate(&self) -> u32 {
        self.model
            .as_ref()
            .map(|m| m.sample_rate())
            .unwrap_or(SUPERTONE_SAMPLE_RATE)
    }

    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }
}

impl Default for SupertoneTts {
    fn default() -> Self {
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
            return Err(Error::Other(format!(
                "supertone model dir not found: {}",
                model_dir.display()
            )));
        }
        if !voice_path.is_file() {
            return Err(Error::Other(format!(
                "supertone voice not found: {}",
                voice_path.display()
            )));
        }

        tracing::info!(
            model = SUPERTONE_MODEL_ID,
            voice = %self.voice,
            path = %model_dir.display(),
            "loading Supertone TTS"
        );
        let t = Instant::now();

        let model = Tts::from_local(model_dir, &voice_path)
            .map_err(|e| Error::Other(format!("Supertone load failed: {e}")))?;

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
            .ok_or_else(|| Error::Other("TTS model not loaded".into()))?;

        let start = Instant::now();
        let lang = self.lang.clone();
        let params = self.params.clone();
        let units = speakable_units(text);

        if units.is_empty() {
            return Ok(Vec::new());
        }

        // Synthesize one complete spoken unit at a time. Supertonic (and most
        // small on-device TTS models) routinely drop middle clauses when given
        // multi-sentence / multi-clause monologues in a single forward pass.
        // st-tts only auto-chunks past ~300 chars — our replies are usually
        // under that, so we force sentence-level units ourselves.
        let sample_rate = model.sample_rate().max(1);
        let gap = (params.silence_duration * sample_rate as f32).round() as usize;
        let mut full: AudioBuffer = Vec::new();
        let mut total_duration = 0.0f32;

        for (i, unit) in units.iter().enumerate() {
            let result = self
                .runtime
                .block_on(async { model.synthesize(unit, &lang, Some(&params)).await })
                .map_err(|e| {
                    Error::Other(format!(
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

            // Prefer the real PCM length over predicted duration_secs — the
            // latter can under-slice and cut the tail of a sentence.
            let pcm = prefer_full_pcm(&result.audio, result.duration_secs, result.sample_rate);
            full.extend_from_slice(&pcm);
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

/// Split reply text into short, complete spoken units for reliable TTS.
///
/// 1. Split on sentence-ending punctuation (`.?!`) when followed by space/end.
/// 2. Further split very long units on commas / semicolons so a single
///    monologue clause cannot starve later words in the model.
fn speakable_units(text: &str) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        current.push(c);
        if matches!(c, '.' | '!' | '?') {
            let boundary = i + 1 >= chars.len() || chars[i + 1].is_whitespace();
            if boundary {
                let unit = current.trim().to_string();
                if !unit.is_empty() {
                    sentences.push(unit);
                }
                current.clear();
                while i + 1 < chars.len() && chars[i + 1].is_whitespace() {
                    i += 1;
                }
            }
        }
        i += 1;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }
    if sentences.is_empty() {
        sentences.push(text.to_string());
    }

    let mut units = Vec::new();
    for sentence in sentences {
        units.extend(split_long_unit(&sentence, PREFERRED_UNIT_CHARS));
    }
    units
}

fn split_long_unit(text: &str, max_chars: usize) -> Vec<String> {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return vec![text.to_string()];
    }

    // Prefer clause breaks.
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    let tokens: Vec<&str> = text
        .split_inclusive(|c: char| matches!(c, ',' | ';' | ':'))
        .collect();

    if tokens.len() <= 1 {
        // Fall back to word packing.
        return pack_words(text, max_chars);
    }

    for token in tokens {
        let token_chars = token.chars().count();
        if current_chars > 0 && current_chars + token_chars > max_chars {
            let unit = current.trim().to_string();
            if !unit.is_empty() {
                parts.push(unit);
            }
            current.clear();
            current_chars = 0;
        }
        current.push_str(token);
        current_chars += token_chars;
    }
    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        vec![text.to_string()]
    } else {
        // Any leftover mega-clause → word pack.
        parts
            .into_iter()
            .flat_map(|p| {
                if p.chars().count() > max_chars {
                    pack_words(&p, max_chars)
                } else {
                    vec![p]
                }
            })
            .collect()
    }
}

fn pack_words(text: &str, max_chars: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let next_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
        if !current.is_empty() && next_len > max_chars {
            parts.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    let tail = current.trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    if parts.is_empty() {
        vec![text.to_string()]
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speakable_units_splits_sentences() {
        let units = speakable_units(
            "The web search is empty right now. Try me again in a bit, or pick a city.",
        );
        assert_eq!(units.len(), 2);
        assert!(units[0].ends_with('.'));
        assert!(units[1].ends_with('.'));
    }

    #[test]
    fn speakable_units_keeps_short_reply() {
        let units = speakable_units("Phone stuff is easy.");
        assert_eq!(units, vec!["Phone stuff is easy.".to_string()]);
    }

    #[test]
    fn speakable_units_splits_long_clause_on_commas() {
        let long = "Okay bro, real talk: every search engine and job site is slamming the door on me with robot checks, so I can't pull real live Jharkhand job listings and I will not invent a fake list of openings for you today.";
        let units = speakable_units(long);
        assert!(
            units.len() >= 2,
            "expected multi-unit split, got {:?}",
            units
        );
        assert!(units.iter().all(|u| u.chars().count() <= PREFERRED_UNIT_CHARS + 40));
    }

    #[test]
    fn prefer_full_pcm_keeps_short_tail() {
        let audio = vec![0.1f32; 1000];
        // predicted slightly shorter than buffer
        let kept = prefer_full_pcm(&audio, 1000.0 / 44_100.0 - 0.001, 44_100);
        assert_eq!(kept.len(), 1000);
    }
}
