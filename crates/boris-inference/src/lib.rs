//! Speech model **ports** (traits) shared by STT/TTS adapters and the engine.
//!
//! # Why a separate crate?
//!
//! Adapters (`boris-stt-*`, `boris-tts-*`) implement these traits against
//! `boris-core` types only. Perception (wake / VAD) lives in `boris-sense` and
//! must not become a dependency of model adapters.
//!
//! The voice engine in `boris-pipeline` holds `Box<dyn SpeechToText>` /
//! `Box<dyn TextToSpeech>` and never imports a concrete vendor crate at the
//! call site of `transcribe` / `synthesize` (feature gates pick the impl).
//!
//! # Object safety
//!
//! Both traits are object-safe (`&mut self` / `&self` methods, no generics) so
//! the engine can erase concrete backends behind `dyn`. Implementations only
//! need [`Send`] — models are typically used from one engine thread.
//!
//! # Load policy
//!
//! Prefer **explicit** `load` / `unload` from the host (preload while the user
//! speaks or the agent thinks). Adapters may also **lazy-load** on the first
//! `transcribe` / `synthesize`. Either way, missing config (paths, voices)
//! should map to [`boris_core::Error::Config`]; inference failures to
//! [`boris_core::Error::Other`].

#![deny(missing_docs)]

mod stt;
mod tts;

pub use stt::SpeechToText;
pub use tts::TextToSpeech;
