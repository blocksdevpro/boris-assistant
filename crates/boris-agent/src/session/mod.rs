//! Session persistence scaffolding for the agent.
//!
//! - [`types`] — session identity / metadata
//! - [`transcript`] — append-only message log shapes + pure wire helpers
//! - [`store`] — filesystem persistence (`summary.json`, `chat_history.jsonl`, …)
//! - [`artifacts`] — session-local visual cards (`artifacts/index.json` + files)
//!
//! Working memory remains [`crate::context::Context`]; session code loads and
//! saves message snapshots via [`crate::Agent`] session-facing methods.

pub mod artifacts;
pub mod store;
pub mod transcript;
pub mod types;

pub use artifacts::{
    ArtifactIndex, ArtifactKind, ArtifactMeta, ArtifactStore, PresentRequest, PresentedArtifact,
};
pub use store::SessionStore;
pub use types::{generate_session_id, now_unix_ms, SessionId, SessionMeta, SessionStatus};
