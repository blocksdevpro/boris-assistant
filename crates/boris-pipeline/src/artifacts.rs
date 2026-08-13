//! Host-facing session card list / get (no tool execution).

use boris_agent::session::{ArtifactStore, SessionStore};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactListItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub path: String,
    pub pinned: bool,
    pub revision: u32,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCard {
    pub id: String,
    pub title: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub path: String,
    pub pinned: bool,
    pub revision: u32,
    pub body: String,
}

fn open_current() -> Result<(SessionStore, boris_agent::SessionId), String> {
    let store = SessionStore::new(paths::sessions_dir());
    let id = store
        .current_id()
        .map_err(|e| format!("read current session: {e}"))?
        .ok_or_else(|| "no active session".to_string())?;
    Ok((store, id))
}

/// Cards in the active session. No current session → empty list (not an error).
pub fn list_session_artifacts() -> Result<Vec<ArtifactListItem>, String> {
    let (store, id) = match open_current() {
        Ok(pair) => pair,
        Err(_) => return Ok(Vec::new()),
    };
    let index = store.load_artifact_index(&id)?;
    let current = index.current.clone();
    Ok(index
        .items
        .into_iter()
        .map(|m| ArtifactListItem {
            current: current.as_deref() == Some(m.id.as_str()),
            id: m.id,
            title: m.title,
            kind: m.kind.as_str().to_string(),
            language: m.language,
            path: m.path,
            pinned: m.pinned,
            revision: m.revision,
        })
        .collect())
}

/// One card body. `id` omitted → current card.
pub fn get_session_artifact(id: Option<&str>) -> Result<ArtifactCard, String> {
    let (store, sid) = open_current()?;
    let arts = ArtifactStore::new(store.artifacts_dir(&sid));
    let (meta, body) = arts.get(id)?;
    Ok(ArtifactCard {
        id: meta.id,
        title: meta.title,
        kind: meta.kind.as_str().to_string(),
        language: meta.language,
        path: meta.path,
        pinned: meta.pinned,
        revision: meta.revision,
        body,
    })
}
