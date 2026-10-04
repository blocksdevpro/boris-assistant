//! Map the session artifact catalog onto a UI peek (no body).

use boris_agent::{SessionId, SessionStore};

use crate::status::ArtifactPeek;

pub(super) fn apply_report_fallback(
    latest: &mut crate::StatusPicture,
    turn: &str,
    event: &boris_agent::AgentEvent,
) -> bool {
    let boris_agent::AgentEvent::ReportFallback {
        meta,
        title,
        kind,
        language,
        body,
    } = event
    else {
        return false;
    };
    if latest.turn.as_deref() != Some(turn) {
        return false;
    }
    let card = crate::artifacts::ArtifactCard {
        id: meta
            .as_ref()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| format!("unsaved-report-{turn}")),
        title: title.clone(),
        kind: kind.clone(),
        language: language.clone(),
        path: meta.as_ref().map(|m| m.path.clone()).unwrap_or_default(),
        pinned: false,
        revision: 1,
        body: body.clone(),
    };
    latest.artifact = Some(ArtifactPeek {
        id: card.id.clone(),
        title: card.title.clone(),
        kind: card.kind.clone(),
        language: card.language.clone(),
        path: card.path.clone(),
    });
    latest.fallback_report = Some(card);
    true
}

/// Current card in this session, if the catalog has one.
pub(super) fn peek_current(store: &SessionStore, id: &SessionId) -> Option<ArtifactPeek> {
    let index = boris_agent::ArtifactStore::new(store.artifacts_dir(id))
        .load_display_index()
        .ok()?;
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
    use boris_agent::{ArtifactKind, ArtifactStore, PresentRequest};

    #[test]
    fn fallback_contains_full_body_and_rejects_an_old_turn() {
        let event = boris_agent::AgentEvent::ReportFallback {
            meta: None,
            title: "Execution report".into(),
            kind: "markdown".into(),
            language: None,
            body: "Full evidence\n".repeat(100),
        };
        let mut latest = crate::StatusPicture::off();
        latest.turn = Some("current".into());
        assert!(!apply_report_fallback(&mut latest, "old", &event));
        assert!(latest.fallback_report.is_none());
        assert!(apply_report_fallback(&mut latest, "current", &event));
        assert_eq!(
            latest.fallback_report.unwrap().body,
            "Full evidence\n".repeat(100)
        );
        assert_eq!(latest.artifact.unwrap().id, "unsaved-report-current");
    }

    #[test]
    fn recovered_card_is_available_after_reopening_a_damaged_catalog() {
        let root = std::env::temp_dir().join(format!(
            "boris-peek-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = SessionStore::new(&root);
        let meta = store.create().unwrap();
        let cards = ArtifactStore::new(store.artifacts_dir(&meta.id));
        std::fs::write(cards.index_path(), "broken").unwrap();
        let recovered = cards
            .preserve_report(PresentRequest {
                id: None,
                title: "Recovery".into(),
                kind: ArtifactKind::Markdown,
                language: None,
                body: "Tool evidence".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap();
        let peek = peek_current(&SessionStore::new(&root), &meta.id).unwrap();
        assert_eq!(peek.id, recovered.meta.id);
        assert_eq!(
            cards.get_display(Some(&peek.id)).unwrap().1,
            "Tool evidence"
        );
        let _ = std::fs::remove_dir_all(root);
    }

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
