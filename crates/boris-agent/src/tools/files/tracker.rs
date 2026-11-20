//! Read-before-edit tracking for `file_edit` / `file_write`.
//!
//! `file_read` records each text file it returns with a stamp (length,
//! modification time, content hash). `file_edit`, and `file_write` over an
//! existing file, refuse when the file was not read in this session or has
//! changed on disk since that read, so the model never edits from a stale or
//! imagined copy. New files need no prior read, and Boris's own successful
//! writes refresh the stamp so consecutive edits work without rereading.
//!
//! Keyed by the tool-call session id (`None` shares one anonymous bucket).
//! Bounded: a few recent sessions, a capped path count per session. This is a
//! correctness guard against stale edits, not a security boundary — path
//! policy and HITL are unchanged.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use crate::tool::ToolError;

/// Sessions kept before the least recently used one is forgotten.
const MAX_SESSIONS: usize = 8;
/// Paths remembered per session before the oldest read is forgotten.
const MAX_PATHS_PER_SESSION: usize = 1024;

/// What a file looked like when Boris last read or wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    hash: u64,
}

impl FileStamp {
    /// Stamp from bytes Boris just read or wrote plus the file's metadata.
    pub(crate) fn from_bytes(bytes: &[u8], meta: Option<&std::fs::Metadata>) -> Self {
        Self {
            len: bytes.len() as u64,
            modified: meta.and_then(|m| m.modified().ok()),
            hash: hash_bytes(bytes),
        }
    }
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

#[derive(Default)]
struct SessionReads {
    seq: u64,
    paths: HashMap<PathBuf, (u64, FileStamp)>,
}

#[derive(Default)]
struct Inner {
    clock: u64,
    /// session key → (last used tick, reads)
    sessions: HashMap<String, (u64, SessionReads)>,
}

/// Shared read tracker for the file tools.
#[derive(Default)]
pub struct FileReadTracker {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for FileReadTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileReadTracker").finish_non_exhaustive()
    }
}

/// Why an edit or overwrite is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Staleness {
    NotRead,
    Changed,
}

impl FileReadTracker {
    /// Process-wide tracker the file tools share by default.
    pub fn global() -> Arc<FileReadTracker> {
        static GLOBAL: OnceLock<Arc<FileReadTracker>> = OnceLock::new();
        Arc::clone(GLOBAL.get_or_init(|| Arc::new(FileReadTracker::default())))
    }

    /// Record that `path` was read (or written by Boris) with `stamp`.
    pub(crate) fn record(&self, session: Option<&str>, path: &Path, stamp: FileStamp) {
        let key = path_key(path);
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.clock += 1;
        let tick = inner.clock;
        let session_key = session.unwrap_or("").to_string();
        if !inner.sessions.contains_key(&session_key) && inner.sessions.len() >= MAX_SESSIONS {
            if let Some(oldest) = inner
                .sessions
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone())
            {
                inner.sessions.remove(&oldest);
            }
        }
        let entry = inner.sessions.entry(session_key).or_default();
        entry.0 = tick;
        let reads = &mut entry.1;
        reads.seq += 1;
        let seq = reads.seq;
        reads.paths.insert(key, (seq, stamp));
        if reads.paths.len() > MAX_PATHS_PER_SESSION {
            if let Some(oldest) = reads
                .paths
                .iter()
                .min_by_key(|(_, (s, _))| *s)
                .map(|(k, _)| k.clone())
            {
                reads.paths.remove(&oldest);
            }
        }
    }

    /// Record `path` from its current on-disk bytes (after a Boris write).
    pub(crate) fn record_written(&self, session: Option<&str>, path: &Path, bytes: &[u8]) {
        let meta = std::fs::metadata(path).ok();
        self.record(session, path, FileStamp::from_bytes(bytes, meta.as_ref()));
    }

    /// Whether `path` (which exists) may be modified now. A length and
    /// mtime match is enough; otherwise the content hash decides, so a touch
    /// or a no-op rewrite does not force a reread.
    pub(crate) fn staleness(&self, session: Option<&str>, path: &Path) -> Option<Staleness> {
        let key = path_key(path);
        let stamp = {
            // A poisoned lock fails open rather than blocking every edit.
            let inner = self.inner.lock().ok()?;
            inner
                .sessions
                .get(session.unwrap_or(""))
                .and_then(|(_, reads)| reads.paths.get(&key).map(|(_, s)| *s))
        };
        let Some(stamp) = stamp else {
            return Some(Staleness::NotRead);
        };
        let meta = std::fs::metadata(path).ok()?;
        if meta.len() == stamp.len && meta.modified().ok() == stamp.modified {
            return None;
        }
        if meta.len() != stamp.len {
            return Some(Staleness::Changed);
        }
        match std::fs::read(path) {
            Ok(bytes) if hash_bytes(&bytes) == stamp.hash => {
                // Same content, new mtime: refresh so later checks stay cheap.
                self.record(session, path, FileStamp::from_bytes(&bytes, Some(&meta)));
                None
            }
            _ => Some(Staleness::Changed),
        }
    }

    /// `Ok` when `path` is new or was read and is unchanged; otherwise a
    /// repairable error telling the model to `file_read` first.
    pub(crate) fn require_fresh(
        &self,
        session: Option<&str>,
        path: &Path,
        action: &str,
    ) -> Result<(), ToolError> {
        if !path.is_file() {
            return Ok(());
        }
        match self.staleness(session, path) {
            None => Ok(()),
            Some(Staleness::NotRead) => Err(ToolError::failed(format!(
                "Refused to {action} {}: it has not been read in this session. \
                 Call file_read on it first, then retry with text copied from that read.",
                path.display()
            ))),
            Some(Staleness::Changed) => Err(ToolError::failed(format!(
                "Refused to {action} {}: it changed on disk since you last read it. \
                 Call file_read on it again, then retry against the current content.",
                path.display()
            ))),
        }
    }
}

