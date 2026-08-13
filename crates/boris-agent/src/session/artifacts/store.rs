//! Filesystem catalog + body files under `{session}/artifacts/`.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::id::{generate_artifact_id, normalize_artifact_id};
use super::slug::{artifact_filename, extension_for, slugify};
use super::{
    ArtifactIndex, ArtifactKind, ArtifactMeta, PresentRequest, PresentedArtifact, MAX_ARTIFACTS,
    MAX_ARTIFACT_BODY_CHARS, MAX_TITLE_CHARS,
};

/// Session-local artifact catalog (`{dir}/index.json` + named body files).
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    dir: PathBuf,
}

impl ArtifactStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    /// Create the directory and an empty index when missing.
    pub fn ensure(&self) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| format!("create artifacts dir: {e}"))?;
        let index = self.index_path();
        if !index.is_file() {
            self.write_index(&ArtifactIndex::default())?;
        }
        Ok(())
    }

    pub fn load_index(&self) -> Result<ArtifactIndex, String> {
        let path = self.index_path();
        if !path.is_file() {
            return Ok(ArtifactIndex::default());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read artifacts index {}: {e}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(ArtifactIndex::default());
        }
        serde_json::from_str(&raw)
            .map_err(|e| format!("parse artifacts index {}: {e}", path.display()))
    }

    /// Create a new card or replace the body of an existing one.
    pub fn present(&self, req: PresentRequest) -> Result<PresentedArtifact, String> {
        let title = normalize_title(&req.title)?;
        let body = normalize_body(&req.body)?;
        let kind = req.kind;
        let language = normalize_language(kind, req.language.as_deref());

        self.ensure()?;
        let mut index = self.load_index()?;

        let (meta, created) = if let Some(raw_id) = req.id.as_deref() {
            let existing = resolve_meta(&index, raw_id)
                .ok_or_else(|| format!("unknown artifact id `{raw_id}`"))?
                .clone();
            let mut meta = existing;
            meta.title = title;
            meta.kind = kind;
            meta.language = language.clone();
            meta.updated_at = now_rfc3339();
            meta.revision = meta.revision.saturating_add(1);
            if let Some(turn) = req.turn_id.clone() {
                meta.turn = Some(turn);
            }
            if let Some(pinned) = req.pinned {
                meta.pinned = pinned;
            }
            (meta, false)
        } else {
            if index.items.len() >= MAX_ARTIFACTS {
                return Err(format!("max {MAX_ARTIFACTS} artifacts per session"));
            }
            let id = unique_id(&index);
            let slug = slugify(&title);
            let ext = extension_for(kind, language.as_deref());
            let filename = artifact_filename(&slug, &id, ext);
            let now = now_rfc3339();
            let meta = ArtifactMeta {
                id,
                title,
                kind,
                language,
                path: filename,
                turn: req.turn_id.clone(),
                created_at: now.clone(),
                updated_at: now,
                pinned: req.pinned.unwrap_or(false),
                revision: 1,
            };
            (meta, true)
        };

        let dest = self.dir.join(&meta.path);
        write_atomic_file(&dest, body.as_bytes())
            .map_err(|e| format!("write artifact {}: {e}", dest.display()))?;

        if created {
            index.items.push(meta.clone());
        } else if let Some(slot) = index.items.iter_mut().find(|m| m.id == meta.id) {
            *slot = meta.clone();
        }
        index.current = Some(meta.id.clone());
        self.write_index(&index)?;

        Ok(PresentedArtifact { meta, created })
    }

    /// Load meta + body. `id` of `None` uses the current card.
    pub fn get(&self, id: Option<&str>) -> Result<(ArtifactMeta, String), String> {
        let index = self.load_index()?;
        let meta = match id {
            Some(raw) => resolve_meta(&index, raw)
                .cloned()
                .ok_or_else(|| format!("unknown artifact id `{raw}`"))?,
            None => {
                let current = index
                    .current
                    .as_deref()
                    .ok_or_else(|| "no current artifact in this session".to_string())?;
                index
                    .get(current)
                    .cloned()
                    .ok_or_else(|| format!("current artifact `{current}` missing from index"))?
            }
        };
        let path = self.dir.join(&meta.path);
        let body = fs::read_to_string(&path)
            .map_err(|e| format!("read artifact {}: {e}", path.display()))?;
        Ok((meta, body))
    }
}

impl ArtifactStore {
    fn write_index(&self, index: &ArtifactIndex) -> Result<(), String> {
        if let Some(parent) = self.index_path().parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create artifacts dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(index)
            .map_err(|e| format!("serialize artifacts index: {e}"))?;
        write_atomic_file(&self.index_path(), json.as_bytes())
            .map_err(|e| format!("write artifacts index: {e}"))
    }
}

fn unique_id(index: &ArtifactIndex) -> String {
    for _ in 0..16 {
        let id = generate_artifact_id();
        if index.get(&id).is_none() {
            return id;
        }
    }
    // Extremely unlikely; fold in item count so we still return something unique.
    let extra = index.items.len() as u32;
    let seed = u32::from_str_radix(&generate_artifact_id(), 16).unwrap_or(0);
    format!("{:06x}", (seed ^ extra) & 0x00FF_FFFF)
}

fn resolve_meta<'a>(index: &'a ArtifactIndex, raw: &str) -> Option<&'a ArtifactMeta> {
    let trimmed = raw.trim();
    if let Some(id) = normalize_artifact_id(trimmed) {
        return index.get(&id);
    }
    // Filename or `{slug}-{id}` — take the last `-` segment, strip extension.
    let stem = trimmed
        .rsplit_once('.')
        .filter(|(_, ext)| !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(|(s, _)| s)
        .unwrap_or(trimmed);
    if let Some((_, maybe_id)) = stem.rsplit_once('-') {
        if let Some(id) = normalize_artifact_id(maybe_id) {
            return index.get(&id);
        }
    }
    None
}

