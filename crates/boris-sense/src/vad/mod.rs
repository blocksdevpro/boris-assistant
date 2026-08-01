//! Voice-activity detection port + WebRTC adapter.

mod webrtc;

use std::time::Duration;

use boris_core::{error::Result, AudioSample};

pub use webrtc::WebRtcVad;

/// Give up if the user never starts speaking after wake / follow-up open.
pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);
/// End utterance after this much non-speech following real speech.
/// Slightly longer than 400ms so soft endings aren't cut; short enough for snappy turns.
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(450);
pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(40);
pub const VAD_WINDOW_SIZE: usize = 160; // 10 ms at 16 kHz (WebRTC frame size)

/// `true` = speech present in this frame.
pub trait Vad: Send {
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool>;
}
