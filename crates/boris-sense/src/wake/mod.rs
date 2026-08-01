//! Wake-word scoring port + LiveKit adapter.

mod livekit;

use std::time::Duration;

use boris_core::{error::Result, AudioSample};

pub use livekit::LivekitWakeWord;

pub const WAKEWORD_THRESHOLD: f32 = 0.5;
pub const WAKEWORD_WINDOW_SIZE: usize = 32_000; // 2 sec audio, 16 kHz
pub const WAKEWORD_PROCESSING_INTERVAL: Duration = Duration::from_millis(80);

/// Scores a mono PCM window; higher means more confident wake detection.
pub trait WakeWord: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32>;
}
