//! Spill full tool outputs to disk when truncation cuts them.
//!
//! Voice turns keep only ~12k chars in context ([`crate::tool::truncate_tool_result`]),
//! but follow-ups like "tell me more" should not re-run `bash`/`grep`.
//! [`ToolOutputStore`] saves the pre-truncate text under
//! `{store_dir}/tool_{ms}-{tool}-{seq}.log` and [`GetToolOutputTool`] reads it
//! back with offset/limit. Files older than [`GC_DAYS`] are best-effort GC'd
//! on save (hourly marker file, like OpenCode's 7-day truncation GC).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Max full output kept per file (pre-truncate spill). Above OpenCode's 50KB
/// display budget but below unbounded flood: bash already caps capture at 120KB.
pub const MAX_STORED_CHARS: usize = 256 * 1024;
/// GC age for spilled outputs (days).
pub const GC_DAYS: u64 = 7;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// File-backed store for truncated tool observations.
#[derive(Debug, Clone)]
pub struct ToolOutputStore {
    dir: PathBuf,
}

impl ToolOutputStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Save `full_text` (pre-truncate) and return the file path.
    ///
    /// Best-effort: creates `dir`, truncates to [`MAX_STORED_CHARS`] chars,
    /// runs hourly GC. Returns `Err` message on I/O failure (caller should
    /// ignore spill failures — the truncated observation is still returned).
    pub fn save(&self, tool_name: &str, full_text: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("output store mkdir {}: {e}", self.dir.display()))?;
        self.gc_best_effort();

        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let safe_tool: String = tool_name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(32)
            .collect();
        let name = format!("tool_{ms}-{safe_tool}-{seq}.log");
        let path = self.dir.join(name);

        let text: String = if full_text.chars().count() > MAX_STORED_CHARS {
            let mut s: String = full_text.chars().take(MAX_STORED_CHARS).collect();
            s.push_str("\n…[stored output truncated]");
            s
        } else {
            full_text.to_string()
        };
        std::fs::write(&path, text.as_bytes())
            .map_err(|e| format!("output store write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// Read a spilled file with char offset/limit (1-based offset, like file_read).
    pub fn read(&self, path: &Path, offset: usize, limit: usize) -> Result<String, String> {
        let canonical_store = self.dir.canonicalize().unwrap_or_else(|_| self.dir.clone());
        let canonical_file = path
            .canonicalize()
            .map_err(|e| format!("output file not found: {e}"))?;
        if !canonical_file.starts_with(&canonical_store) {
            return Err("output path is outside the output store".into());
        }
        let content = std::fs::read_to_string(&canonical_file)
            .map_err(|e| format!("read output file: {e}"))?;
        let total = content.chars().count();
        let offset = offset.max(1);
        let limit = limit.clamp(1, MAX_STORED_CHARS);
        let start = (offset - 1).min(total);
        let end = (start + limit).min(total);
        let slice: String = content.chars().skip(start).take(end - start).collect();
        let mut out = slice;
        if end < total {
            out.push_str(&format!(
                "\n[Showing chars {start}-{end} of {total}. Use offset={} to continue.]",
                end + 1
            ));
        }
        Ok(out)
    }

    fn gc_best_effort(&self) {
        // Hourly marker so concurrent turns don't all scan.
        let marker = self.dir.join(".gc-marker");
        let now = SystemTime::now();
        if let Ok(meta) = std::fs::metadata(&marker) {
            if let Ok(mtime) = meta.modified() {
                if now
                    .duration_since(mtime)
                    .map(|d| d.as_secs() < 3600)
                    .unwrap_or(false)
                {
                    return;
                }
            }
        }
        let _ = std::fs::write(&marker, b"1");
        let cutoff = now
            .checked_sub(std::time::Duration::from_secs(GC_DAYS * 24 * 3600))
            .unwrap_or(UNIX_EPOCH);
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.file_name().and_then(|s| s.to_str()) == Some(".gc-marker") {
                continue;
            }
            let is_old = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .map(|t| t < cutoff)
                .unwrap_or(false);
            if is_old {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Append a retrieval hint to a truncated observation.
pub fn output_store_hint(path: &Path) -> String {
    format!(
        "\n[Full output saved to {} — use get_tool_output to read more.]",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("boris-outstore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ToolOutputStore::new(&dir);
        let full = "line1\nline2\nline3\n";
        let path = store.save("bash", full).unwrap();
        assert!(path.starts_with(&dir));
        let back = store.read(&path, 1, 1000).unwrap();
        assert!(back.contains("line1"));
        // Paging.
        let page = store.read(&path, 1, 5).unwrap();
        assert!(page.contains("Showing chars"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_rejects_escape() {
        let dir = std::env::temp_dir().join(format!("boris-outstore-esc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let store = ToolOutputStore::new(&dir);
        let outside = std::env::temp_dir().join("boris-outside.log");
        std::fs::write(&outside, "secret").unwrap();
        let err = store.read(&outside, 1, 100).unwrap_err();
        assert!(err.contains("outside"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&outside);
    }
}
