//! Voice-activity detection port + WebRTC adapter.
//!
//! The engine feeds fixed-size frames ([`VAD_WINDOW_SIZE`]) at
//! [`boris_core::AUDIO_TARGET_RATE`] and uses silence/initial timeouts from
//! [`crate::time`] measured in **samples**.

#[cfg(feature = "vad")]
mod webrtc;

use std::time::Duration;

use boris_core::{AudioSample, Result};

#[cfg(feature = "vad")]
pub use webrtc::{WebRtcVad, WEBRTC_VAD_FRAME_SAMPLES_16K};

/// How long to wait for the first speech after arming listen (audio-time).
pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);

/// Trailing non-speech before we end the utterance.
///
/// 600ms was cutting multi-sentence user speech on natural mid-thought pauses.
/// ~900ms still endpoints promptly after a real stop without swallowing the
/// next clause.
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(900);

/// Suggested spacing between VAD evaluations on the capture stream.
pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(40);

/// WebRTC frame size: 10 ms at 16 kHz (see also 20/30 ms: 320 / 480 samples).
pub const VAD_WINDOW_SIZE: usize = 160;

/// `true` = speech present in this frame.
///
/// Implementations must be cheap enough to run on the engine thread for every
/// window; heavy work belongs in STT, not here.
pub trait Vad: Send {
    /// Classify one mono PCM frame (typically [`VAD_WINDOW_SIZE`] samples).
    ///
    /// WebRTC VAD accepts only 10/20/30 ms frames at 16 kHz (160/320/480 samples).
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool>;
}
