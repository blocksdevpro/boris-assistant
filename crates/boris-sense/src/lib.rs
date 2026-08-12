//! Local perception primitives: wake-word scoring and VAD.
//!
//! # Boundaries
//!
//! - **In scope:** "wake score?" / "is this frame speech?" + ORT init helpers.
//! - **Out of scope:** threads, session policy, STT/TTS, agent tools.
//!
//! The voice engine owns the capture loop and decides when to call these ports.
//!
//! # Features
//!
//! | Feature | Default | Enables |
//! |---------|---------|---------|
//! | `vad`   | yes     | WebRTC VAD (`WebRtcVad`) |
//! | `wake`  | yes     | LiveKit wake-word + ORT init |
//!
//! Desktop / pipeline use the default feature set. Disable `wake` to build
//! without `ort` / `livekit-wakeword` (e.g. lighter local builds).

pub mod pcm;
pub mod time;

#[cfg(feature = "vad")]
pub mod vad;

#[cfg(feature = "wake")]
pub mod ort;
#[cfg(feature = "wake")]
pub mod wake;

// Pipeline rate for all sense adapters is 16 kHz mono.
const _: () = assert!(boris_core::AUDIO_TARGET_RATE == 16_000);

pub use pcm::{f32_to_pcm16_samples, f32_to_pcm16_samples_into};
pub use time::duration_to_samples;

#[cfg(feature = "vad")]
pub use time::{vad_initial_timeout_samples, vad_silence_samples};
#[cfg(feature = "vad")]
pub use vad::{
    Vad, WebRtcVad, VAD_INITIAL_TIMEOUT, VAD_PROCESSING_INTERVAL, VAD_SILENCE_WINDOW,
    VAD_WINDOW_SIZE, WEBRTC_VAD_FRAME_SAMPLES_16K,
};

#[cfg(feature = "wake")]
pub use ort::init_onnx_runtime;
#[cfg(feature = "wake")]
pub use wake::{
    LiveKitWakeWord, LivekitWakeWord, WakeWord, WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD,
    WAKEWORD_WINDOW_SIZE,
};
