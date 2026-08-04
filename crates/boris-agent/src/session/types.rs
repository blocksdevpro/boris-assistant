//! Serializable session identity and metadata for Boris voice sessions.

use std::fmt;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Opaque session identifier (string newtype).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    /// Borrow the inner id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SessionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SessionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Lifecycle status of a voice session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Ended,
}

/// Persisted / in-memory metadata for one voice session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub status: SessionStatus,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    /// Optional human title later; empty is fine.
    pub title: String,
}

impl SessionMeta {
    /// Create an active session with timestamps set to now.
    pub fn new_active(id: SessionId) -> Self {
        let now = now_unix_ms();
        Self {
            id,
            status: SessionStatus::Active,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            title: String::new(),
        }
    }

    /// Refresh `updated_at_unix_ms` to the current time.
    pub fn touch(&mut self) {
        self.updated_at_unix_ms = now_unix_ms();
    }

    /// Mark the session ended and update timestamps.
    pub fn end(&mut self) {
        self.status = SessionStatus::Ended;
        self.touch();
    }
}

/// Current Unix time in milliseconds.
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Generate a session id without external crates: `s-{millis}-{pid}`.
pub fn generate_session_id() -> SessionId {
    let millis = now_unix_ms();
    let pid = process::id();
    SessionId(format!("s-{millis}-{pid}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_session_id_is_non_empty() {
        let id = generate_session_id();
        assert!(!id.as_str().is_empty());
        assert!(id.as_str().starts_with("s-"));
    }

    #[test]
    fn new_active_has_active_status() {
        let id = generate_session_id();
        let meta = SessionMeta::new_active(id.clone());
        assert_eq!(meta.status, SessionStatus::Active);
        assert_eq!(meta.id, id);
        assert!(meta.title.is_empty());
        assert_eq!(meta.created_at_unix_ms, meta.updated_at_unix_ms);
    }

    #[test]
    fn end_sets_ended_and_touches() {
        let mut meta = SessionMeta::new_active(generate_session_id());
        let created = meta.created_at_unix_ms;
        meta.end();
        assert_eq!(meta.status, SessionStatus::Ended);
        assert!(meta.updated_at_unix_ms >= created);
    }

    #[test]
    fn session_id_display_and_as_ref() {
        let id = SessionId::from("s-1-2");
        assert_eq!(id.to_string(), "s-1-2");
        assert_eq!(id.as_ref(), "s-1-2");
    }
}
