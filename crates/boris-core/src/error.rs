//! Shared error type for speech/audio adapters and thin hosts.
//!
//! Domain crates may still define their own richer errors (`AgentError`,
//! `LlmError`, …). Use this type at the `boris-core` / `boris-inference`
//! boundary so STT/TTS traits stay simple.
//!
//! # Variant mapping convention
//!
//! When mapping failures into [`Error`], pick the variant by **source domain**:
//!
//! | Domain | Variant | Examples |
//! |--------|---------|----------|
//! | Paths, settings, env, missing config | [`Error::Config`] | invalid model path, missing API key, bad settings JSON |
//! | Device I/O, capture, playback, resample | [`Error::Audio`] | mic open failed, device busy, resample error |
//! | Model load, runtime, adapter glue, misc | [`Error::Other`] | ONNX load failure, unexpected panic message, unclassified |
//!
//! A future `Model` (or similar) variant may absorb model/runtime failures;
//! until then use [`Error::Other`] (or [`Error::other`]) for those cases.
//!
//! # `From<String>` / `From<&str>`
//!
//! [`From<String>`] and [`From<&str>`] **only** construct [`Error::Other`].
//! They exist for ergonomic catch-alls (`?` on stringly APIs, quick adapters).
//! Classified failures **must** use the constructors [`Error::config`],
//! [`Error::audio`], or [`Error::other`] so the variant stays meaningful for
//! logging and recovery.

use thiserror::Error;

/// Shared failure kind for core audio/speech boundaries.
///
/// # Mapping convention
///
/// | Domain | Variant |
/// |--------|---------|
/// | Paths, settings, env | [`Error::Config`] |
/// | Device I/O, capture, playback, resample | [`Error::Audio`] |
/// | Model load, runtime, unclassified | [`Error::Other`] |
///
/// `From<String>` / `From<&str>` only create [`Error::Other`]. Use
/// [`Error::config`], [`Error::audio`], or [`Error::other`] for classified
/// failures. A future `Model` variant may absorb model/runtime errors.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Invalid or missing configuration (paths, env, settings).
    #[error("config error: {0}")]
    Config(String),

    /// Capture, playback, resample, or device failure.
    #[error("audio error: {0}")]
    Audio(String),

    /// Catch-all for adapter/runtime/model failures that do not fit above.
    ///
    /// Prefer a dedicated variant when one exists; until a `Model` kind is
    /// added, model and runtime errors land here.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Build a [`Error::Config`] from anything displayable.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Build a [`Error::Audio`] from anything displayable.
    pub fn audio(msg: impl Into<String>) -> Self {
        Self::Audio(msg.into())
    }

    /// Build a [`Error::Other`] from anything displayable.
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

impl From<String> for Error {
    /// Always produces [`Error::Other`]. Use [`Error::config`] / [`Error::audio`]
    /// when the failure is classified.
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}

impl From<&str> for Error {
    /// Always produces [`Error::Other`]. Use [`Error::config`] / [`Error::audio`]
    /// when the failure is classified.
    fn from(value: &str) -> Self {
        Self::Other(value.to_string())
    }
}

/// Result alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_and_display() {
        let c = Error::config("missing key");
        assert!(matches!(c, Error::Config(_)));
        assert_eq!(c.to_string(), "config error: missing key");

        let a = Error::audio("device busy");
        assert_eq!(a.to_string(), "audio error: device busy");

        let o: Error = "boom".into();
        assert_eq!(o, Error::Other("boom".into()));
    }

    #[test]
    fn from_string_and_str_are_other_only() {
        let from_string: Error = String::from("owned").into();
        assert!(matches!(from_string, Error::Other(_)));
        assert_eq!(from_string, Error::Other("owned".into()));
        // Display for Other is the raw message (no prefix).
        assert_eq!(from_string.to_string(), "owned");

        let from_str: Error = "borrowed".into();
        assert!(matches!(from_str, Error::Other(_)));
        assert_eq!(from_str, Error::Other("borrowed".into()));
        assert_eq!(from_str.to_string(), "borrowed");
    }

    #[test]
    fn display_prefixes_are_stable_contracts() {
        // Config and Audio carry a stable prefix used by hosts/logs.
        assert_eq!(Error::config("x").to_string(), "config error: x");
        assert_eq!(Error::audio("y").to_string(), "audio error: y");
        // Other has no prefix — message is the Display form.
        assert_eq!(Error::other("z").to_string(), "z");
    }
}
