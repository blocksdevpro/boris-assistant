pub mod wakeword;

use std::time::Duration;

use boris_core::{AudioSample, AudioSampleBuffer, error::BorisResult};

pub const WAKEWORD_THRESHOLD: f32 = 0.5;
pub const WAKEWORD_WINDOW_SIZE: usize = 32_000; // 2 sec audio, 16 kHz
pub const WAKEWORD_PROCESSING_INTERVAL: Duration = Duration::from_millis(80);

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
