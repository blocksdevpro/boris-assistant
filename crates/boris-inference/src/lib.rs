pub mod vad;
pub mod wakeword;

use std::time::Duration;

use boris_core::{AudioSample, AudioSampleBuffer, error::BorisResult};

pub const WAKEWORD_THRESHOLD: f32 = 0.5;
pub const WAKEWORD_WINDOW_SIZE: usize = 32_000; // 2 sec audio, 16 kHz
pub const WAKEWORD_PROCESSING_INTERVAL: Duration = Duration::from_millis(80);

pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(600);

pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(10);
pub const VAD_WINDOW_SIZE: usize = 160; // 10 ms at 16 kHz

pub trait WakeWordDetector: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<f32>;
}

pub trait VoiceActivityDetector: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> BorisResult<bool>;
}

pub trait SpeechToText: Send {
    fn transcribe(&mut self, audio: &[AudioSample]) -> BorisResult<String>;
}

pub trait TextToSpeech: Send {
    fn synthesize(&mut self, text: &str) -> BorisResult<AudioSampleBuffer>;
}

/// Converts a normalized &[f32] audio sample (-1.0..1.0) into PCM16 Vec<i16>.
///
/// Values outside the range are clamped.
#[inline]
pub fn f32_to_pcm16_samples(audio: &[AudioSample]) -> Vec<i16> {
    audio
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}
