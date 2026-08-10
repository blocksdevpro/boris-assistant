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
//!
//! # Error taxonomy
//!
//! Map failures by domain: paths/settings → [`Error::Config`], device/I-O/resample
//! → [`Error::Audio`], model/runtime/misc → [`Error::Other`]. See the docs on
//! [`Error`] for the full convention. `From<String>` / `From<&str>` only create
//! `Other`; classified failures should use [`Error::config`], [`Error::audio`],
//! or [`Error::other`].

#![deny(missing_docs)]

mod error;
mod types;

// ── Crate-root re-exports (preferred import path for hosts & siblings) ───────

pub use error::{Error, Result};
pub use types::{
    ArcAudioBuffer, AudioBuffer, AudioSample, TurnId, AUDIO_TARGET_RATE,
};
