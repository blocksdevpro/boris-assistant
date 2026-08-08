//! Serializable session identity and metadata for Boris voice sessions.

use std::fmt;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Generate a Grok-like UUID session id (`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`).
///
/// Uses wall-clock, process id, and a process-local counter for uniqueness —
/// no extra crate. Matches the on-disk folder naming style of `~/.grok/sessions`.
pub fn generate_session_id() -> SessionId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let t = now_unix_ms();
    let pid = process::id() as u64;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Mix independent entropy sources into 128 bits (UUID layout).
    let a = t
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(pid << 32)
        .wrapping_add(n);
    let b = (pid.wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(n.wrapping_mul(0x1656_67B1))
        .wrapping_add(t.rotate_left(17));

    // RFC 4122 variant + version 4 shape (random-class UUID).
    let time_low = (a >> 32) as u32;
    let time_mid = ((a >> 16) & 0xffff) as u16;
    let time_hi = (0x4000 | (a & 0x0fff)) as u16; // version 4
    let clock_seq = (0x8000 | ((b >> 48) & 0x3fff)) as u16; // variant 10xx
    let node = b & 0x0000_ffff_ffff_ffff;

    SessionId(format!(
        "{time_low:08x}-{time_mid:04x}-{time_hi:04x}-{clock_seq:04x}-{node:012x}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_session_id_is_uuid_shaped() {
        let id = generate_session_id();
        assert!(!id.as_str().is_empty());
        let parts: Vec<_> = id.as_str().split('-').collect();
        assert_eq!(parts.len(), 5, "uuid has 5 hyphen groups: {}", id.as_str());
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        // Version nibble is 4.
        assert!(
            parts[2].starts_with('4'),
            "version 4 uuid: {}",
            id.as_str()
        );
    }

    #[test]
    fn generate_session_id_is_unique() {
        let a = generate_session_id();
        let b = generate_session_id();
        assert_ne!(a, b);
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
        let id = SessionId::from("019f4633-5c50-7293-a20d-62505015e1a4");
        assert_eq!(id.to_string(), "019f4633-5c50-7293-a20d-62505015e1a4");
        assert_eq!(id.as_ref(), "019f4633-5c50-7293-a20d-62505015e1a4");
    }
}
