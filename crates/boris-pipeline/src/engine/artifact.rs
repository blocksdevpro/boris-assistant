//! Map the session artifact catalog onto a UI peek (no body).

use boris_agent::session::types::SessionId;
use boris_agent::session::SessionStore;

use crate::status::ArtifactPeek;

/// Current card in this session, if the catalog has one.
pub(super) fn peek_current(store: &SessionStore, id: &SessionId) -> Option<ArtifactPeek> {
    let index = store.load_artifact_index(id).ok()?;
    let current = index.current.as_deref()?;
    let meta = index.get(current)?;
    Some(ArtifactPeek {
        id: meta.id.clone(),
        title: meta.title.clone(),
        kind: meta.kind.as_str().to_string(),
        language: meta.language.clone(),
        path: meta.path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use boris_agent::session::artifacts::{ArtifactKind, ArtifactStore, PresentRequest};

    #[test]
    fn peek_none_when_empty() {
        let root = std::env::temp_dir().join(format!(
            "boris-peek-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let store = SessionStore::new(&root);
        let meta = store.create().unwrap();
        assert!(peek_current(&store, &meta.id).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn peek_reads_current_card() {
        let root = std::env::temp_dir().join(format!(
            "boris-peek-card-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let store = SessionStore::new(&root);
        let meta = store.create().unwrap();
        ArtifactStore::new(store.artifacts_dir(&meta.id))
            .present(PresentRequest {
                id: None,
                title: "Rename photos".into(),
                kind: ArtifactKind::Code,
                language: Some("powershell".into()),
                body: "Get-ChildItem".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap();

        let peek = peek_current(&store, &meta.id).expect("peek");
        assert_eq!(peek.title, "Rename photos");
        assert_eq!(peek.kind, "code");
        assert_eq!(peek.language.as_deref(), Some("powershell"));
        assert!(peek.path.contains(&peek.id));
        let _ = std::fs::remove_dir_all(&root);
    }
}
