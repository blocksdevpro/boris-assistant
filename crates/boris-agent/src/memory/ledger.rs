//! Boris's canonical, local-first, evidence-carrying memory system.
//!
//! The legacy implementation treated a session markdown file as both the
//! archive and retrieval unit.
//! This module keeps those concerns separate:
//!
//! - `memory_events` is a source ledger (conversation/tool evidence).
//! - `memories` contains small, independently retrievable semantic or episodic
//!   records with a lifecycle and validity window.
//! - SQLite FTS is a rebuildable retrieval index, never the source of truth.
//!
//! The vector/graph planes are intentionally behind this canonical store. The
//! first release ships the lexical + structured plane so a desktop install
//! has no daemon or hosted dependency; vector candidates can be added as a
//! derived index without changing the record model.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{extract_heuristic, FactCategory, UserFact, UserProfile};

const SCHEMA_VERSION: i64 = 1;
const MAX_EVENT_CHARS: usize = 8_000;
const MAX_MEMORY_TEXT_CHARS: usize = 1_600;

/// Memory scope prevents personal, project, and session facts from bleeding
/// into each other as Boris gains connectors and multiple workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    User,
    Session,
    Workspace,
    Agent,
}

impl MemoryScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Session => "session",
            Self::Workspace => "workspace",
            Self::Agent => "agent",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "session" => Self::Session,
            "workspace" => Self::Workspace,
            "agent" => Self::Agent,
            _ => Self::User,
        }
    }
}

/// Different memories need different formation and retrieval policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Stable personal or world fact.
    Semantic,
    /// A dated interaction, decision, or outcome.
    Episodic,
    /// Current project state, commitment, or open loop.
    Project,
    /// A learned way Boris should help the human.
    Procedural,
}

impl MemoryKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Episodic => "episodic",
            Self::Project => "project",
            Self::Procedural => "procedural",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "episodic" => Self::Episodic,
            "project" => Self::Project,
            "procedural" => Self::Procedural,
            _ => Self::Semantic,
        }
    }
}

/// Privacy policy used by ingestion and the future Memory Center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPrivacy {
    /// Low-risk preferences and project details can be saved automatically.
    Standard,
    /// Sensitive data must be written only after an explicit user request.
    ExplicitOnly,
}

impl MemoryPrivacy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::ExplicitOnly => "explicit_only",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "explicit_only" => Self::ExplicitOnly,
            _ => Self::Standard,
        }
    }
}

/// Lifecycle is checked before every retrieval. Forgotten items are deleted
/// from the canonical store/index rather than merely hidden in a renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Superseded,
    Expired,
}

/// A source event is evidence, not automatically a reusable memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEvent {
    pub id: String,
    pub session_id: Option<String>,
    pub user_text: String,
    pub assistant_text: String,
    pub observed_at_ms: u64,
}

/// Canonical atom used for prompt retrieval and Memory Center display.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecord {
    pub id: String,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub text: String,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    /// Stable slot used to supersede an earlier value, e.g. `home_city`.
    pub memory_key: Option<String>,
    pub confidence: f32,
    pub importance: u8,
    pub privacy: MemoryPrivacy,
    pub status: MemoryStatus,
    pub source_event_id: Option<String>,
    pub valid_from_ms: u64,
    pub valid_to_ms: Option<u64>,
    pub observed_at_ms: u64,
    pub last_accessed_at_ms: Option<u64>,
    pub access_count: u32,
}

/// Input for an atomic memory write.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub text: String,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub memory_key: Option<String>,
    pub confidence: f32,
    pub importance: u8,
    pub privacy: MemoryPrivacy,
    pub source_event_id: Option<String>,
    pub valid_to_ms: Option<u64>,
}

impl NewMemory {
    pub fn semantic(text: impl Into<String>) -> Self {
        Self {
            scope: MemoryScope::User,
            kind: MemoryKind::Semantic,
            text: text.into(),
            subject: Some("user".into()),
            predicate: None,
            object: None,
            memory_key: None,
            confidence: 0.75,
            importance: 5,
            privacy: MemoryPrivacy::Standard,
            source_event_id: None,
            valid_to_ms: None,
        }
    }
}

/// Prompt-safe result. `score` is explainable and deterministic until a
/// vector index is attached as another candidate source.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySearchHit {
    pub record: MemoryRecord,
    pub score: f32,
}

