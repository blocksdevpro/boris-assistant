//! Verified import of the retired Markdown/profile memory layout.
//!
//! The importer deliberately separates staging, AI refinement, and retirement:
//! a file is never deleted merely because it was read. Every non-empty excerpt
//! must be marked refined in the canonical store first.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{FactStatus, MemoryStore, ProfileStore, UserProfile};

const CHUNK_CHARS: usize = 6_000;

/// Host paths containing data written by the retired memory implementation.
#[derive(Debug, Clone)]
pub struct LegacyMemoryPaths {
    pub profile_path: PathBuf,
    pub memory_root: PathBuf,
    pub sessions_root: PathBuf,
}

/// One bounded archived excerpt to refine with the configured LLM.
#[derive(Debug, Clone)]
pub struct LegacyMemorySource {
    pub id: String,
    pub path: PathBuf,
    pub label: String,
    pub content: String,
}

/// A deterministic plan. Discovery performs no writes or deletes.
#[derive(Debug, Clone)]
pub struct LegacyMigrationPlan {
    pub profile: Option<UserProfile>,
    pub sources: Vec<LegacyMemorySource>,
    pub files_to_retire: Vec<PathBuf>,
}

impl LegacyMigrationPlan {
    pub fn source_ids(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|source| source.id.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.profile.is_none() && self.sources.is_empty() && self.files_to_retire.is_empty()
    }
}

/// Discover the exact legacy files Boris used. This is intentionally narrow:
/// it never walks arbitrary user directories or imports notes/artifacts.
pub fn discover_legacy_memory(paths: &LegacyMemoryPaths) -> Result<LegacyMigrationPlan, String> {
    let mut candidates = vec![
        paths.memory_root.join("MEMORY.md"),
        paths.memory_root.join("desktop").join("MEMORY.md"),
    ];
    candidates.extend(session_memory_files(&paths.sessions_root)?);
    candidates.extend(session_memory_files(
        &paths.memory_root.join("desktop").join("sessions"),
    )?);

    let mut seen = HashSet::new();
    candidates.retain(|path| seen.insert(normalized_path_key(path)));

    let profile = if paths.profile_path.is_file() {
        Some(ProfileStore::new(paths.profile_path.clone()).load()?)
    } else {
        None
    };
    let mut files_to_retire = candidates
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect::<Vec<_>>();
    if paths.profile_path.is_file() {
        files_to_retire.push(paths.profile_path.clone());
    }
    // The old FTS index is derived entirely from the Markdown sources.
    for suffix in ["search.sqlite", "search.sqlite-wal", "search.sqlite-shm"] {
        let path = paths.memory_root.join(suffix);
        if path.is_file() {
            files_to_retire.push(path);
        }
    }

    let mut sources = Vec::new();
    for path in candidates.into_iter().filter(|path| path.is_file()) {
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read legacy memory {}: {e}", path.display()))?;
        for (part, content) in split_chunks(&raw).into_iter().enumerate() {
            let path_key = stable_path_id(&path);
            sources.push(LegacyMemorySource {
                id: format!("legacy-{path_key}-{part}"),
                label: format!("{} (part {})", path.display(), part + 1),
                path: path.clone(),
                content,
            });
        }
    }
    Ok(LegacyMigrationPlan {
        profile,
        sources,
        files_to_retire,
    })
}

/// Merge structured legacy profile data without overwriting facts already
/// learned in a running canonical install.
pub fn merge_legacy_profile(current: &mut UserProfile, legacy: UserProfile) {
    if current.preferred_name.is_none() {
        current.preferred_name = legacy.preferred_name;
    }
    if current.address_as.is_none() {
        current.address_as = legacy.address_as;
    }
    for preference in legacy.preferences {
        current.add_preference(preference);
    }
    for fact in legacy
        .facts
        .into_iter()
        .filter(|fact| fact.status == FactStatus::Active && fact.is_active())
    {
        current.add_or_refresh_fact(fact);
    }
    for ongoing in legacy.ongoing {
        current.add_ongoing(ongoing);
    }
    current.turns_seen = current.turns_seen.max(legacy.turns_seen);
    current.touch();
}

/// Delete only files whose complete source set has been verified as refined.
/// On a partial filesystem failure the canonical import remains intact and the
/// next startup can retry the remaining retired files safely.
pub fn retire_legacy_files(
    store: &MemoryStore,
    plan: &LegacyMigrationPlan,
) -> Result<Vec<PathBuf>, String> {
    let source_ids = plan.source_ids();
    if !store.legacy_sources_refined(&source_ids)? {
        return Err("legacy migration is not fully AI-refined; retirement refused".into());
    }
    let mut retired = Vec::new();
    for path in &plan.files_to_retire {
        if path.is_file() {
            fs::remove_file(path)
                .map_err(|e| format!("delete verified legacy memory {}: {e}", path.display()))?;
            retired.push(path.clone());
        }
    }
    store.mark_legacy_sources_retired(&source_ids)?;
    Ok(retired)
}

fn session_memory_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let entries =
        fs::read_dir(root).map_err(|e| format!("read legacy sessions {}: {e}", root.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("read legacy session entry: {e}"))?;
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            let memory = entry.path().join("memory.md");
            if memory.is_file() {
                files.push(memory);
            }
        }
    }
    Ok(files)
}

fn split_chunks(raw: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let extra = usize::from(!current.is_empty()) + line.chars().count();
        if !current.is_empty() && current.chars().count() + extra > CHUNK_CHARS {
            chunks.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

fn normalized_path_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn stable_path_id(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized_path_key(path).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_known_legacy_memory_files() {
        let root =
            std::env::temp_dir().join(format!("boris-memory-migration-{}", std::process::id()));
        let memory = root.join("memory");
        let sessions = root.join("sessions").join("desktop");
        fs::create_dir_all(sessions.join("one")).unwrap();
        fs::create_dir_all(memory.join("desktop")).unwrap();
        fs::write(memory.join("MEMORY.md"), "User likes Rust.").unwrap();
        fs::write(sessions.join("one").join("memory.md"), "We chose SQLite.").unwrap();
        fs::write(root.join("unrelated.md"), "do not import").unwrap();

        let plan = discover_legacy_memory(&LegacyMemoryPaths {
            profile_path: memory.join("profile.json"),
            memory_root: memory,
            sessions_root: sessions,
        })
        .unwrap();
        assert_eq!(plan.sources.len(), 2);
        assert!(plan
            .sources
            .iter()
            .all(|source| !source.path.ends_with("unrelated.md")));
        let _ = fs::remove_dir_all(root);
    }
}
