//! Markdown long-term memory: one global corpus + per-session logs.
//!
//! ```text
//! ~/.boris/memory/
//!   MEMORY.md                      # single global curated knowledge
//!   profile.json / notes.jsonl     # personal tools (not LTM search)
//!   desktop/
//!     MEMORY.md                    # workspace-scoped curated notes
//!
//! ~/.boris/sessions/desktop/{uuid}/
//!   memory.md                      # this chat's turn log (session-local)
//! ```
//!
//! Session turn logs used to live under `memory/desktop/sessions/YYYY-MM-DD-sid8.md`.
//! That tree is no longer written (clean break). Search still only reads:
//! global + workspace MEMORY.md and `{sessions_root}/{id}/memory.md`.
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | (this)  | [`LongTermMemory`] paths, I/O, search orchestration |
//! | [`score`] | pure scoring / snippets / path-safety |

mod score;

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;

use score::{is_safe_rel_path, score_file, score_file_as};

/// Workspace subdirectory for desktop voice (no project cwd).
const DESKTOP_WORKSPACE: &str = "desktop";

/// Filename for session-local turn logs inside a session directory.
pub const SESSION_MEMORY_FILE: &str = "memory.md";

/// Virtual path prefix for session logs in search hits / `memory_get`
/// (`session/{uuid}/memory.md`).
pub const SESSION_PATH_PREFIX: &str = "session/";

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
    /// Sessions workspace root (`~/.boris/sessions/desktop`) for cross-chat search.
    sessions_root: Option<PathBuf>,
    /// Active session log file (`{session_dir}/memory.md`).
    session_file: Mutex<Option<PathBuf>>,
    session_id: Mutex<Option<String>>,
}

