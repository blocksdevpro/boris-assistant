//! Append-only JSONL session transcript.
//!
//! Each line is one JSON object:
//! `{"ts_ms":123,"role":"user|assistant|system|tool","content": <Value>}`

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// One line of a session transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptRecord {
    pub ts_ms: u64,
    pub role: String,
    pub content: Value,
}

impl TranscriptRecord {
    /// Build a record with the current wall-clock time in milliseconds.
    pub fn now(role: impl Into<String>, content: Value) -> Self {
        Self {
            ts_ms: now_ms(),
            role: role.into(),
            content,
        }
    }

    fn to_json_line(&self) -> Result<String, String> {
        let v = json!({
            "ts_ms": self.ts_ms,
            "role": self.role,
            "content": self.content,
        });
        serde_json::to_string(&v).map_err(|e| format!("serialize transcript record: {e}"))
    }

    fn from_json_value(v: Value) -> Result<Self, String> {
        let ts_ms = v
            .get("ts_ms")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| "missing or invalid ts_ms".to_string())?;
        let role = v
            .get("role")
            .and_then(|x| x.as_str())
            .ok_or_else(|| "missing or invalid role".to_string())?
            .to_string();
        let content = v
            .get("content")
            .cloned()
            .ok_or_else(|| "missing content".to_string())?;
        Ok(Self {
            ts_ms,
            role,
            content,
        })
    }
}

/// Append a single record as one JSON line. Creates parent directories if needed.
pub fn append_record(path: &Path, record: &TranscriptRecord) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create transcript parent dir {}: {e}", parent.display()))?;
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open transcript {}: {e}", path.display()))?;

    let line = record.to_json_line()?;
    writeln!(file, "{line}").map_err(|e| format!("write transcript {}: {e}", path.display()))?;
    file.flush()
        .map_err(|e| format!("flush transcript {}: {e}", path.display()))?;
    Ok(())
}

/// Convenience: append a user line then an assistant line (same timestamp).
pub fn append_exchange(path: &Path, user: &str, assistant: &str) -> Result<(), String> {
    let ts_ms = now_ms();
    append_record(
        path,
        &TranscriptRecord {
            ts_ms,
            role: "user".into(),
            content: Value::String(user.to_string()),
        },
    )?;
    append_record(
        path,
        &TranscriptRecord {
            ts_ms,
            role: "assistant".into(),
            content: Value::String(assistant.to_string()),
        },
    )?;
    Ok(())
}

/// Read every well-formed record. Missing file → empty vec.
/// Blank lines are skipped; malformed lines are skipped (with a tracing warn when available).
pub fn read_all(path: &Path) -> Result<Vec<TranscriptRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("open transcript {}: {e}", path.display()))?;

    let reader = BufReader::new(file);
    let mut out = Vec::new();

    for (idx, line_res) in reader.lines().enumerate() {
        let line = line_res.map_err(|e| format!("read transcript {}: {e}", path.display()))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(trimmed)
            .map_err(|e| e.to_string())
            .and_then(TranscriptRecord::from_json_value)
        {
            Ok(rec) => out.push(rec),
            Err(err) => {
                // Line numbers are 1-based for human logs.
                tracing::warn!(
                    path = %path.display(),
                    line = idx + 1,
                    error = %err,
                    "skipping malformed transcript line"
                );
            }
        }
    }

    Ok(out)
}

/// Dump records as OpenAI-style `{role, content}` message objects for LLM context.
pub fn records_to_openai_messages(records: &[TranscriptRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|r| {
            json!({
                "role": r.role,
                "content": r.content,
            })
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_transcript_path(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("boris-transcript-{nanos}-{n}-{label}"));
        let _ = fs::create_dir_all(&dir);
        dir.join("transcript.jsonl")
    }

    #[test]
    fn append_and_read_roundtrip() {
        let path = temp_transcript_path("roundtrip");
        let _ = fs::remove_file(&path);

        let rec = TranscriptRecord {
            ts_ms: 42,
            role: "user".into(),
            content: Value::String("hello".into()),
        };
        append_record(&path, &rec).expect("append");
        let all = read_all(&path).expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], rec);

        let _ = fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn append_creates_parent_dirs() {
        let base = temp_transcript_path("parents");
        let path = base
            .parent()
            .unwrap()
            .join("nested")
            .join("deep")
            .join("transcript.jsonl");
        let _ = fs::remove_file(&path);

        append_record(
            &path,
            &TranscriptRecord {
                ts_ms: 1,
                role: "system".into(),
                content: json!({"note": true}),
            },
        )
        .expect("append with parents");

        assert!(path.exists());
        let all = read_all(&path).expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].role, "system");
        assert_eq!(all[0].content, json!({"note": true}));

        if let Some(root) = base.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn append_exchange_writes_two_lines() {
        let path = temp_transcript_path("exchange");
        let _ = fs::remove_file(&path);

        append_exchange(&path, "hi", "hello there").expect("exchange");
        let all = read_all(&path).expect("read");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].role, "user");
        assert_eq!(all[0].content, Value::String("hi".into()));
        assert_eq!(all[1].role, "assistant");
        assert_eq!(all[1].content, Value::String("hello there".into()));
        assert_eq!(all[0].ts_ms, all[1].ts_ms);

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn read_missing_file_returns_empty() {
        let path = temp_transcript_path("missing");
        let _ = fs::remove_file(&path);
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
        // path itself does not exist
        let all = read_all(&path).expect("read missing");
        assert!(all.is_empty());
    }

    #[test]
    fn skips_blank_and_malformed_lines() {
        let path = temp_transcript_path("malformed");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &path,
            r#"
{"ts_ms":1,"role":"user","content":"ok"}

not-json
{"ts_ms":2,"role":"assistant","content":"fine"}
{"bad":true}
"#,
        )
        .unwrap();

        let all = read_all(&path).expect("read");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, Value::String("ok".into()));
        assert_eq!(all[1].content, Value::String("fine".into()));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn records_to_openai_messages_shape() {
        let records = vec![
            TranscriptRecord {
                ts_ms: 1,
                role: "user".into(),
                content: Value::String("q".into()),
            },
            TranscriptRecord {
                ts_ms: 2,
                role: "assistant".into(),
                content: json!({"text": "a"}),
            },
        ];
        let msgs = records_to_openai_messages(&records);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "q");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"], json!({"text": "a"}));
        // ts_ms is not part of the OpenAI dump
        assert!(msgs[0].get("ts_ms").is_none());
    }

    #[test]
    fn append_is_append_only() {
        let path = temp_transcript_path("append_only");
        let _ = fs::remove_file(&path);

        append_record(
            &path,
            &TranscriptRecord {
                ts_ms: 10,
                role: "user".into(),
                content: Value::String("first".into()),
            },
        )
        .unwrap();
        append_record(
            &path,
            &TranscriptRecord {
                ts_ms: 20,
                role: "assistant".into(),
                content: Value::String("second".into()),
            },
        )
        .unwrap();

        let all = read_all(&path).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content, Value::String("first".into()));
        assert_eq!(all[1].content, Value::String("second".into()));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}
