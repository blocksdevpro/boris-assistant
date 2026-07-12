use std::{path::Path, time::Instant};

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

/// Supertonic Model dir.
pub const SUPERTONE_MODEL_DIR: &str = "assets/models/supertone/onnx/";

/// Supertonic Voice path.
pub const SUPERTONE_VOICE_DIR: &str = "assets/models/supertone/voices/";

pub struct SupertoneTts {
    runtime: tokio::runtime::Runtime,
    model: Option<Tts>,
    voice: String,
    lang: String,
    params: SynthesisParams,
}

impl SupertoneTts {
    pub fn new() -> Self {
        Self::with_voice("M4")
    }

    pub fn with_voice(voice: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for Supertone TTS");

        Self {
            runtime,
            model: None,
            voice: voice.to_string(),
            lang: "en".into(),
            params: SynthesisParams {
                total_step: 8,
                speed: 1.17,
                silence_duration: 0.18,
                rng_seed: None,
            },
        }
    }

    /// Language code (`en`, `de`, `na`, …). See Supertonic docs for all 31.
    pub fn with_lang(mut self, lang: impl Into<String>) -> Self {
        self.lang = lang.into();
        self
    }

    /// Denoising steps: 5 (fast) … 12 (high quality). Default 8.
    pub fn with_total_step(mut self, steps: usize) -> Self {
        self.params.total_step = steps;
        self
    }

    /// Speech rate: ~0.7 (slow) … 2.0 (fast). Default 1.05.
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

        tracing::info!(
            model = SUPERTONE_MODEL_ID,
            voice = %self.voice,
            "Loading Supertone TTS (downloads on first run)…"
        );
        let t = Instant::now();

        let model_dir = Path::new(SUPERTONE_MODEL_DIR);
        let voice_path = Path::new(SUPERTONE_VOICE_DIR).join(self.voice.clone() + ".json");

        let model = Tts::from_local(model_dir, voice_path)
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
