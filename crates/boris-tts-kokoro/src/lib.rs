use std::time::Instant;

use any_tts::{ModelType, SynthesisRequest, TtsConfig, TtsModel, load_model};
use boris_core::{AudioBuffer, error::Result};
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
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
        let start = Instant::now();
        let model = self.model.as_ref().unwrap();
        let result = model
            .synthesize(
                &SynthesisRequest::new(text)
                    .with_language("English")
                    .with_voice("bm_lewis"),
            )
            .map_err(|e| boris_core::error::Error::Other(e.to_string()))?;
        tracing::info!("any-tts synthesize took {}ms", start.elapsed().as_millis());
        Ok(result.samples)
    }
    
    fn load(&mut self) -> Result<()> {
        if self.model.is_some() {
            return Ok(());
        }

        tracing::info!("Loading any-tts Kokoro model…");
        let t = Instant::now();

        let model =
            load_model(TtsConfig::new(ModelType::Kokoro).with_model_path(KOKORO_MODEL_PATH))
                .unwrap();
        
        self.model = Some(model);
        tracing::info!("any-tts Kokoro model loaded in {}ms", t.elapsed().as_millis());

        // Pre-warm (JIT compilation overhead for candle)
        tracing::info!("Pre-warming any-tts Kokoro…");
        let pw = Instant::now();
        let _ = self.synthesize("Hi.");
        tracing::info!("Pre-warm done in {}ms", pw.elapsed().as_millis());

        Ok(())
    }
    
    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }
}
