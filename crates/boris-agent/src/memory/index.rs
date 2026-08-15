//! Incrementally maintained local search index (SQLite FTS5).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

const INDEX_SCHEMA_VERSION: i64 = 1;

/// On-disk FTS5 index for memory markdown + session logs.
pub struct MemoryIndex {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for MemoryIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryIndex")
            .field("path", &self.path)
            .finish()
    }
}

impl MemoryIndex {
    /// Open (or create) `search.sqlite` under `memory_root`.
    pub fn open(memory_root: impl AsRef<Path>) -> Result<Self, String> {
        let root = memory_root.as_ref();
        std::fs::create_dir_all(root).map_err(|e| format!("create memory index dir: {e}"))?;
        let path = root.join("search.sqlite");
        let conn = Connection::open(&path).map_err(|e| format!("open memory index: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS memory_meta (
                 path TEXT PRIMARY KEY,
                 source TEXT NOT NULL,
                 salience INTEGER NOT NULL DEFAULT 1,
                 mtime_unix INTEGER NOT NULL,
                 recency_unix INTEGER NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
                 path UNINDEXED,
                 body,
                 tokenize = 'porter'
             );
             CREATE TABLE IF NOT EXISTS memory_index_state (
                 id INTEGER PRIMARY KEY CHECK(id = 1),
                 schema_version INTEGER NOT NULL,
                 rebuilt_unix INTEGER NOT NULL
             );",
        )
        .map_err(|e| format!("init memory index: {e}"))?;
        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn upsert(
        &self,
        path: &str,
        body: &str,
        source: &str,
        salience: u32,
    ) -> Result<(), String> {
        let now = unix_now();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("index begin upsert: {e}"))?;
        tx.execute("DELETE FROM memory_fts WHERE path = ?1", params![path])
            .map_err(|e| format!("index delete fts: {e}"))?;
        tx.execute(
            "INSERT INTO memory_fts(path, body) VALUES (?1, ?2)",
            params![path, body],
        )
        .map_err(|e| format!("index insert fts: {e}"))?;
        tx.execute(
            "INSERT INTO memory_meta(path, source, salience, mtime_unix, recency_unix)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                source=excluded.source,
                salience=excluded.salience,
                mtime_unix=excluded.mtime_unix,
                recency_unix=excluded.recency_unix",
            params![path, source, salience as i64, now as i64, now as i64],
        )
        .map_err(|e| format!("index upsert meta: {e}"))?;
        tx.commit()
            .map_err(|e| format!("index commit upsert: {e}"))?;
        Ok(())
    }

    /// Ranked search: FTS relevance + recency + salience, then dedupe by path.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<IndexHit>, String> {
        let q = query.trim();
        if q.is_empty() {
            return Err("query is empty".into());
        }
        let max_results = max_results.clamp(1, 20);
        let fts_q = fts_query(q);
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT f.path, snippet(memory_fts, 1, '', '', '…', 24),
                        bm25(memory_fts), m.salience, m.recency_unix, m.source
                 FROM memory_fts f
                 JOIN memory_meta m ON m.path = f.path
                 WHERE memory_fts MATCH ?1
                 ORDER BY (bm25(memory_fts) * -1.0)
                          + (m.salience * 0.15)
                          + ((m.recency_unix / 86400.0) * 0.01) DESC
                 LIMIT ?2",
            )
            .map_err(|e| format!("prepare memory search: {e}"))?;
        let rows = stmt
            .query_map(params![fts_q, max_results as i64], |row| {
                Ok(IndexHit {
                    path: row.get(0)?,
                    snippet: row.get(1)?,
                    score: score_from_bm25(row.get::<_, f64>(2)?, row.get::<_, i64>(3)?),
                    source: row.get(5)?,
                })
            })
            .map_err(|e| format!("memory search: {e}"))?;
        let mut hits = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let hit = row.map_err(|e| format!("memory search row: {e}"))?;
            if seen.insert(hit.path.clone()) {
                hits.push(hit);
            }
        }
        Ok(hits)
    }

    pub fn rebuild(&self) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        // Clear the ready marker first. If rebuilding is interrupted, the next
        // process start will retry instead of accepting a half-populated index.
        conn.execute_batch(
            "DELETE FROM memory_index_state;
             DELETE FROM memory_fts;
             DELETE FROM memory_meta;",
        )
        .map_err(|e| format!("rebuild memory index: {e}"))?;
        Ok(())
    }

    /// Whether a full source scan is required for this on-disk schema.
    pub fn needs_rebuild(&self) -> Result<bool, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        let version: Option<i64> = conn
            .query_row(
                "SELECT schema_version FROM memory_index_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("read memory index version: {e}"))?;
        Ok(version != Some(INDEX_SCHEMA_VERSION))
    }

    /// Mark a complete source scan as durable and current for this schema.
    pub fn mark_rebuilt(&self) -> Result<(), String> {
        let now = unix_now() as i64;
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        conn.execute(
            "INSERT INTO memory_index_state(id, schema_version, rebuilt_unix)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET
                schema_version=excluded.schema_version,
                rebuilt_unix=excluded.rebuilt_unix",
            params![INDEX_SCHEMA_VERSION, now],
        )
        .map_err(|e| format!("mark memory index rebuilt: {e}"))?;
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory index lock".to_string())?;
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM memory_meta", [], |r| r.get(0))
            .optional()
            .map_err(|e| format!("count memory index: {e}"))?
            .unwrap_or(0);
        Ok(n == 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexHit {
    pub path: String,
    pub snippet: String,
    pub score: u32,
    pub source: String,
}

fn score_from_bm25(bm25: f64, salience: i64) -> u32 {
    // bm25 is lower-is-better; invert into a 0..10k-ish rank.
    let base = (-bm25 * 40.0).max(1.0);
    (base as u32).saturating_add((salience.max(0) as u32).saturating_mul(5))
}

fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .filter(|w| w.chars().any(|c| c.is_alphanumeric()))
        .map(|w| {
            let cleaned: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            format!("\"{cleaned}\"")
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("boris-memidx-{n}-{}", unix_now()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn upsert_and_search() {
        let dir = temp_dir();
        let idx = MemoryIndex::open(&dir).unwrap();
        idx.upsert("MEMORY.md", "User likes rust and boris", "global", 3)
            .unwrap();
        idx.upsert(
            "session/abc/memory.md",
            "Talked about baking bread",
            "session",
            1,
        )
        .unwrap();
        let hits = idx.search("rust", 5).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "MEMORY.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebuild_marker_survives_reopen() {
        let dir = temp_dir();
        {
            let idx = MemoryIndex::open(&dir).unwrap();
            assert!(idx.needs_rebuild().unwrap());
            idx.rebuild().unwrap();
            idx.upsert("MEMORY.md", "durable", "global", 3).unwrap();
            idx.mark_rebuilt().unwrap();
            assert!(!idx.needs_rebuild().unwrap());
        }
        let reopened = MemoryIndex::open(&dir).unwrap();
        assert!(!reopened.needs_rebuild().unwrap());
        assert_eq!(reopened.search("durable", 5).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interrupted_rebuild_remains_dirty() {
        let dir = temp_dir();
        let idx = MemoryIndex::open(&dir).unwrap();
        idx.mark_rebuilt().unwrap();
        assert!(!idx.needs_rebuild().unwrap());
        idx.rebuild().unwrap();
        assert!(idx.needs_rebuild().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