/// Local SQLite source of truth for Boris memory.
#[derive(Debug)]
pub struct MemoryStore {
    path: PathBuf,
    conn: Mutex<Connection>,
}

impl MemoryStore {
    /// Open or create the canonical store. This starts separate from legacy
    /// `search.sqlite`, so rollback remains possible.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create memory directory: {e}"))?;
        }
        let conn = Connection::open(&path).map_err(|e| format!("open memory store: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS memory_meta (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS memory_events (
               id TEXT PRIMARY KEY,
               session_id TEXT,
               user_text TEXT NOT NULL,
               assistant_text TEXT NOT NULL,
               observed_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS memory_migrations (
               source_id TEXT PRIMARY KEY,
               source_path TEXT NOT NULL,
               content_hash TEXT NOT NULL,
               event_id TEXT NOT NULL,
               state TEXT NOT NULL,
               imported_at_ms INTEGER NOT NULL,
               refined_at_ms INTEGER,
               retired_at_ms INTEGER,
               FOREIGN KEY(event_id) REFERENCES memory_events(id) ON DELETE CASCADE
             );
             CREATE TABLE IF NOT EXISTS memories (
               id TEXT PRIMARY KEY,
               scope TEXT NOT NULL,
               kind TEXT NOT NULL,
               text TEXT NOT NULL,
               subject TEXT,
               predicate TEXT,
               object TEXT,
               memory_key TEXT,
               confidence REAL NOT NULL,
               importance INTEGER NOT NULL,
               privacy TEXT NOT NULL,
               status TEXT NOT NULL,
               source_event_id TEXT,
               valid_from_ms INTEGER NOT NULL,
               valid_to_ms INTEGER,
               observed_at_ms INTEGER NOT NULL,
               last_accessed_at_ms INTEGER,
               access_count INTEGER NOT NULL DEFAULT 0,
               FOREIGN KEY(source_event_id) REFERENCES memory_events(id) ON DELETE SET NULL
             );
             CREATE INDEX IF NOT EXISTS memories_active_key
               ON memories(scope, memory_key, status);
             CREATE INDEX IF NOT EXISTS memories_source_event
               ON memories(source_event_id);
             CREATE INDEX IF NOT EXISTS memories_validity
               ON memories(status, valid_to_ms, observed_at_ms);
             CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
               memory_id UNINDEXED,
               text,
               tokenize='unicode61 remove_diacritics 2'
             );",
        )
        .map_err(|e| format!("init memory schema: {e}"))?;
        conn.execute(
            "INSERT INTO memory_meta(key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )
        .map_err(|e| format!("write memory schema version: {e}"))?;
        Ok(Self {
            path,
            conn: Mutex::new(conn),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Store the compact extraction working set in the canonical database.
    /// It is a cache for extraction cadence and tool ergonomics; retrieved
    /// memory always comes from `memories`, never from this snapshot.
    pub fn load_profile_snapshot(&self) -> Result<UserProfile, String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM memory_meta WHERE key='profile_snapshot'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("load memory profile snapshot: {e}"))?;
        let Some(raw) = raw else {
            return Ok(UserProfile::default());
        };
        let mut profile: UserProfile = serde_json::from_str(&raw)
            .map_err(|e| format!("parse memory profile snapshot: {e}"))?;
        profile.expire_due_facts();
        Ok(profile)
    }

    /// Update the extraction snapshot and project active values into the
    /// retrievable record plane. This is the one write path for new profile
    /// changes; no JSON profile is needed after migration.
    pub fn save_profile_snapshot(&self, profile: &UserProfile) -> Result<(), String> {
        let raw = serde_json::to_string(profile)
            .map_err(|e| format!("serialize memory profile snapshot: {e}"))?;
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        conn.execute(
            "INSERT INTO memory_meta(key, value) VALUES ('profile_snapshot', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![raw],
        )
        .map_err(|e| format!("save memory profile snapshot: {e}"))?;
        drop(conn);
        self.sync_profile(profile)
    }

    /// Render the high-confidence personal layer directly from canonical
    /// records. This intentionally replaces profile.json prompt injection.
    pub fn personal_context(&self, max_chars: usize) -> Result<String, String> {
        let now = now_ms();
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, scope, kind, text, subject, predicate, object, memory_key,
                        confidence, importance, privacy, status, source_event_id,
                        valid_from_ms, valid_to_ms, observed_at_ms, last_accessed_at_ms, access_count
                 FROM memories
                 WHERE scope='user' AND status='active'
                   AND (valid_to_ms IS NULL OR valid_to_ms > ?1)
                 ORDER BY importance DESC, confidence DESC, observed_at_ms DESC
                 LIMIT 24",
            )
            .map_err(|e| format!("prepare personal memory context: {e}"))?;
        let rows = stmt
            .query_map(params![now as i64], row_to_record)
            .map_err(|e| format!("query personal memory context: {e}"))?;
        let mut lines = Vec::new();
        let mut used = 0usize;
        for row in rows {
            let record = row.map_err(|e| format!("read personal memory context: {e}"))?;
            let line = format!("- {}", record.text);
            if used + line.len() + 1 > max_chars {
                break;
            }
            used += line.len() + 1;
            lines.push(line);
        }
        if lines.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!(
                "<personal_context>\n{}\n</personal_context>",
                lines.join("\n")
            ))
        }
    }

    /// Record source evidence and deterministically promote only safe,
    /// high-precision signal without an extra LLM call.
    pub fn ingest_turn(
        &self,
        session_id: Option<&str>,
        user_text: &str,
        assistant_text: &str,
    ) -> Result<MemoryEvent, String> {
        let now = now_ms();
        let event = MemoryEvent {
            id: new_id("evt"),
            session_id: normalize_optional(session_id),
            user_text: truncate(user_text, MAX_EVENT_CHARS),
            assistant_text: truncate(assistant_text, MAX_EVENT_CHARS),
            observed_at_ms: now,
        };
        {
            let conn = self
                .conn
                .lock()
                .map_err(|_| "memory store lock poisoned".to_string())?;
            conn.execute(
                "INSERT INTO memory_events(id, session_id, user_text, assistant_text, observed_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.id,
                    event.session_id,
                    event.user_text,
                    event.assistant_text,
                    event.observed_at_ms as i64,
                ],
            )
            .map_err(|e| format!("insert memory event: {e}"))?;
        }

        let delta = extract_heuristic(&event.user_text);
        if delta.forget_all {
            self.forget_all()?;
            return Ok(event);
        }
        for query in &delta.facts_remove_query {
            self.forget_matching(query)?;
        }
        if delta.forget_preferred_name {
            self.forget_matching("preferred name")?;
        }
        if let Some(name) = delta.preferred_name {
            let mut memory = NewMemory::semantic(format!("Preferred name: {name}"));
            memory.memory_key = Some("preferred_name".into());
            memory.importance = 10;
            memory.source_event_id = Some(event.id.clone());
            self.upsert(memory)?;
        }
        if let Some(address_as) = delta.address_as {
            let mut memory = NewMemory::semantic(format!("Address user as: {address_as}"));
            memory.kind = MemoryKind::Procedural;
            memory.memory_key = Some("address_as".into());
            memory.importance = 9;
            memory.source_event_id = Some(event.id.clone());
            self.upsert(memory)?;
        }
        for preference in delta.preferences_add {
            let mut memory = NewMemory::semantic(format!("Preference: {preference}"));
            memory.kind = MemoryKind::Procedural;
            memory.importance = 8;
            memory.source_event_id = Some(event.id.clone());
            self.upsert(memory)?;
        }
        for fact in delta.facts_add {
            self.upsert(from_user_fact(fact, Some(event.id.clone())))?;
        }
        for ongoing in delta.ongoing_add {
            let mut memory = NewMemory::semantic(format!("Current project: {ongoing}"));
            memory.kind = MemoryKind::Project;
            memory.memory_key = Some(format!("project:{}", normalize_key(&ongoing)));
            memory.importance = 7;
            memory.source_event_id = Some(event.id.clone());
            self.upsert(memory)?;
        }

        // Keep an episodic fallback, but only inject it if it wins a
        // query-specific retrieval score.
        if event.user_text.len() >= 12 {
            let mut episode = NewMemory::semantic(format!(
                "Conversation: User said: {} Boris replied: {}",
                truncate(&event.user_text, 700),
                truncate(&event.assistant_text, 700)
            ));
            episode.scope = MemoryScope::Session;
            episode.kind = MemoryKind::Episodic;
            episode.importance = episode_importance(&event.user_text);
            episode.confidence = 1.0;
            episode.source_event_id = Some(event.id.clone());
            self.upsert(episode)?;
        }
        Ok(event)
    }

    /// Insert, refresh, or supersede an atomic memory. A nonempty stable key
    /// makes corrections deterministic rather than dependent on fuzzy text.
    pub fn upsert(&self, mut memory: NewMemory) -> Result<MemoryRecord, String> {
        memory.text = normalize_text(&memory.text);
        if memory.text.len() < 3 {
            return Err("memory text is too short".into());
        }
        memory.text = truncate(&memory.text, MAX_MEMORY_TEXT_CHARS);
        memory.memory_key = memory
            .memory_key
            .as_deref()
            .map(normalize_key)
            .filter(|key| !key.is_empty());
        let now = now_ms();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin memory upsert: {e}"))?;
        let duplicate: Option<String> = tx
            .query_row(
                "SELECT id FROM memories
                 WHERE scope=?1 AND text=?2 AND status='active'
                 ORDER BY observed_at_ms DESC LIMIT 1",
                params![memory.scope.as_str(), memory.text],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("find duplicate memory: {e}"))?;
        if let Some(id) = duplicate {
            tx.execute(
                "UPDATE memories SET observed_at_ms=?2, confidence=MAX(confidence, ?3),
                     importance=MAX(importance, ?4), source_event_id=COALESCE(?5, source_event_id)
                 WHERE id=?1",
                params![
                    id,
                    now as i64,
                    memory.confidence.clamp(0.0, 1.0),
                    memory.importance.clamp(1, 10) as i64,
                    memory.source_event_id,
                ],
            )
            .map_err(|e| format!("refresh duplicate memory: {e}"))?;
            tx.commit()
                .map_err(|e| format!("commit duplicate memory: {e}"))?;
            drop(conn);
            return self
                .get(&id)?
                .ok_or_else(|| "refreshed memory disappeared".into());
        }
        if let Some(key) = memory.memory_key.as_deref() {
            tx.execute(
                "UPDATE memories SET status='superseded', valid_to_ms=?3
                 WHERE scope=?1 AND memory_key=?2 AND status='active'",
                params![memory.scope.as_str(), key, now as i64],
            )
            .map_err(|e| format!("supersede keyed memory: {e}"))?;
        }
        let id = new_id("mem");
        tx.execute(
            "INSERT INTO memories(
               id, scope, kind, text, subject, predicate, object, memory_key,
               confidence, importance, privacy, status, source_event_id,
               valid_from_ms, valid_to_ms, observed_at_ms, last_accessed_at_ms, access_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'active', ?12, ?13, ?14, ?15, NULL, 0)",
            params![
                id,
                memory.scope.as_str(),
                memory.kind.as_str(),
                memory.text,
                memory.subject,
                memory.predicate,
                memory.object,
                memory.memory_key,
                memory.confidence.clamp(0.0, 1.0),
                memory.importance.clamp(1, 10) as i64,
                memory.privacy.as_str(),
                memory.source_event_id,
                now as i64,
                memory.valid_to_ms.map(|v| v as i64),
                now as i64,
            ],
        )
        .map_err(|e| format!("insert memory: {e}"))?;
        tx.execute(
            "INSERT INTO memory_fts(memory_id, text) VALUES (?1, ?2)",
            params![id, memory.text],
        )
        .map_err(|e| format!("index memory: {e}"))?;
        tx.commit().map_err(|e| format!("commit memory: {e}"))?;
        drop(conn);
        self.get(&id)?
            .ok_or_else(|| "inserted memory disappeared".into())
    }

    /// Project legacy profile data into the canonical store during migration.
    /// Repeated calls are idempotent.
    pub fn sync_profile(&self, profile: &UserProfile) -> Result<(), String> {
        if let Some(name) = &profile.preferred_name {
            let mut memory = NewMemory::semantic(format!("Preferred name: {name}"));
            memory.memory_key = Some("preferred_name".into());
            memory.importance = 10;
            self.upsert(memory)?;
        }
        if let Some(address_as) = &profile.address_as {
            let mut memory = NewMemory::semantic(format!("Address user as: {address_as}"));
            memory.kind = MemoryKind::Procedural;
            memory.memory_key = Some("address_as".into());
            memory.importance = 9;
            self.upsert(memory)?;
        }
        for preference in &profile.preferences {
            let mut memory = NewMemory::semantic(format!("Preference: {preference}"));
            memory.kind = MemoryKind::Procedural;
            memory.importance = 8;
            self.upsert(memory)?;
        }
        for fact in profile.facts.iter().filter(|fact| fact.is_active()) {
            self.upsert(from_user_fact(fact.clone(), None))?;
        }
        for ongoing in &profile.ongoing {
            let mut memory = NewMemory::semantic(format!("Current project: {ongoing}"));
            memory.kind = MemoryKind::Project;
            memory.memory_key = Some(format!("project:{}", normalize_key(ongoing)));
            memory.importance = 7;
            self.upsert(memory)?;
        }
        Ok(())
    }

    /// Stage one bounded legacy-memory excerpt as source evidence. The source
    /// remains in `staged` state until the migration worker has successfully
    /// run its AI refinement pass. Re-running after a crash is idempotent.
    pub fn stage_legacy_source(
        &self,
        source_id: &str,
        source_path: &str,
        content: &str,
    ) -> Result<bool, String> {
        let source_id = normalize_text(source_id);
        let source_path = normalize_text(source_path);
        let content = normalize_text(content);
        if source_id.is_empty() || source_path.is_empty() || content.len() < 3 {
            return Err("legacy memory source is empty".into());
        }
        let content_hash = stable_hash(&content);
        let now = now_ms();
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let existing: Option<(String, String, String)> = conn
            .query_row(
                "SELECT content_hash, state, event_id FROM memory_migrations WHERE source_id=?1",
                params![&source_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("read legacy migration state: {e}"))?;
        if let Some((old_hash, state, event_id)) = existing {
            // A `retired` marker normally has no source file to discover. If
            // a same-content legacy file appears again, stage it anew rather
            // than assuming a previous retirement still covers this file.
            if old_hash == content_hash && state == "refined" {
                return Ok(false);
            }
            if old_hash == content_hash && state == "staged" {
                drop(conn);
                self.ensure_legacy_episode(&source_id, &source_path, &content, event_id)?;
                return Ok(true);
            }
        }

        let event = MemoryEvent {
            id: new_id("legacy_evt"),
            session_id: None,
            user_text: truncate(&content, MAX_EVENT_CHARS),
            assistant_text: String::new(),
            observed_at_ms: now,
        };
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin legacy source stage: {e}"))?;
        tx.execute(
            "INSERT INTO memory_events(id, session_id, user_text, assistant_text, observed_at_ms)
             VALUES (?1, NULL, ?2, '', ?3)",
            params![&event.id, &event.user_text, event.observed_at_ms as i64],
        )
        .map_err(|e| format!("insert legacy source event: {e}"))?;
        tx.execute(
            "INSERT INTO memory_migrations(
                source_id, source_path, content_hash, event_id, state, imported_at_ms, refined_at_ms, retired_at_ms
             ) VALUES (?1, ?2, ?3, ?4, 'staged', ?5, NULL, NULL)
             ON CONFLICT(source_id) DO UPDATE SET
                source_path=excluded.source_path,
                content_hash=excluded.content_hash,
                event_id=excluded.event_id,
                state='staged',
                imported_at_ms=excluded.imported_at_ms,
                refined_at_ms=NULL,
                retired_at_ms=NULL",
            params![&source_id, &source_path, &content_hash, &event.id, now as i64],
        )
        .map_err(|e| format!("record legacy migration state: {e}"))?;
        tx.commit()
            .map_err(|e| format!("commit legacy source stage: {e}"))?;
        drop(conn);

        self.ensure_legacy_episode(&source_id, &source_path, &content, event.id)?;
        Ok(true)
    }

    /// Mark a staged source as AI-refined. Only a fully refined source set can
    /// cause legacy files to be retired.
    pub fn mark_legacy_source_refined(&self, source_id: &str) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let changed = conn
            .execute(
                "UPDATE memory_migrations SET state='refined', refined_at_ms=?2
                 WHERE source_id=?1 AND state='staged'",
                params![source_id.trim(), now_ms() as i64],
            )
            .map_err(|e| format!("mark legacy source refined: {e}"))?;
        if changed == 0 {
            return Err(format!(
                "legacy source `{}` is not staged",
                source_id.trim()
            ));
        }
        Ok(())
    }

    /// Verification gate used immediately before deleting legacy files.
    pub fn legacy_sources_refined(&self, source_ids: &[String]) -> Result<bool, String> {
        if source_ids.is_empty() {
            return Ok(true);
        }
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        for source_id in source_ids {
            let state: Option<String> = conn
                .query_row(
                    "SELECT state FROM memory_migrations WHERE source_id=?1",
                    params![source_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| format!("verify legacy migration state: {e}"))?;
            if !matches!(state.as_deref(), Some("refined") | Some("retired")) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn mark_legacy_sources_retired(&self, source_ids: &[String]) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("begin legacy retirement state: {e}"))?;
        for source_id in source_ids {
            let changed = tx
                .execute(
                    "UPDATE memory_migrations SET state='retired', retired_at_ms=?2
                     WHERE source_id=?1 AND state='refined'",
                    params![source_id, now_ms() as i64],
                )
                .map_err(|e| format!("mark legacy source retired: {e}"))?;
            if changed == 0 {
                return Err(format!("legacy source `{source_id}` was not refined"));
            }
        }
        tx.commit()
            .map_err(|e| format!("commit legacy retirement state: {e}"))
    }

    /// Hybrid-ready retrieval: direct lifecycle filtering plus lexical
    /// candidates today. A future vector index contributes candidates to this
    /// same ranker rather than becoming a second source of truth.
    pub fn search(&self, query: &str, max_results: usize) -> Result<Vec<MemorySearchHit>, String> {
        let now = now_ms();
        let tokens = query_tokens(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = tokens
            .iter()
            .map(|token| format!("\"{token}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.scope, m.kind, m.text, m.subject, m.predicate, m.object,
                        m.memory_key, m.confidence, m.importance, m.privacy, m.status,
                        m.source_event_id, m.valid_from_ms, m.valid_to_ms, m.observed_at_ms,
                        m.last_accessed_at_ms, m.access_count
                 FROM memory_fts f
                 JOIN memories m ON m.id=f.memory_id
                 WHERE memory_fts MATCH ?1
                   AND m.status='active'
                   AND (m.valid_to_ms IS NULL OR m.valid_to_ms > ?2)
                 LIMIT 96",
            )
            .map_err(|e| format!("prepare memory search: {e}"))?;
        let rows = stmt
            .query_map(params![fts_query, now as i64], row_to_record)
            .map_err(|e| format!("query memory search: {e}"))?;
        let mut hits = Vec::new();
        for row in rows {
            let record = row.map_err(|e| format!("read memory row: {e}"))?;
            let score = score(&record, &tokens, now);
            if score > 0.0 {
                hits.push(MemorySearchHit { record, score });
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| b.record.observed_at_ms.cmp(&a.record.observed_at_ms))
        });
        hits.truncate(max_results.clamp(1, 20));
        drop(stmt);
        for hit in &hits {
            conn.execute(
                "UPDATE memories SET last_accessed_at_ms=?2, access_count=access_count+1 WHERE id=?1",
                params![hit.record.id, now as i64],
            )
            .map_err(|e| format!("record memory access: {e}"))?;
        }
        Ok(hits)
    }

    pub fn get(&self, id: &str) -> Result<Option<MemoryRecord>, String> {
        let now = now_ms();
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        conn.query_row(
            "SELECT id, scope, kind, text, subject, predicate, object, memory_key,
                    confidence, importance, privacy, status, source_event_id,
                    valid_from_ms, valid_to_ms, observed_at_ms, last_accessed_at_ms, access_count
             FROM memories WHERE id=?1 AND status='active'
               AND (valid_to_ms IS NULL OR valid_to_ms > ?2)",
            params![id.trim(), now as i64],
            row_to_record,
        )
        .optional()
        .map_err(|e| format!("read memory: {e}"))
    }

    /// Hard forget removes canonical memory/index entries and any source event
    /// containing the exact requested phrase, preventing direct re-retrieval.
    pub fn forget_matching(&self, query: &str) -> Result<usize, String> {
        let query = normalize_text(query);
        if query.len() < 3 {
            return Err("forget query is too short".into());
        }
        let pattern = format!("%{}%", query.to_ascii_lowercase());
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("begin forget: {e}"))?;
        let mut ids = Vec::new();
        {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM memories
                     WHERE lower(text) LIKE ?1 OR lower(COALESCE(subject, '')) LIKE ?1
                        OR lower(COALESCE(predicate, '')) LIKE ?1 OR lower(COALESCE(object, '')) LIKE ?1",
                )
                .map_err(|e| format!("prepare forget lookup: {e}"))?;
            let rows = stmt
                .query_map(params![pattern], |row| row.get::<_, String>(0))
                .map_err(|e| format!("query forget lookup: {e}"))?;
            for row in rows {
                ids.push(row.map_err(|e| format!("read forget id: {e}"))?);
            }
        }
        for id in &ids {
            tx.execute("DELETE FROM memory_fts WHERE memory_id=?1", params![id])
                .map_err(|e| format!("delete forgotten fts entry: {e}"))?;
            tx.execute("DELETE FROM memories WHERE id=?1", params![id])
                .map_err(|e| format!("delete forgotten memory: {e}"))?;
        }
        tx.execute(
            "DELETE FROM memory_events WHERE lower(user_text) LIKE ?1 OR lower(assistant_text) LIKE ?1",
            params![format!("%{}%", query.to_ascii_lowercase())],
        )
        .map_err(|e| format!("delete matching source events: {e}"))?;
        tx.commit().map_err(|e| format!("commit forget: {e}"))?;
        drop(conn);
        self.forget_from_profile_snapshot(&query)?;
        Ok(ids.len())
    }

    /// Complete erasure used only for an explicit "forget everything" request.
    pub fn forget_all(&self) -> Result<(), String> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| "memory store lock poisoned".to_string())?;
        conn.execute_batch(
            "DELETE FROM memory_fts; DELETE FROM memories; DELETE FROM memory_events;",
        )
        .map_err(|e| format!("erase memory: {e}"))?;
        drop(conn);
        self.save_profile_snapshot(&UserProfile::default())
    }

    pub fn prompt_hint(&self) -> String {
        "<memory>\nUse memory_search when prior decisions, people, projects, or preferences are relevant. Search returns current evidence-backed memory records. Never claim a memory that was not returned.\n</memory>".into()
    }

    fn forget_from_profile_snapshot(&self, query: &str) -> Result<(), String> {
        let mut profile = self.load_profile_snapshot()?;
        profile.forget_matching(query);
        self.save_profile_snapshot(&profile)
    }

    fn ensure_legacy_episode(
        &self,
        source_id: &str,
        source_path: &str,
        content: &str,
        event_id: String,
    ) -> Result<(), String> {
        // A conservative episode preserves a searchable provenance trail
        // while the AI refiner turns the excerpt into smaller semantic records.
        let mut episode = NewMemory::semantic(format!(
            "Archived memory from {source_path}: {}",
            truncate(content, 1_100)
        ));
        episode.scope = MemoryScope::Agent;
        episode.kind = MemoryKind::Episodic;
        episode.memory_key = Some(format!("legacy_source_{source_id}"));
        episode.importance = 2;
        episode.confidence = 0.55;
        episode.source_event_id = Some(event_id);
        self.upsert(episode).map(|_| ())
    }

    #[cfg(test)]
    fn event_count(&self) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM memory_events", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }
}

