//! Markdown long-term memory (Grok-lite): curated MEMORY.md + session logs.
//!
//! ```text
//! ~/.boris/memory/
//!   MEMORY.md                 # curated durable knowledge
//!   sessions/
//!     YYYY-MM-DD-{sid8}.md    # append-only voice turn logs
//!   notes.jsonl / profile.json  # (existing short-term tools)
//! ```

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;

/// One search hit for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHit {
    pub path: String,
    pub score: u32,
    pub snippet: String,
}

/// File-backed markdown memory under a host-supplied root (`~/.boris/memory`).
#[derive(Debug)]
pub struct LongTermMemory {
    root: PathBuf,
    /// Active session log file (created on first append).
    session_file: Mutex<Option<PathBuf>>,
    session_id: Mutex<Option<String>>,
}

impl LongTermMemory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            session_file: Mutex::new(None),
            session_id: Mutex::new(None),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn memory_md_path(&self) -> PathBuf {
        self.root.join("MEMORY.md")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Ensure directory layout exists (soft-fail callers wrap this).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(self.sessions_dir())?;
        let mem = self.memory_md_path();
        if !mem.is_file() {
            if let Some(parent) = mem.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(
                &mem,
                "# MEMORY\n\nCurated facts and decisions Boris should keep across sessions.\n\
                 Edit freely. Use memory_search / memory_get from the agent.\n",
            )?;
        }
        Ok(())
    }

    /// Bind / rebind the session id used for the daily log filename.
    pub fn set_session_id(&self, id: Option<String>) {
        if let Ok(mut g) = self.session_id.lock() {
            *g = id;
        }
        if let Ok(mut g) = self.session_file.lock() {
            *g = None; // force re-resolve on next append
        }
    }

    fn resolve_session_path(&self) -> PathBuf {
        let sid = self
            .session_id
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(|| "local".into());
        let sid8: String = sid.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
        let sid8 = if sid8.is_empty() {
            "session".into()
        } else {
            sid8
        };
        let date = Local::now().format("%Y-%m-%d");
        self.sessions_dir().join(format!("{date}-{sid8}.md"))
    }

    /// Append one user/assistant exchange to today's session log.
    pub fn append_turn(&self, user: &str, assistant: &str) -> Result<(), String> {
        let user = user.trim();
        let assistant = assistant.trim();
        if user.is_empty() && assistant.is_empty() {
            return Ok(());
        }
        self.ensure_dirs().map_err(|e| format!("memory dirs: {e}"))?;

        let path = {
            let mut slot = self
                .session_file
                .lock()
                .map_err(|_| "memory session lock poisoned".to_string())?;
            if slot.is_none() {
                *slot = Some(self.resolve_session_path());
            }
            slot.clone().unwrap()
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create sessions: {e}"))?;
        }

        let ts = Local::now().format("%H:%M:%S");
        let mut block = format!("\n## {ts}\n");
        if !user.is_empty() {
            block.push_str(&format!("**User:** {user}\n"));
        }
        if !assistant.is_empty() {
            block.push_str(&format!("**Boris:** {assistant}\n"));
        }

        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("open session log {}: {e}", path.display()))?;
        if f.metadata().map(|m| m.len()).unwrap_or(1) == 0 {
            let header = format!(
                "# Session log {}\n\n",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("session")
            );
            f.write_all(header.as_bytes())
                .map_err(|e| format!("write header: {e}"))?;
        }
        f.write_all(block.as_bytes())
            .map_err(|e| format!("append session: {e}"))?;
        Ok(())
    }

    /// Keyword search over MEMORY.md + recent session logs.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<MemoryHit>, String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Err("query is empty".into());
        }
        let max_results = max_results.clamp(1, 20);
        let mut hits: Vec<MemoryHit> = Vec::new();

        // Curated memory first.
        let mem_path = self.memory_md_path();
        if mem_path.is_file() {
            score_file(&mem_path, &self.root, &q, &mut hits)?;
        }

        // Session logs (newest first by name — date prefix sorts well).
        let sess = self.sessions_dir();
        if sess.is_dir() {
            let mut files: Vec<PathBuf> = fs::read_dir(&sess)
                .map_err(|e| format!("read sessions: {e}"))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("md"))
                        .unwrap_or(false)
                })
                .collect();
            files.sort();
            files.reverse();
            for path in files.into_iter().take(40) {
                score_file(&path, &self.root, &q, &mut hits)?;
            }
        }

        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        hits.truncate(max_results);
        Ok(hits)
    }

    /// Read a memory file relative to the memory root (path traversal safe).
    pub fn get(&self, rel_path: &str, max_chars: usize) -> Result<String, String> {
        let rel = rel_path.trim().trim_start_matches(['/', '\\']);
        if rel.is_empty() {
            return Err("path is empty".into());
        }
        // Reject `..` components before joining.
        use std::path::Component;
        for c in Path::new(rel).components() {
            match c {
                Component::Normal(_) => {}
                Component::CurDir => {}
                _ => return Err("path escapes memory root".into()),
            }
        }
        let candidate = self.root.join(rel);
        if !candidate.is_file() {
            return Err(format!("not a file: {rel}"));
        }
        // Extra belt: if both exist as canonical paths, enforce prefix.
        if let (Ok(root), Ok(file)) = (fs::canonicalize(&self.root), fs::canonicalize(&candidate)) {
            if !file.starts_with(&root) {
                return Err("path escapes memory root".into());
            }
        }
        let mut raw = String::new();
        fs::File::open(&candidate)
            .and_then(|mut f| f.read_to_string(&mut raw))
            .map_err(|e| format!("read {rel}: {e}"))?;
        let max_chars = max_chars.clamp(200, 40_000);
        if raw.chars().count() <= max_chars {
            return Ok(raw);
        }
        let head: String = raw.chars().take(max_chars).collect();
        Ok(format!("{head}\n…[truncated]"))
    }

    /// Short system-prompt hint when memory is enabled.
    pub fn prompt_hint(&self) -> String {
        format!(
            "<memory>\n\
             Cross-session markdown memory is enabled under {}.\n\
             Use memory_search to find past facts/decisions; memory_get to read a file path from search hits.\n\
             Prefer MEMORY.md for durable knowledge; session logs are raw turn history.\n\
             </memory>",
            self.root.display()
        )
    }
}

