pub mod wakeword;

use boris_core::{AudioSample, AudioSampleBuffer, error::BorisResult};

pub trait WakeWordDetector: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<f32>;
}

pub trait VoiceActivityDetector: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<f32>;
}

pub trait SpeechToText: Send {
    fn transcribe(&mut self, audio: &[AudioSample]) -> BorisResult<String>;
}

pub trait TextToSpeech: Send {
    fn synthesize(&mut self, text: &str) -> BorisResult<AudioSampleBuffer>;
}
