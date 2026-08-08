//! Shared error type for speech/audio adapters and thin hosts.
//!
//! Domain crates may still define their own richer errors (`AgentError`,
//! `LlmError`, …). Use this type at the `boris-core` / `boris-inference`
//! boundary so STT/TTS traits stay simple.

use thiserror::Error;

/// Shared failure kind for core audio/speech boundaries.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Invalid or missing configuration (paths, env, settings).
    #[error("config error: {0}")]
    Config(String),

    /// Capture, playback, resample, or device failure.
    #[error("audio error: {0}")]
    Audio(String),

    /// Catch-all for adapter/runtime failures that do not fit above.
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
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}

impl From<&str> for Error {
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
}