/// Stable map key: canonical when the file exists, case-folded on Windows.
fn path_key(path: &Path) -> PathBuf {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    {
        PathBuf::from(p.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("boris-tracker-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn read_into(t: &FileReadTracker, session: Option<&str>, p: &Path) {
        let bytes = std::fs::read(p).unwrap();
        let meta = std::fs::metadata(p).unwrap();
        t.record(session, p, FileStamp::from_bytes(&bytes, Some(&meta)));
    }

    #[test]
    fn unread_file_is_refused_and_new_file_allowed() {
        let t = FileReadTracker::default();
        let p = temp_file("unread.txt", "x");
        let err = t.require_fresh(Some("s"), &p, "edit").unwrap_err();
        assert!(err.message.contains("file_read"), "{}", err.message);
        let missing = p.with_file_name("does-not-exist.txt");
        t.require_fresh(Some("s"), &missing, "write")
            .expect("new file");
    }

    #[test]
    fn read_then_external_change_is_refused() {
        let t = FileReadTracker::default();
        let p = temp_file("changed.txt", "one");
        read_into(&t, Some("s"), &p);
        t.require_fresh(Some("s"), &p, "edit").expect("fresh");
        std::fs::write(&p, "two two").unwrap();
        let err = t.require_fresh(Some("s"), &p, "edit").unwrap_err();
        assert!(err.message.contains("changed on disk"), "{}", err.message);
    }

    #[test]
    fn same_content_rewrite_still_counts_as_fresh() {
        let t = FileReadTracker::default();
        let p = temp_file("touched.txt", "same");
        let bytes = std::fs::read(&p).unwrap();
        // Stamp with a deliberately wrong mtime: the hash must rescue it.
        t.record(
            Some("s"),
            &p,
            FileStamp {
                len: bytes.len() as u64,
                modified: Some(SystemTime::UNIX_EPOCH),
                hash: hash_bytes(&bytes),
            },
        );
        t.require_fresh(Some("s"), &p, "edit")
            .expect("same content");
    }

    #[test]
    fn reads_are_per_session() {
        let t = FileReadTracker::default();
        let p = temp_file("session.txt", "x");
        read_into(&t, Some("a"), &p);
        t.require_fresh(Some("a"), &p, "edit")
            .expect("session a read it");
        assert!(t.require_fresh(Some("b"), &p, "edit").is_err());
    }

    #[test]
    fn own_write_refreshes_stamp() {
        let t = FileReadTracker::default();
        let p = temp_file("own.txt", "v1");
        read_into(&t, None, &p);
        std::fs::write(&p, "v2 longer").unwrap();
        t.record_written(None, &p, b"v2 longer");
        t.require_fresh(None, &p, "edit").expect("own write");
    }

    #[test]
    fn session_count_is_bounded() {
        let t = FileReadTracker::default();
        let p = temp_file("bounded.txt", "x");
        for i in 0..(MAX_SESSIONS + 3) {
            read_into(&t, Some(&format!("s{i}")), &p);
        }
        assert_eq!(t.inner.lock().unwrap().sessions.len(), MAX_SESSIONS);
        // The oldest session was forgotten.
        assert!(t.require_fresh(Some("s0"), &p, "edit").is_err());
    }
}
