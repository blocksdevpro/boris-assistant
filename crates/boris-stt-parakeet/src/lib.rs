use std::path::Path;

use boris_core::error::Result;
use boris_inference::SpeechToText;
use transcribe_rs::{
    SpeechModel, TranscribeOptions,
    onnx::{Quantization, parakeet::ParakeetModel},
};

pub struct ParakeetSTT {
    model: Option<ParakeetModel>,
}

impl ParakeetSTT {
    pub fn new() -> Self {
        Self { model: None }
    }
}

impl SpeechToText for ParakeetSTT {
    fn load(&mut self) -> Result<()> {
        if self.model.is_none() {
            let model_path = "./assets/models/parakeet/";
            let model = ParakeetModel::load(Path::new(model_path), &Quantization::Int8).unwrap();
            self.model = Some(model);
        }
        Ok(())
    }

    fn unload(&mut self) -> Result<()> {
        self.model = None;
        Ok(())
    }

    fn transcribe(&mut self, audio: &[boris_core::AudioSample]) -> Result<String> {
        let model = self.model.as_mut().expect("Model not loaded");
        let result = model
            .transcribe(
                audio,
                &TranscribeOptions {
                    language: Some("en".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        Ok(result.text)
    }
}
