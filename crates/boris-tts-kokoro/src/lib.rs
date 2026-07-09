use std::time::Instant;

use any_tts::{load_model, ModelType, SynthesisRequest, TtsConfig, TtsModel};
use boris_core::{
    error::{Error, Result},
    AudioBuffer,
};
use boris_inference::TextToSpeech;

pub const KOKORO_SAMPLE_RATE: u32 = 24_000;
pub const KOKORO_MODEL_PATH: &str = "./assets/models/kokoro";

pub struct KokoroTts {
    model: Option<Box<dyn TtsModel>>,
}

impl KokoroTts {
    pub fn new() -> Self {
        Self { model: None }
    }
}

impl Default for KokoroTts {
    fn default() -> Self {
        Self::new()
    }
}

impl TextToSpeech for KokoroTts {
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        tracing::info!("Loading Kokoro TTS model…");
        let t = Instant::now();

        let model =
            load_model(TtsConfig::new(ModelType::Kokoro).with_model_path(KOKORO_MODEL_PATH))
                .map_err(|e| Error::Other(e.to_string()))?;
        self.model = Some(model);

        tracing::info!("Kokoro TTS model loaded in {}ms", t.elapsed().as_millis());

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
        let result = model
            .synthesize(
                &SynthesisRequest::new(text)
                    .with_language("English")
                    .with_voice("bm_lewis"),
            )
            .map_err(|e| Error::Other(e.to_string()))?;

        tracing::info!("Kokoro synthesis took {}ms", start.elapsed().as_millis());
        Ok(result.samples)
    }
}