fn from_user_fact(fact: UserFact, source_event_id: Option<String>) -> NewMemory {
    let mut memory = NewMemory::semantic(fact.text);
    memory.kind = match fact.category {
        FactCategory::Project => MemoryKind::Project,
        FactCategory::Preference | FactCategory::Habit => MemoryKind::Procedural,
        _ => MemoryKind::Semantic,
    };
    memory.memory_key = fact.memory_key;
    memory.confidence = fact.confidence;
    memory.importance = fact.salience;
    memory.source_event_id = source_event_id;
    memory.valid_to_ms = fact.expires_at_ms;
    memory
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let scope: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let privacy: String = row.get(10)?;
    let status: String = row.get(11)?;
    Ok(MemoryRecord {
        id: row.get(0)?,
        scope: MemoryScope::parse(&scope),
        kind: MemoryKind::parse(&kind),
        text: row.get(3)?,
        subject: row.get(4)?,
        predicate: row.get(5)?,
        object: row.get(6)?,
        memory_key: row.get(7)?,
        confidence: row.get(8)?,
        importance: row.get::<_, i64>(9)?.clamp(0, 10) as u8,
        privacy: MemoryPrivacy::parse(&privacy),
        status: match status.as_str() {
            "superseded" => MemoryStatus::Superseded,
            "expired" => MemoryStatus::Expired,
            _ => MemoryStatus::Active,
        },
        source_event_id: row.get(12)?,
        valid_from_ms: row.get::<_, i64>(13)?.max(0) as u64,
        valid_to_ms: row
            .get::<_, Option<i64>>(14)?
            .map(|value| value.max(0) as u64),
        observed_at_ms: row.get::<_, i64>(15)?.max(0) as u64,
        last_accessed_at_ms: row
            .get::<_, Option<i64>>(16)?
            .map(|value| value.max(0) as u64),
        access_count: row.get::<_, i64>(17)?.max(0) as u32,
    })
}

