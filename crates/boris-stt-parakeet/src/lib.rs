use std::path::Path;

use boris_core::{error::Result, AudioSample};
use boris_inference::SpeechToText;
use transcribe_rs::{
    onnx::{parakeet::ParakeetModel, Quantization},
    SpeechModel, TranscribeOptions,
};

pub struct ParakeetSTT {
    model: Option<ParakeetModel>,
}

impl ParakeetSTT {
    pub fn new() -> Self {
        Self { model: None }
    }
}

impl Default for ParakeetSTT {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechToText for ParakeetSTT {
    fn load(&mut self) -> Result<()> {
        if self.model.is_none() {
            let model =
                ParakeetModel::load(Path::new("./assets/models/parakeet/"), &Quantization::Int8)
                    .map_err(|e| boris_core::error::Error::Other(e.to_string()))?;
            self.model = Some(model);
        }
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }

    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
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
