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
                // Fewer diffusion steps → lower synth latency (quality trade-off).
                total_step: 6,
                speed: 1.17,
                // Less trailing silence on each clip.
                silence_duration: 0.12,
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

        let result = self
            .runtime
            .block_on(async { model.synthesize(text, &lang, Some(&params)).await })
            .map_err(|e| Error::Other(format!("Supertone synthesis failed: {e}")))?;

        tracing::info!(
            samples = result.audio.len(),
            duration_secs = result.duration_secs,
            sample_rate = result.sample_rate,
            "Supertone synthesis took {}ms",
            start.elapsed().as_millis()
        );

        Ok(result.audio)
    }
}