fn score(record: &MemoryRecord, tokens: &[String], now: u64) -> f32 {
    let text = record.text.to_ascii_lowercase();
    let matches = tokens
        .iter()
        .filter(|token| text.contains(token.as_str()))
        .count();
    if matches == 0 {
        return 0.0;
    }
    let coverage = matches as f32 / tokens.len() as f32;
    let age_days = now.saturating_sub(record.observed_at_ms) as f32 / 86_400_000.0;
    let recency = 1.0 / (1.0 + age_days / 45.0);
    let importance = record.importance as f32 / 10.0;
    let strength = (record.access_count.min(20) as f32) / 40.0;
    let kind_boost = match record.kind {
        MemoryKind::Semantic | MemoryKind::Procedural => 0.18,
        MemoryKind::Project => 0.14,
        MemoryKind::Episodic => 0.0,
    };
    coverage * 0.55
        + record.confidence * 0.15
        + importance * 0.14
        + recency * 0.10
        + strength
        + kind_boost
}

fn query_tokens(query: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "about", "also", "been", "could", "from", "have", "help", "just", "make", "okay", "please",
        "that", "then", "there", "these", "they", "think", "this", "want", "what", "when", "where",
        "which", "with", "would", "your", "were", "does", "did", "tell",
    ];
    let mut seen = std::collections::HashSet::new();
    query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|token| token.chars().count() >= 2 && !STOP.contains(&token.as_str()))
        .filter(|token| seen.insert(token.clone()))
        .take(12)
        .collect()
}