fn normalize_title(title: &str) -> Result<String, String> {
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Ok("Untitled".into());
    }
    if t.chars().count() > MAX_TITLE_CHARS {
        let clipped: String = t.chars().take(MAX_TITLE_CHARS).collect();
        return Ok(clipped);
    }
    Ok(t)
}

fn normalize_body(body: &str) -> Result<String, String> {
    if body.trim().is_empty() {
        return Err("artifact body is empty".into());
    }
    if body.chars().count() > MAX_ARTIFACT_BODY_CHARS {
        return Err(format!(
            "artifact body exceeds {MAX_ARTIFACT_BODY_CHARS} characters"
        ));
    }
    Ok(body.to_string())
}

fn normalize_language(kind: ArtifactKind, language: Option<&str>) -> Option<String> {
    if matches!(kind, ArtifactKind::Markdown) {
        return None;
    }
    language
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Sibling `{filename}.tmp` then rename (works for `.md` / `.ps1`, not only JSON).
fn write_atomic_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = match path.file_name() {
        Some(name) => {
            let mut tmp_name = name.to_os_string();
            tmp_name.push(".tmp");
            path.with_file_name(tmp_name)
        }
        None => path.with_extension("tmp"),
    };
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_e) => {
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("boris-art-store-{nanos}-{n}-{label}"));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn present_code(store: &ArtifactStore, title: &str, body: &str) -> PresentedArtifact {
        store
            .present(PresentRequest {
                id: None,
                title: title.into(),
                kind: ArtifactKind::Code,
                language: Some("powershell".into()),
                body: body.into(),
                turn_id: Some("turn-1".into()),
                pinned: None,
            })
            .expect("present")
    }

    #[test]
    fn present_writes_named_file_and_index() {
        let dir = temp_dir("create");
        let store = ArtifactStore::new(&dir);
        let out = present_code(&store, "Rename photos", "Get-ChildItem");

        assert!(out.created);
        assert_eq!(out.meta.revision, 1);
        assert_eq!(out.meta.kind, ArtifactKind::Code);
        assert_eq!(out.meta.language.as_deref(), Some("powershell"));
        assert!(
            out.meta.path.starts_with("rename-photos-"),
            "path={}",
            out.meta.path
        );
        assert!(out.meta.path.ends_with(".ps1"), "path={}", out.meta.path);
        assert!(out.meta.path.contains(&out.meta.id));

        let body = fs::read_to_string(dir.join(&out.meta.path)).unwrap();
        assert_eq!(body, "Get-ChildItem");

        let index = store.load_index().unwrap();
        assert_eq!(index.current.as_deref(), Some(out.meta.id.as_str()));
        assert_eq!(index.items.len(), 1);

        cleanup(&dir);
    }

    #[test]
    fn present_same_id_overwrites_body_keeps_filename() {
        let dir = temp_dir("update");
        let store = ArtifactStore::new(&dir);
        let first = present_code(&store, "Rename photos", "v1");
        let path = first.meta.path.clone();

        let second = store
            .present(PresentRequest {
                id: Some(first.meta.id.clone()),
                title: "Photo renamer".into(),
                kind: ArtifactKind::Code,
                language: Some("powershell".into()),
                body: "v2".into(),
                turn_id: None,
                pinned: Some(true),
            })
            .unwrap();

        assert!(!second.created);
        assert_eq!(second.meta.revision, 2);
        assert_eq!(second.meta.path, path);
        assert_eq!(second.meta.title, "Photo renamer");
        assert!(second.meta.pinned);
        assert_eq!(fs::read_to_string(dir.join(&path)).unwrap(), "v2");
        assert_eq!(store.load_index().unwrap().items.len(), 1);

        cleanup(&dir);
    }

    #[test]
    fn get_by_filename_and_current() {
        let dir = temp_dir("get");
        let store = ArtifactStore::new(&dir);
        let out = present_code(&store, "Rename photos", "body");

        let (meta, body) = store.get(None).unwrap();
        assert_eq!(meta.id, out.meta.id);
        assert_eq!(body, "body");

        let (meta2, _) = store.get(Some(&out.meta.path)).unwrap();
        assert_eq!(meta2.id, out.meta.id);

        cleanup(&dir);
    }

    #[test]
    fn unknown_id_errors() {
        let dir = temp_dir("missing");
        let store = ArtifactStore::new(&dir);
        let err = store
            .present(PresentRequest {
                id: Some("a1f3c9".into()),
                title: "x".into(),
                kind: ArtifactKind::Markdown,
                language: None,
                body: "hi".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap_err();
        assert!(err.contains("unknown"), "{err}");
        cleanup(&dir);
    }

    #[test]
    fn empty_body_rejected() {
        let dir = temp_dir("empty");
        let store = ArtifactStore::new(&dir);
        let err = store
            .present(PresentRequest {
                id: None,
                title: "x".into(),
                kind: ArtifactKind::Markdown,
                language: None,
                body: "   ".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap_err();
        assert!(err.contains("empty"), "{err}");
        cleanup(&dir);
    }

    #[test]
    fn markdown_file_is_md() {
        let dir = temp_dir("md");
        let store = ArtifactStore::new(&dir);
        let out = store
            .present(PresentRequest {
                id: None,
                title: "Weekly meal plan".into(),
                kind: ArtifactKind::Markdown,
                language: Some("rust".into()),
                body: "# Dinner".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap();
        assert!(out.meta.path.ends_with(".md"));
        assert!(out.meta.language.is_none());
        cleanup(&dir);
    }
}