impl LongTermMemory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            sessions_root: None,
            session_file: Mutex::new(None),
            session_id: Mutex::new(None),
        }
    }

    /// Point search at a sessions workspace (e.g. `~/.boris/sessions/desktop`).
    pub fn with_sessions_root(mut self, sessions_root: impl Into<PathBuf>) -> Self {
        self.sessions_root = Some(sessions_root.into());
        self
    }

    pub fn set_sessions_root(&mut self, sessions_root: Option<PathBuf>) {
        self.sessions_root = sessions_root;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sessions_root(&self) -> Option<&Path> {
        self.sessions_root.as_deref()
    }

    /// Global curated knowledge (`MEMORY.md` at memory root).
    pub fn memory_md_path(&self) -> PathBuf {
        self.root.join("MEMORY.md")
    }

    /// Workspace memory root (`desktop/` under the memory home).
    pub fn workspace_dir(&self) -> PathBuf {
        // If host already pointed us at a leaf workspace (ends with desktop), use as-is.
        if self
            .root
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == DESKTOP_WORKSPACE)
        {
            return self.root.clone();
        }
        self.root.join(DESKTOP_WORKSPACE)
    }

    /// Workspace-scoped MEMORY.md (project / desktop bucket).
    pub fn workspace_memory_md_path(&self) -> PathBuf {
        self.workspace_dir().join("MEMORY.md")
    }

    /// Session-local turn log path for a session directory.
    pub fn session_memory_path(session_dir: &Path) -> PathBuf {
        session_dir.join(SESSION_MEMORY_FILE)
    }

    /// Ensure **global** directory layout exists (not per-session dirs).
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        // Global MEMORY.md — Grok-style scaffold.
        let mem = self.memory_md_path();
        if !mem.is_file() {
            if let Some(parent) = mem.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(
                &mem,
                "# Global Memory\n\
                 \n\
                 > This file is automatically managed by Boris's memory system.\n\
                 > You can also edit it manually — changes are used on the next session.\n\
                 \n\
                 ## Preferences\n\
                 \n\
                 <!-- Add any cross-project preferences here -->\n",
            )?;
        }

        // Workspace MEMORY.md (single workspace bucket; no nested sessions/).
        let ws_mem = self.workspace_memory_md_path();
        if !ws_mem.is_file() {
            if let Some(parent) = ws_mem.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(
                &ws_mem,
                "# Project Memory — desktop\n\
                 \n\
                 > Auto-populated by Boris. Edit freely.\n\
                 > Workspace-scoped notes for the desktop voice assistant.\n",
            )?;
        }
        Ok(())
    }

    /// Bind / rebind the session id (for log headers only).
    pub fn set_session_id(&self, id: Option<String>) {
        if let Ok(mut g) = self.session_id.lock() {
            *g = id;
        }
    }

    /// Bind turn-log writes to a session directory (`…/memory.md`).
    ///
    /// `None` clears the binding — [`append_turn`] becomes a no-op until rebound.
    pub fn set_session_dir(&self, session_dir: Option<PathBuf>) {
        if let Ok(mut g) = self.session_file.lock() {
            *g = session_dir.map(|d| Self::session_memory_path(&d));
        }
    }

    /// Explicit log path (tests / advanced hosts).
    pub fn set_session_log_path(&self, path: Option<PathBuf>) {
        if let Ok(mut g) = self.session_file.lock() {
            *g = path;
        }
    }

    /// Append one user/assistant exchange to the **bound session** `memory.md`.
    ///
    /// No-ops (Ok) when no session dir is bound — avoids writing under global memory.
    pub fn append_turn(&self, user: &str, assistant: &str) -> Result<(), String> {
        let user = user.trim();
        let assistant = assistant.trim();
        if user.is_empty() && assistant.is_empty() {
            return Ok(());
        }

        let path = {
            let slot = self
                .session_file
                .lock()
                .map_err(|_| "memory session lock poisoned".to_string())?;
            match slot.as_ref() {
                Some(p) => p.clone(),
                None => return Ok(()), // unbound — clean break, no global fallback
            }
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create session dir: {e}"))?;
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
            let sid = self
                .session_id
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| "session".into());
            let header = format!("# Session memory — {sid}\n\n");
            f.write_all(header.as_bytes())
                .map_err(|e| format!("write header: {e}"))?;
        }
        f.write_all(block.as_bytes())
            .map_err(|e| format!("append session: {e}"))?;
        Ok(())
    }

    /// Keyword search over global MEMORY + workspace MEMORY + session `memory.md` files.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<MemoryHit>, String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return Err("query is empty".into());
        }
        let max_results = max_results.clamp(1, 20);
        let mut hits: Vec<MemoryHit> = Vec::new();

        // Curated memory first (global + workspace).
        for mem_path in [self.memory_md_path(), self.workspace_memory_md_path()] {
            if mem_path.is_file() {
                score_file(&mem_path, &self.root, &q, &mut hits)?;
            }
        }

        // Per-session logs under sessions_root/{uuid}/memory.md
        if let Some(sessions_root) = &self.sessions_root {
            if sessions_root.is_dir() {
                let entries = fs::read_dir(sessions_root)
                    .map_err(|e| format!("read sessions root: {e}"))?;
                let mut session_logs: Vec<(String, PathBuf)> = Vec::new();
                for entry in entries {
                    let entry = entry.map_err(|e| format!("read sessions entry: {e}"))?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Some(id) = entry.file_name().to_str().map(|s| s.to_string()) else {
                        continue;
                    };
                    if id == "current.json" || id.starts_with('.') {
                        continue;
                    }
                    let log = Self::session_memory_path(&path);
                    if log.is_file() {
                        session_logs.push((id, log));
                    }
                }
                // Stable order by session id (newest uuid-ish sort not guaranteed; score handles rank).
                session_logs.sort_by(|a, b| b.0.cmp(&a.0));
                for (id, log) in session_logs.into_iter().take(80) {
                    let display = format!("{SESSION_PATH_PREFIX}{id}/{SESSION_MEMORY_FILE}");
                    score_file_as(&log, display, &q, &mut hits, true, false)?;
                }
            }
        }

        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        hits.truncate(max_results);
        Ok(hits)
    }

    /// Read a memory file by virtual path (path traversal safe).
    ///
    /// - Global: `MEMORY.md`, `desktop/MEMORY.md`, …
    /// - Session: `session/{uuid}/memory.md`
    pub fn get(&self, rel_path: &str, max_chars: usize) -> Result<String, String> {
        let rel = rel_path.trim().trim_start_matches(['/', '\\']);
        if rel.is_empty() {
            return Err("path is empty".into());
        }

        let candidate = if let Some(rest) = rel.strip_prefix(SESSION_PATH_PREFIX) {
            self.resolve_session_rel(rest)?
        } else {
            if !is_safe_rel_path(rel) {
                return Err("path escapes memory root".into());
            }
            let candidate = self.root.join(rel);
            if !candidate.is_file() {
                return Err(format!("not a file: {rel}"));
            }
            if let (Ok(root), Ok(file)) =
                (fs::canonicalize(&self.root), fs::canonicalize(&candidate))
            {
                if !file.starts_with(&root) {
                    return Err("path escapes memory root".into());
                }
            }
            candidate
        };

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

    /// Resolve `session/{uuid}/memory.md` → absolute path under `sessions_root`.
    fn resolve_session_rel(&self, rest: &str) -> Result<PathBuf, String> {
        let rest = rest.trim().trim_start_matches(['/', '\\']);
        let Some(sessions_root) = &self.sessions_root else {
            return Err("session memory root not configured".into());
        };
        // Expect exactly `{session_id}/memory.md`
        let parts: Vec<&str> = rest
            .split(['/', '\\'])
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() != 2 || parts[1] != SESSION_MEMORY_FILE {
            return Err(format!(
                "session path must be session/{{id}}/{SESSION_MEMORY_FILE}"
            ));
        }
        let sid = parts[0];
        if sid == "." || sid == ".." || sid.contains("..") {
            return Err("path escapes session root".into());
        }
        // Single path segment only (no nested dirs).
        if sid.contains('/') || sid.contains('\\') {
            return Err("path escapes session root".into());
        }
        let candidate = sessions_root.join(sid).join(SESSION_MEMORY_FILE);
        if !candidate.is_file() {
            return Err(format!("not a file: {SESSION_PATH_PREFIX}{rest}"));
        }
        if let (Ok(root), Ok(file)) = (
            fs::canonicalize(sessions_root),
            fs::canonicalize(&candidate),
        ) {
            if !file.starts_with(&root) {
                return Err("path escapes session root".into());
            }
        }
        Ok(candidate)
    }

    /// Short system-prompt hint when memory is enabled.
    pub fn prompt_hint(&self) -> String {
        format!(
            "<memory>\n\
             Global curated memory: {}/MEMORY.md (plus workspace MEMORY.md).\n\
             Each chat keeps its own turn log at sessions/…/{{id}}/memory.md.\n\
             Use memory_search for past facts; memory_get with hit paths \
             (MEMORY.md or session/{{id}}/memory.md).\n\
             </memory>",
            self.root.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_root(label: &str) -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("boris-ltm-{label}-{n}"))
    }

    #[test]
    fn append_search_get_roundtrip_session_scoped() {
        let mem_root = tmp_root("mem");
        let sessions = tmp_root("sess");
        let session_id = "abc12345-sess-uuid";
        let session_dir = sessions.join(session_id);
        fs::create_dir_all(&session_dir).unwrap();

        let mem = LongTermMemory::new(&mem_root).with_sessions_root(&sessions);
        mem.ensure_dirs().unwrap();
        mem.set_session_id(Some(session_id.into()));
        mem.set_session_dir(Some(session_dir.clone()));
        mem.append_turn(
            "I love dark mode for coding",
            "Got it bro, dark mode forever.",
        )
        .unwrap();

        // Log must live under the session dir, not memory/sessions.
        let log = session_dir.join(SESSION_MEMORY_FILE);
        assert!(log.is_file(), "expected {}", log.display());
        assert!(!mem_root.join("desktop").join("sessions").exists());

        fs::write(
            mem.memory_md_path(),
            "# MEMORY\n\nUser prefers dark mode editors.\n",
        )
        .unwrap();

        let hits = mem.search("dark mode", 5).unwrap();
        assert!(!hits.is_empty(), "expected hits, got {hits:?}");
        assert!(hits.iter().any(|h| h.path.contains("MEMORY")
            || h.path.starts_with(SESSION_PATH_PREFIX)));

        let body = mem.get("MEMORY.md", 2000).unwrap();
        assert!(body.contains("dark mode"));

        let sess_body = mem
            .get(&format!("{SESSION_PATH_PREFIX}{session_id}/{SESSION_MEMORY_FILE}"), 2000)
            .unwrap();
        assert!(sess_body.contains("dark mode"));

        let _ = fs::remove_dir_all(&mem_root);
        let _ = fs::remove_dir_all(&sessions);
    }

    #[test]
    fn append_without_session_dir_is_noop() {
        let mem_root = tmp_root("unbound");
        let mem = LongTermMemory::new(&mem_root);
        mem.ensure_dirs().unwrap();
        mem.append_turn("hello", "world").unwrap();
        // No session file anywhere under memory root
        assert!(!mem_root.join("desktop").join("sessions").exists());
        let _ = fs::remove_dir_all(&mem_root);
    }

    #[test]
    fn get_rejects_escape() {
        let root = tmp_root("escape");
        let mem = LongTermMemory::new(&root);
        mem.ensure_dirs().unwrap();
        let err = mem.get("../secrets.txt", 100).unwrap_err();
        assert!(
            err.contains("escape") || err.contains("not a file"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn get_session_rejects_dotdot() {
        let mem_root = tmp_root("sess-esc-m");
        let sessions = tmp_root("sess-esc-s");
        let mem = LongTermMemory::new(&mem_root).with_sessions_root(&sessions);
        let err = mem
            .get("session/../evil/memory.md", 100)
            .unwrap_err();
        assert!(
            err.contains("escape") || err.contains("must be") || err.contains("not a file"),
            "{err}"
        );
        let _ = fs::remove_dir_all(&mem_root);
        let _ = fs::remove_dir_all(&sessions);
    }
}
