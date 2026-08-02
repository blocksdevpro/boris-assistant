//! Append-only audit log for tool invocations.

use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use tracing::warn;

/// One structured audit line (JSONL).
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub ts_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub tool: String,
    pub risk: String,
    /// allow | deny | confirm | confirmed | rejected | timeout | error
    pub decision: String,
    pub args_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

/// Sink for audit events (file, null, or test capture).
pub trait AuditSink: Send {
    fn write(&self, event: &AuditEvent);
}

/// Discards all events.
#[derive(Debug, Default)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn write(&self, _event: &AuditEvent) {}
}

/// Append JSONL lines to a file. Soft-fails (warns) on I/O errors.
pub struct JsonlAuditSink {
    path: PathBuf,
    lock: Mutex<()>,
}

impl JsonlAuditSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AuditSink for JsonlAuditSink {
    fn write(&self, event: &AuditEvent) {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = fs::create_dir_all(parent) {
                    warn!(error = %e, path = %self.path.display(), "audit mkdir failed");
                    return;
                }
            }
        }
        let line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "audit serialize failed");
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
                return;
            }
        };
        if let Err(e) = writeln!(file, "{line}") {
            warn!(error = %e, "audit write failed");
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
                || lower == "authorization"
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
        });
        let content = fs::read_to_string(&path).expect("read audit");
        assert!(content.contains("get_time"));
        let _ = fs::remove_dir_all(&dir);
    }
}
