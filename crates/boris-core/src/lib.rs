pub mod error;
pub mod event;
pub mod types;

pub use crate::types::{AudioBuffer, AudioSample, ServiceKind, TurnId};

/// The sample rate all pipeline components operate at after resampling.
pub const AUDIO_TARGET_RATE: u32 = 16_000;
