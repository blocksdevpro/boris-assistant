//! Session persistence scaffolding for the agent.
//!
//! - [`types`] — session identity / metadata
//! - [`transcript`] — append-only message log shapes + pure wire helpers
//! - [`store`] — filesystem persistence (`summary.json`, `chat_history.jsonl`, …)
//!
//! Working memory remains [`crate::context::Context`]; session code loads and
//! saves message snapshots via [`crate::Agent`] session-facing methods.

pub mod store;
pub mod transcript;
pub mod types;

pub use store::SessionStore;
pub use types::{generate_session_id, now_unix_ms, SessionId, SessionMeta, SessionStatus};
