//! Session persistence scaffolding for the agent.
//!
//! - [`types`] — session identity / metadata
//! - [`transcript`] — append-only message log shapes
//! - [`store`] — filesystem (or other) persistence backends
//!
//! Working memory remains [`crate::context::Context`]; session code loads and
//! saves message snapshots via [`crate::AgentEngine`] session-facing methods.
//!
//! Submodule bodies are owned by other P1 slices — declare modules here only.

pub mod store;
pub mod transcript;
pub mod types;

pub use store::SessionStore;
pub use types::{
    generate_session_id, now_unix_ms, SessionId, SessionMeta, SessionStatus,
};
