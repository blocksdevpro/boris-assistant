//! Append-only notes JSONL store (pure list/search over in-memory records).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Append-only notes store backed by a single JSONL file.
#[derive(Debug, Clone)]
pub struct NotesStore {
    path: PathBuf,
}

/// One line in the notes file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct NoteRecord {
    pub ts_ms: u64,
    pub note: String,
}

impl NotesStore {
    /// Create a store for the given file path (does not create the file yet).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Path to the JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one note as a JSONL line. Creates parent directories if needed.
    pub fn append(&self, note: &str) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("create notes parent dir {}: {e}", parent.display()))?;
            }
        }

        let record = NoteRecord {
            ts_ms: now_ms(),
            note: note.to_string(),
        };
        let line = serde_json::to_string(&record).map_err(|e| format!("serialize note: {e}"))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("open notes {}: {e}", self.path.display()))?;

        writeln!(file, "{line}")
            .map_err(|e| format!("write notes {}: {e}", self.path.display()))?;
        file.flush()
            .map_err(|e| format!("flush notes {}: {e}", self.path.display()))?;
        Ok(())
    }

    /// Most recent notes (newest first), up to `limit`.
    pub fn list_recent(&self, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let all = self.read_all()?;
        Ok(list_recent_notes(&all, limit))
    }

    /// Case-insensitive substring search; newest matches first, up to `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<String>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let all = self.read_all()?;
        Ok(search_notes(&all, query, limit))
    }

    /// Read every well-formed record in file order. Missing file → empty.
    /// Blank lines skipped; malformed lines skipped (warn via tracing).
    fn read_all(&self) -> Result<Vec<NoteRecord>, String> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|e| format!("open notes {}: {e}", self.path.display()))?;

        let reader = BufReader::new(file);
        let mut out = Vec::new();

        for (idx, line_res) in reader.lines().enumerate() {
            let line = line_res.map_err(|e| format!("read notes {}: {e}", self.path.display()))?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<NoteRecord>(trimmed) {
                Ok(rec) => out.push(rec),
                Err(err) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        line = idx + 1,
                        error = %err,
                        "skipping malformed notes line"
                    );
                }
            }
        }

        Ok(out)
    }
}

/// Newest-first note texts, up to `limit` (pure).
pub(super) fn list_recent_notes(records: &[NoteRecord], limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let take = limit.min(records.len());
    records
        .iter()
        .rev()
        .take(take)
        .map(|r| r.note.clone())
        .collect()
}

/// Case-insensitive contains; newest matches first (pure).
pub(super) fn search_notes(records: &[NoteRecord], query: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for rec in records.iter().rev() {
        if rec.note.to_lowercase().contains(&q) {
            out.push(rec.note.clone());
            if out.len() >= limit {
                break;
            }
        }
    }
    out
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

    fn rec(ts: u64, note: &str) -> NoteRecord {
        NoteRecord {
            ts_ms: ts,
            note: note.into(),
        }
    }

    #[test]
    fn list_recent_pure_newest_first() {
        let all = vec![rec(1, "one"), rec(2, "two"), rec(3, "three")];
        assert_eq!(
            list_recent_notes(&all, 2),
            vec!["three".to_string(), "two".to_string()]
        );
        assert!(list_recent_notes(&all, 0).is_empty());
    }

    #[test]
    fn search_pure_case_insensitive() {
        let all = vec![rec(1, "Buy Milk"), rec(2, "walk dog"), rec(3, "MILK tea")];
        let hits = search_notes(&all, "milk", 10);
        assert_eq!(hits, vec!["MILK tea".to_string(), "Buy Milk".to_string()]);
    }
}
