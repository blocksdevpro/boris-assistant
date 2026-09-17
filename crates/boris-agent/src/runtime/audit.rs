//! Append-only audit log for tool invocations.

use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, warn};

/// One structured audit line (JSONL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub ts_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub tool: String,
    pub risk: String,
    /// allow | deny | confirm | confirmed | rejected | timeout | error | input
    pub decision: String,
    pub args_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// Whether the recorded observation was truncated (additive; defaults to
    /// `false` for deny/confirm/rejected paths that never execute).
    #[serde(default)]
    pub truncated: bool,
    /// Byte length of the recorded observation text (`0` when no observation).
    #[serde(default)]
    pub bytes: usize,
}

/// Sink for audit events (file, null, or test capture).
pub trait AuditSink: Send + Sync {
    fn write(&self, event: &AuditEvent);
}

/// Discards all events.
#[derive(Debug, Default)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn write(&self, _event: &AuditEvent) {}
}

/// Append JSONL lines to a file.
///
/// Soft-fails on I/O errors (never panics the tool path) but counts failures
/// in [`JsonlAuditSink::failed_count`] and logs at `error` level (not just
/// `warn`) so hosts can alert on audit loss.
pub struct JsonlAuditSink {
    path: PathBuf,
    lock: Mutex<()>,
    failed_writes: AtomicU64,
}

impl JsonlAuditSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
            failed_writes: AtomicU64::new(0),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of failed audit writes since construction (mkdir / serialize /
    /// open / write). Monotonic; never resets.
    pub fn failed_count(&self) -> u64 {
        self.failed_writes.load(Ordering::Relaxed)
    }

    fn note_failure(&self, msg: &str) {
        self.failed_writes.fetch_add(1, Ordering::Relaxed);
        error!(
            path = %self.path.display(),
            failed = self.failed_count(),
            "{msg}"
        );
    }
}

impl AuditSink for JsonlAuditSink {
    fn write(&self, event: &AuditEvent) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = fs::create_dir_all(parent) {
                    warn!(error = %e, path = %self.path.display(), "audit mkdir failed");
                    self.note_failure("audit mkdir failed");
                    return;
                }
            }
        }
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "audit serialize failed");
                self.note_failure("audit serialize failed");
                return;
            }
        };
        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) => {
                warn!(error = %e, path = %self.path.display(), "audit open failed");
                self.note_failure("audit open failed");
                return;
            }
        };
        if let Err(e) = writeln!(file, "{line}") {
            warn!(error = %e, "audit write failed");
            self.note_failure("audit write failed");
        }
    }
}

/// In-memory sink for tests.
#[derive(Debug, Default)]
pub struct MemoryAuditSink {
    pub events: Mutex<Vec<AuditEvent>>,
}

