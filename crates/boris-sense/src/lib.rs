//! Local perception primitives: wake-word scoring and VAD.
//!
//! # Boundaries
//!
//! - **In scope:** "wake score?" / "is this frame speech?" + ORT init helpers.
//! - **Out of scope:** threads, session policy, STT/TTS, agent tools.
//!
//! The voice engine owns the capture loop and decides when to call these ports.

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
