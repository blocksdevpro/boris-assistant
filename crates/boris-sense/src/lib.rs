//! Local perception primitives: wake-word scoring and VAD.
//!
//! # Boundaries
//!
//! - **In scope:** "wake score?" / "is this frame speech?" / "playback-like?"
//!   + ORT init helpers.
//! - **Out of scope:** threads, session policy, STT/TTS, agent tools.
//!
//! The voice engine owns the capture loop and decides when to call these ports.
//!
//! # Features
//!
//! | Feature | Default | Enables |
//! |---------|---------|---------|
//! | `vad`     | yes     | Silero VAD (`SileroVad`, needs ORT) |
//! | `wake`    | yes     | LiveKit wake-word + ORT init |
//! | `speaker` | yes     | Live-vs-loudspeaker acoustics (no extra ONNX) |
//!
//! Desktop / pipeline use the default feature set. `--features vad` without
//! `wake` still needs native ONNX Runtime. `--no-default-features` builds
//! only `pcm` / `time`.

pub mod pcm;
pub mod time;

#[cfg(feature = "vad")]
pub mod vad;

#[cfg(any(feature = "wake", feature = "vad"))]
pub mod ort;
#[cfg(feature = "wake")]
pub mod wake;
#[cfg(feature = "speaker")]
pub mod speaker;

// Pipeline rate for all sense adapters is 16 kHz mono.
const _: () = assert!(boris_core::AUDIO_TARGET_RATE == 16_000);

pub use pcm::{f32_to_pcm16_samples, f32_to_pcm16_samples_into};
pub use time::duration_to_samples;

#[cfg(feature = "vad")]
pub use time::{vad_initial_timeout_samples, vad_silence_samples};
#[cfg(feature = "vad")]
pub use vad::{
    SileroVad, Vad, SILERO_SPEECH_THRESHOLD, SILERO_VAD_CONTEXT_SAMPLES_16K,
    SILERO_VAD_FRAME_SAMPLES_16K, SILERO_VAD_INPUT_SAMPLES_16K, SILERO_VAD_STATE_SHAPE,
    VAD_INITIAL_TIMEOUT, VAD_PROCESSING_INTERVAL, VAD_SILENCE_WINDOW, VAD_WINDOW_SIZE,
};

#[cfg(any(feature = "wake", feature = "vad"))]
pub use ort::init_onnx_runtime;
#[cfg(feature = "wake")]
pub use wake::{
    LiveKitWakeWord, LivekitWakeWord, WakeWord, WAKEWORD_PROCESSING_INTERVAL, WAKEWORD_THRESHOLD,
    WAKEWORD_WINDOW_SIZE,
};
#[cfg(feature = "speaker")]
pub use speaker::{
    compute_acoustic_feat, AcousticFeat, AcousticModel, MATCH_Z_REJECT, PLAYBACK_Z_REJECT,
};