fn score_file(
    path: &Path,
    root: &Path,
    query_lc: &str,
    hits: &mut Vec<MemoryHit>,
) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let lower = raw.to_ascii_lowercase();
    let doc_len = lower.split_whitespace().count().max(1) as u32;
    let mut score = 0u32;
    // BM25-ish: term frequency with length normalization + multi-token boost.
    let tokens: Vec<&str> = query_lc
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .collect();
    if tokens.is_empty() {
        return Ok(());
    }
    let mut matched = 0u32;
    for tok in &tokens {
        let c = lower.matches(tok).count() as u32;
        if c == 0 {
            continue;
        }
        matched += 1;
        // tf saturation
        let tf = (c * 100) / (c + 2);
        let idf = 3 + tok.len() as u32; // longer rare-ish terms score higher
        score = score.saturating_add(tf.saturating_mul(idf));
    }
    if matched == 0 {
        return Ok(());
    }
    // Prefer docs that match more query tokens.
    score = score.saturating_add(matched.saturating_mul(40));
    // Soft length penalty.
    score = score.saturating_mul(100) / (80 + doc_len.min(200));
    // Recency boost for session logs (filename starts with date).
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if rel.contains("sessions/") {
        score = score.saturating_add(15);
    }
    if rel.ends_with("MEMORY.md") {
        score = score.saturating_add(25); // curated knowledge wins ties
    }
    let snippet = best_snippet(&raw, query_lc, 220);
    hits.push(MemoryHit {
        path: rel,
        score,
        snippet,
    });
    Ok(())
}

fn best_snippet(raw: &str, query_lc: &str, max_len: usize) -> String {
    let lower = raw.to_ascii_lowercase();
    let mut pos = None;
    for tok in query_lc.split_whitespace() {
        if tok.len() < 2 {
            continue;
        }
        if let Some(p) = lower.find(tok) {
            pos = Some(p);
            break;
        }
    }
    let start = pos.unwrap_or(0).saturating_sub(40);
    let slice: String = raw
        .chars()
        .skip(start)
        .take(max_len)
        .collect::<String>()
        .replace('\n', " ");
    let mut s = slice.trim().to_string();
    if start > 0 {
        s = format!("…{s}");
    }
    if raw.chars().count() > start + max_len {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_root() -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("boris-ltm-{n}"))
    }

    #[test]
    fn append_search_get_roundtrip() {
        let root = tmp_root();
        let mem = LongTermMemory::new(&root);
        mem.ensure_dirs().unwrap();
        mem.set_session_id(Some("abc12345sess".into()));
        mem.append_turn(
            "I love dark mode for coding",
            "Got it bro, dark mode forever.",
        )
        .unwrap();
        fs::write(
            mem.memory_md_path(),
            "# MEMORY\n\nUser prefers dark mode editors.\n",
        )
        .unwrap();

        let hits = mem.search("dark mode", 5).unwrap();
        assert!(!hits.is_empty(), "expected hits, got {hits:?}");
        assert!(hits.iter().any(|h| h.path.contains("MEMORY") || h.path.contains("sessions")));

        let body = mem.get("MEMORY.md", 2000).unwrap();
        assert!(body.contains("dark mode"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn get_rejects_escape() {
        let root = tmp_root();
        let mem = LongTermMemory::new(&root);
        mem.ensure_dirs().unwrap();
        let err = mem.get("../secrets.txt", 100).unwrap_err();
        assert!(
            err.contains("escape") || err.contains("not a file"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
