//! Voice-activity detection port + WebRTC adapter.

mod webrtc;

use std::time::Duration;

use boris_core::{error::Result, AudioSample};

pub use webrtc::WebRtcVad;

pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);
/// Trailing non-speech before we end the utterance.
///
/// 600ms was cutting multi-sentence user speech on natural mid-thought pauses,
/// so STT only ever saw the first sentence. ~900ms still endpoints promptly
/// after a real stop without swallowing the next clause.
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(900);
pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(40);
pub const VAD_WINDOW_SIZE: usize = 160; // 10 ms at 16 kHz (WebRTC frame size)

/// `true` = speech present in this frame.
pub trait Vad: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool>;
}