impl MemoryAuditSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.events.lock().map(|v| v.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AuditSink for MemoryAuditSink {
    fn write(&self, event: &AuditEvent) {
        if let Ok(mut v) = self.events.lock() {
            v.push(event.clone());
        }
    }
}

/// Unix epoch milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Non-cryptographic stable-ish digest of args (not for security).
///
/// Uses `std::collections::hash_map::DefaultHasher` (SipHash-based, **not**
/// collision-resistant and **not** stable across processes/versions). It is
/// only a low-cardinality correlation key for audit lines — never a security
/// boundary, dedup key, or integrity check. Secrets are redacted before
/// hashing on a best-effort basis (see [`redact_secrets` docs]).
///
/// Redacts common secret keys before hashing.
pub fn args_digest(args: &Value) -> String {
    let redacted = redact_secrets(args.clone());
    let s = redacted.to_string();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Voice-safe one-line summary of tool args (redacted, truncated).
pub fn args_summary(tool_name: &str, args: &Value) -> String {
    let redacted = redact_secrets(args.clone());
    let compact = match &redacted {
        Value::Object(map) if map.is_empty() => String::new(),
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .take(4)
                .map(|(k, v)| {
                    let vs = match v {
                        Value::String(s) => truncate_chars(s, 40),
                        other => truncate_chars(&other.to_string(), 40),
                    };
                    format!("{k}={vs}")
                })
                .collect();
            parts.join(", ")
        }
        other => truncate_chars(&other.to_string(), 60),
    };
    if compact.is_empty() {
        tool_name.to_string()
    } else {
        format!("{tool_name} ({compact})")
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let head: String = it.by_ref().take(max).collect();
    if it.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Best-effort secret scrub for audit digests/summaries.
///
/// Case-insensitive substring match on key names: `password`, `secret`,
/// `token`, `api_key`/`apikey`, `authorization`/`auth`, `bearer`, `key`,
/// `credential`, `private`, `passwd`. Values under matching keys are replaced
/// with `[redacted]` recursively (objects and arrays).
///
/// Best-effort only: a secret smuggled under a non-matching key (e.g.
/// `data`, `note`) is still hashed/summarized. HITL confirmation — not this
/// scrub — is the authoritative control for secret-bearing calls.
fn redact_secrets(mut v: Value) -> Value {
    if let Value::Object(map) = &mut v {
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            let lower = k.to_ascii_lowercase();
            if lower.contains("password")
                || lower.contains("secret")
                || lower.contains("token")
                || lower.contains("api_key")
                || lower.contains("apikey")
                || lower.contains("authorization")
                || lower.contains("auth")
                || lower.contains("bearer")
                || lower.contains("credential")
                || lower.contains("private")
                || lower.contains("passwd")
                || lower.contains("key")
            {
                map.insert(k, Value::String("[redacted]".into()));
            } else if let Some(child) = map.get_mut(&k) {
                *child = redact_secrets(child.clone());
            }
        }
    } else if let Value::Array(items) = &mut v {
        for item in items.iter_mut() {
            *item = redact_secrets(item.clone());
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_password_in_digest_summary() {
        let args = json!({ "password": "hunter2", "note": "hi" });
        let summary = args_summary("t", &args);
        assert!(!summary.contains("hunter2"));
        assert!(summary.contains("[redacted]") || summary.contains("note"));
    }

    #[test]
    fn memory_sink_records() {
        let sink = MemoryAuditSink::new();
        sink.write(&AuditEvent {
            ts_ms: 1,
            session_id: None,
            turn_id: None,
            tool: "get_time".into(),
            risk: "safe".into(),
            decision: "allow".into(),
            args_digest: "abc".into(),
            ok: Some(true),
            duration_ms: Some(1),
            error_kind: None,
            truncated: false,
            bytes: 4,
        });
        assert_eq!(sink.len(), 1);
    }

    #[test]
    fn jsonl_writes_line() {
        let dir = std::env::temp_dir().join(format!("boris-audit-test-{}", now_ms()));
        let path = dir.join("tool_calls.jsonl");
        let sink = JsonlAuditSink::new(&path);
        sink.write(&AuditEvent {
            ts_ms: now_ms(),
            session_id: Some("s-1".into()),
            turn_id: None,
            tool: "get_time".into(),
            risk: "safe".into(),
            decision: "allow".into(),
            args_digest: "x".into(),
            ok: Some(true),
            duration_ms: Some(2),
            error_kind: None,
            truncated: true,
            bytes: 120,
        });
        let content = fs::read_to_string(&path).expect("read audit");
        assert!(content.contains("get_time"));
        // Truncation fields must serialize (additive, no break for old readers).
        assert!(content.contains("truncated"));
        assert!(content.contains("bytes"));
        assert_eq!(sink.failed_count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redacts_expanded_secret_keys() {
        for key in [
            "password",
            "secret",
            "token",
            "api_key",
            "apikey",
            "authorization",
            "Authorization",
            "auth_token",
            "bearer",
            "Bearer",
            "private_key",
            "credential",
            "credentials",
            "passwd",
            "openai_key",
            "APIKEY",
        ] {
            let args = json!({ key: "hunter2-hunter2", "note": "hi" });
            let summary = args_summary("t", &args);
            assert!(
                !summary.contains("hunter2"),
                "key `{key}` must be redacted, got: {summary}"
            );
            let digest_a = args_digest(&args);
            let digest_b = args_digest(&json!({ key: "different-secret", "note": "hi" }));
            assert_eq!(
                digest_a, digest_b,
                "redacted digests must not leak secret bytes for key `{key}`"
            );
        }
        // Nested objects redact too.
        let nested = json!({ "cfg": { "bearer": "abc123" } });
        assert!(!args_summary("t", &nested).contains("abc123"));
    }

    #[test]
    fn audit_event_truncation_defaults_to_false_zero() {
        // Old paths construct with explicit false/0; serde defaults keep
        // backward compat for readers missing the fields.
        let v: AuditEvent = serde_json::from_str(
            r#"{"ts_ms":1,"tool":"t","risk":"safe","decision":"allow","args_digest":"x"}"#,
        )
        .expect("deserialize without new fields");
        assert!(!v.truncated);
        assert_eq!(v.bytes, 0);
        let ser = serde_json::to_value(&AuditEvent {
            ts_ms: 1,
            session_id: None,
            turn_id: None,
            tool: "t".into(),
            risk: "safe".into(),
            decision: "allow".into(),
            args_digest: "x".into(),
            ok: None,
            duration_ms: None,
            error_kind: None,
            truncated: false,
            bytes: 0,
        })
        .unwrap();
        assert_eq!(ser["truncated"], serde_json::json!(false));
        assert_eq!(ser["bytes"], serde_json::json!(0));
    }

    #[test]
    fn jsonl_failure_counter_increments_on_bad_path() {
        // Point the sink at a path whose parent is a file, so mkdir/open must
        // fail without panicking.
        let dir = std::env::temp_dir().join(format!("boris-audit-fail-{}", now_ms()));
        let _ = fs::create_dir_all(&dir);
        let blocker = dir.join("blocker");
        fs::write(&blocker, "i am a file, not a dir").unwrap();
        let bad = blocker.join("tool_calls.jsonl");
        let sink = JsonlAuditSink::new(&bad);
        sink.write(&AuditEvent {
            ts_ms: 1,
            session_id: None,
            turn_id: None,
            tool: "t".into(),
            risk: "safe".into(),
            decision: "allow".into(),
            args_digest: "x".into(),
            ok: None,
            duration_ms: None,
            error_kind: None,
            truncated: false,
            bytes: 0,
        });
        assert!(
            sink.failed_count() >= 1,
            "failed writes must be counted, got {}",
            sink.failed_count()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
