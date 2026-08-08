//! Shared foundation types for the Boris workspace.
//!
//! This crate is intentionally small and dependency-light. Higher crates
//! (`boris-audio`, `boris-sense`, `boris-inference`, adapters, `boris-pipeline`)
//! build on these primitives so audio sample layout, turn identity, and
//! shared errors stay consistent.
//!
//! # What belongs here
//!
//! - Audio sample/buffer aliases and the pipeline sample rate constant
//! - Turn identity used to drop late async results
//! - A small shared [`Error`] type for speech/audio adapters
//!
//! # What does **not** belong here
//!
//! - Agent loop types (see `boris-agent`)
//! - LLM provider clients (see `boris-ai`)
//! - Engine phase / UI status (see `boris-pipeline`)
//! - Session FSM / worker event buses (removed; the product engine is sequential)

#![deny(missing_docs)]

pub mod error;
pub mod types;

// ── Crate-root re-exports (preferred import path for hosts & siblings) ───────

pub use error::{Error, Result};
pub use types::{
    ArcAudioBuffer, AudioBuffer, AudioSample, ServiceKind, TurnId, AUDIO_TARGET_RATE,
};
