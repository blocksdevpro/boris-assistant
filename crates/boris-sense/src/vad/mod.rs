//! Voice-activity detection port + Silero ONNX adapter.
//!
//! The engine feeds fixed-size hops ([`VAD_WINDOW_SIZE`]) at
//! [`boris_core::AUDIO_TARGET_RATE`] and uses silence/initial timeouts from
//! [`crate::time`] measured in **samples**.

#[cfg(feature = "vad")]
mod silero;

use std::time::Duration;

use boris_core::{AudioSample, Result};

#[cfg(feature = "vad")]
pub use silero::{
    SileroVad, SILERO_SPEECH_THRESHOLD, SILERO_VAD_CONTEXT_SAMPLES_16K,
    SILERO_VAD_FRAME_SAMPLES_16K, SILERO_VAD_INPUT_SAMPLES_16K, SILERO_VAD_STATE_SHAPE,
};

/// How long to wait for the first speech after arming listen (audio-time).
pub const VAD_INITIAL_TIMEOUT: Duration = Duration::from_millis(1600);

/// Trailing non-speech before we end a freeform utterance.
///
/// Sized for Silero, not WebRTC. LiveKit Agents' Silero plugin defaults to
/// 550 ms (`min_silence_duration`) for the same streaming graph; Silero's own
/// `VADIterator` uses 100 ms, which is for file segmentation and cuts
/// mid-clause on a voice loop. WebRTC needed ~900 ms because the GMM flickered
/// to silence mid-sentence; Silero does not, so we can follow LiveKit.
pub const VAD_SILENCE_WINDOW: Duration = Duration::from_millis(550);

/// Native Silero hop duration (32 ms at 16 kHz). Not used as a skip interval —
/// every hop must be scored so the LSTM state stays aligned.
pub const VAD_PROCESSING_INTERVAL: Duration = Duration::from_millis(32);

/// Silero hop: 32 ms at 16 kHz.
pub const VAD_WINDOW_SIZE: usize = 512;

#[cfg(feature = "vad")]
const _: () = assert!(VAD_WINDOW_SIZE == SILERO_VAD_FRAME_SAMPLES_16K);

/// `true` = speech present in this hop.
///
/// Implementations must be cheap enough to run on the engine thread for every
/// window; heavy work belongs in STT, not here.
pub trait Vad: Send {
    /// Classify one mono PCM hop (exactly [`VAD_WINDOW_SIZE`] samples @ 16 kHz).
    fn predict(&mut self, audio: &[AudioSample]) -> Result<bool>;

    /// Clear backend memory so the next utterance does not inherit LSTM/context.
    ///
    /// Default: no-op (stateless backends).
    fn reset(&mut self) {}
}
