//! Wake-word scoring port + LiveKit ONNX adapter.
//!
//! The engine maintains a [`WAKEWORD_WINDOW_SIZE`] rolling buffer and calls
//! [`WakeWord::predict`] on an interval. Crossing [`WAKEWORD_THRESHOLD`] starts
//! a listen turn — policy stays in `boris-pipeline`, not here.

#[cfg(feature = "wake")]
mod livekit;

use std::time::Duration;

use boris_core::{AudioSample, Result};

#[cfg(feature = "wake")]
pub use livekit::{LiveKitWakeWord, LivekitWakeWord};

/// Score above which the engine treats the window as a wake hit.
pub const WAKEWORD_THRESHOLD: f32 = 0.5;

/// Rolling window length: 2 seconds at 16 kHz.
pub const WAKEWORD_WINDOW_SIZE: usize = 32_000;

/// Suggested spacing between wake evaluations.
pub const WAKEWORD_PROCESSING_INTERVAL: Duration = Duration::from_millis(80);

/// Scores a mono PCM window; higher means more confident wake detection.
pub trait WakeWord: Send {
    /// Return a confidence in roughly `[0.0, 1.0]` (model-dependent).
    fn predict(&mut self, audio: &[AudioSample]) -> Result<f32>;
}
