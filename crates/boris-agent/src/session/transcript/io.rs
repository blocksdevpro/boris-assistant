//! Filesystem I/O for chat_history.jsonl and events.jsonl.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::time::now_ms;
use super::TranscriptRecord;

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
        .map_err(|e| format!("open chat_history {}: {e}", path.display()))?;

    let line = record.to_json_line()?;
    writeln!(file, "{line}").map_err(|e| format!("write chat_history {}: {e}", path.display()))?;
    file.flush()
        .map_err(|e| format!("flush chat_history {}: {e}", path.display()))?;
    Ok(())
}

/// Append many records (one JSONL line each).
pub fn append_records(path: &Path, records: &[TranscriptRecord]) -> Result<(), String> {
    for r in records {
        append_record(path, r)?;
    }
    Ok(())
}

/// Convenience: append a user line then an assistant line.
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
            ts_ms: now_ms().max(ts_ms),
            role: "assistant".into(),
            content: Value::String(assistant.to_string()),
        },
    )?;
    Ok(())
}

/// Rewrite the entire chat_history file from records (used when context prunes).
pub fn write_all(path: &Path, records: &[TranscriptRecord]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create transcript parent dir {}: {e}", parent.display()))?;
        }
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| format!("open chat_history tmp {}: {e}", tmp.display()))?;
        for r in records {
            let line = r.to_json_line()?;
            writeln!(file, "{line}")
                .map_err(|e| format!("write chat_history tmp {}: {e}", tmp.display()))?;
        }
        file.flush()
            .map_err(|e| format!("flush chat_history tmp {}: {e}", tmp.display()))?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Cross-device fallback: copy contents.
            let data =
                fs::read(&tmp).map_err(|e2| format!("read tmp after rename fail ({e}): {e2}"))?;
            fs::write(path, data).map_err(|e2| format!("write chat_history fallback: {e2}"))?;
            let _ = fs::remove_file(&tmp);
            Ok(())
        }
    }
}

/// Append a lightweight lifecycle event (Grok-like `events.jsonl`).
pub fn append_event(path: &Path, event_type: &str, extra: Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create events parent {}: {e}", parent.display()))?;
        }
    }
    let mut obj = match extra {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    obj.insert(
        "ts".into(),
        json!(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
    );
    obj.insert("type".into(), json!(event_type));
    obj.insert("schema_version".into(), json!("1.0"));
    let line =
        serde_json::to_string(&Value::Object(obj)).map_err(|e| format!("serialize event: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open events {}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("write events: {e}"))?;
    Ok(())
}

/// Read every well-formed record. Missing file → empty vec.
pub fn read_all(path: &Path) -> Result<Vec<TranscriptRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("open chat_history {}: {e}", path.display()))?;

    let reader = BufReader::new(file);
    let mut out = Vec::new();

    for (idx, line_res) in reader.lines().enumerate() {
        let line = line_res.map_err(|e| format!("read chat_history {}: {e}", path.display()))?;
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
                tracing::warn!(
                    path = %path.display(),
                    line = idx + 1,
                    error = %err,
                    "skipping malformed chat_history line"
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
        dir.join("chat_history.jsonl")
    }

    #[test]
    fn append_and_read_roundtrip_grok_shape() {
        let path = temp_transcript_path("roundtrip");
        let _ = fs::remove_file(&path);

        let rec = TranscriptRecord {
            ts_ms: 1_700_000_000_000,
            role: "user".into(),
            content: Value::String("hello".into()),
        };
        append_record(&path, &rec).expect("append");

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"type\":\"user\""));
        assert!(raw.contains("\"text\":\"hello\""));
        assert!(raw.contains("\"ts\":"));

        let all = read_all(&path).expect("read");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].role, "user");
        assert_eq!(all[0].content, Value::String("hello".into()));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn tool_call_and_result_roundtrip_grok_shape() {
        let path = temp_transcript_path("tools");
        let _ = fs::remove_file(&path);

        let assistant_tc = TranscriptRecord {
            ts_ms: 1,
            role: "assistant".into(),
            content: json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"a.txt\"}"
                    }
                }]
            }),
        };
        let tool = TranscriptRecord {
            ts_ms: 2,
            role: "tool".into(),
            content: json!({ "tool_call_id": "call_1", "content": "file body" }),
        };
        let final_a = TranscriptRecord {
            ts_ms: 3,
            role: "assistant".into(),
            content: Value::String("done".into()),
        };
        append_records(&path, &[assistant_tc, tool, final_a]).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"type\":\"tool_result\""));
        assert!(raw.contains("\"tool_call_id\":\"call_1\""));
        // Flat Grok tool_calls on disk (no nested OpenAI `function` object).
        assert!(raw.contains("\"name\":\"read_file\""));
        assert!(
            !raw.contains("\"function\""),
            "disk tool_calls should be flat Grok shape: {raw}"
        );

        let all = read_all(&path).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].role, "assistant");
        assert!(all[0].content.get("tool_calls").is_some());
        // Reloaded as OpenAI-style for the agent.
        let calls = all[0]
            .content
            .get("tool_calls")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(calls[0]["function"]["name"], "read_file");
        assert_eq!(all[1].role, "tool");
        assert_eq!(all[1].content["tool_call_id"], "call_1");
        assert_eq!(all[1].content["content"], "file body");
        assert_eq!(all[2].content, Value::String("done".into()));

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn system_message_plain_string_on_disk() {
        let path = temp_transcript_path("system");
        let _ = fs::remove_file(&path);
        append_record(
            &path,
            &TranscriptRecord {
                ts_ms: 1,
                role: "system".into(),
                content: Value::String("You are Boris.".into()),
            },
        )
        .unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"type\":\"system\""));
        assert!(raw.contains("You are Boris."));
        let all = read_all(&path).unwrap();
        assert_eq!(all[0].role, "system");
        assert_eq!(all[0].content, Value::String("You are Boris.".into()));
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
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

        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn write_all_replaces_file() {
        let path = temp_transcript_path("rewrite");
        append_exchange(&path, "a", "b").unwrap();
        write_all(
            &path,
            &[TranscriptRecord {
                ts_ms: 1,
                role: "user".into(),
                content: Value::String("only".into()),
            }],
        )
        .unwrap();
        let all = read_all(&path).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].content, Value::String("only".into()));
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn append_event_writes_type_and_ts() {
        let base = temp_transcript_path("events");
        let path = base.parent().unwrap().join("events.jsonl");
        let _ = fs::remove_file(&path);
        append_event(&path, "turn_started", json!({"session_id": "s-1"})).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("turn_started"));
        assert!(raw.contains("session_id"));
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
    }
}
