use std::path::Path;

use boris_core::error::Result;
use boris_inference::SpeechToText;
use transcribe_rs::{
    SpeechModel, TranscribeOptions,
    onnx::{Quantization, parakeet::ParakeetModel},
};

pub struct ParakeetSTT {
    model: ParakeetModel,
}

impl ParakeetSTT {
    pub fn new() -> Self {
        let model_path = "./assets/models/parakeet/";
        let model = ParakeetModel::load(Path::new(model_path), &Quantization::Int8).unwrap();
        Self { model }
    }
}

impl SpeechToText for ParakeetSTT {
    fn transcribe(&mut self, audio: &[boris_core::AudioSample]) -> Result<String> {
        let result = self
            .model
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
