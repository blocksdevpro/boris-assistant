//! Local perception primitives for Boris: wake-word scoring and VAD.
//!
//! No threads, no session policy, no STT/TTS. The engine (or legacy workers)
//! owns loops; this crate only answers "wake score?" / "is this speech?".

pub mod ort;
pub mod pcm;
pub mod time;
pub mod vad;
pub mod wake;

pub use ort::init_onnx_runtime;
pub use pcm::f32_to_pcm16_samples;
pub use time::{duration_to_samples, vad_initial_timeout_samples, vad_silence_samples};
pub use vad::{
    Vad, WebRtcVad, VAD_INITIAL_TIMEOUT, VAD_PROCESSING_INTERVAL, VAD_SILENCE_WINDOW,
    VAD_WINDOW_SIZE,
};
pub use wake::{
    LivekitWakeWord, WakeWord, WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD,
    WAKEWORD_WINDOW_SIZE,
};
