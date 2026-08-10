//! Typed errors for pipeline public/setup entry points.
//!
//! Hosts that need `String` (Tauri IPC) can use [`PipelineError::to_string`] /
//! `map_err(|e| e.to_string())`.

use thiserror::Error;

/// Failure kinds for settings, model install, and engine init.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Filesystem / IO under `~/.boris` or model dirs.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// User prefs / secrets load or save.
    #[error("settings: {0}")]
    Settings(String),

    /// Model download / install.
    #[error("download: {0}")]
    Download(String),

    /// Engine thread setup (audio, wake, runtime, spawn).
    #[error("init: {0}")]
    Init(String),

    /// Catch-all for migrated `String` paths.
    #[error("{0}")]
    Other(String),
}

impl PipelineError {
    pub fn settings(msg: impl Into<String>) -> Self {
        Self::Settings(msg.into())
    }

    pub fn download(msg: impl Into<String>) -> Self {
        Self::Download(msg.into())
    }

    pub fn init(msg: impl Into<String>) -> Self {
        Self::Init(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

impl From<String> for PipelineError {
    fn from(value: String) -> Self {
        Self::Other(value)
    }
}

impl From<&str> for PipelineError {
    fn from(value: &str) -> Self {
        Self::Other(value.to_string())
    }
}

/// Result alias using [`PipelineError`].
pub type Result<T> = std::result::Result<T, PipelineError>;