fn episode_importance(text: &str) -> u8 {
    let lower = text.to_ascii_lowercase();
    if [
        "decided",
        "remember",
        "important",
        "deadline",
        "promise",
        "never",
        "always",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        7
    } else {
        3
    }
}

fn normalize_text(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn normalize_optional(raw: Option<&str>) -> Option<String> {
    raw.map(normalize_text).filter(|text| !text.is_empty())
}

fn normalize_key(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn truncate(raw: &str, max_chars: usize) -> String {
    let mut out = raw.chars().take(max_chars).collect::<String>();
    if raw.chars().count() > max_chars {
        out.push_str("…");
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!(
        "{prefix}_{:x}_{:x}",
        now_ms(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Stable, dependency-free content fingerprint for migration idempotence. It
/// is not a security primitive; it only distinguishes a changed local source.
fn stable_hash(raw: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(label: &str) -> (MemoryStore, PathBuf) {
        let path = std::env::temp_dir().join(format!("boris-memory-{label}-{}.sqlite", now_ms()));
        let store = MemoryStore::open(&path).unwrap();
        (store, path)
    }

    #[test]
    fn keyed_correction_supersedes_old_value() {
        let (store, path) = store("supersede");
        let mut first = NewMemory::semantic("Home city: Delhi");
        first.memory_key = Some("home_city".into());
        store.upsert(first).unwrap();
        let mut second = NewMemory::semantic("Home city: Bangalore");
        second.memory_key = Some("home_city".into());
        store.upsert(second).unwrap();
        let hits = store.search("home city", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].record.text.contains("Bangalore"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn turn_ingestion_promotes_preference_and_keeps_event() {
        let (store, path) = store("ingest");
        store
            .ingest_turn(Some("s1"), "I prefer concise Rust answers", "Got it.")
            .unwrap();
        let hits = store.search("Rust answers", 5).unwrap();
        assert!(hits
            .iter()
            .any(|hit| hit.record.text.contains("concise Rust answers")));
        assert_eq!(store.event_count(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn forget_removes_memory_index_and_matching_evidence() {
        let (store, path) = store("forget");
        store
            .ingest_turn(Some("s1"), "I live in Paris", "Thanks.")
            .unwrap();
        assert!(!store.search("Paris", 5).unwrap().is_empty());
        store.forget_matching("Paris").unwrap();
        assert!(store.search("Paris", 5).unwrap().is_empty());
        assert_eq!(store.event_count(), 0);
        let _ = std::fs::remove_file(path);
    }
}
