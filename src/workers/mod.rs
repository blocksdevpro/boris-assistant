//! Background workers and sensors for the voice pipeline.
//!
//! These own threads and channels. Product policy (when to start/stop, legal
//! transitions) lives in [`crate::session`], not here.

pub mod agent;
pub mod audio;
pub mod inference;
pub mod tts;
