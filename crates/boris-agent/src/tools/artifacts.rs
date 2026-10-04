//! Session-local visual cards: present / list / get.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::session::artifacts::{
    ArtifactKind, ArtifactMeta, ArtifactStore, PresentRequest, MAX_ARTIFACT_BODY_CHARS,
};
use crate::tool::{
    optional_bool, optional_string, require_object, require_string, truncate_tool_result,
    Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;

/// Tools bound to `{artifacts_dir}` (session-local or sandbox fallback).
pub fn artifact_tools_at(artifacts_dir: &Path) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(PresentArtifactTool::with_dir(artifacts_dir)),
        Box::new(ListArtifactsTool::with_dir(artifacts_dir)),
        Box::new(GetArtifactTool::with_dir(artifacts_dir)),
    ]
}

fn store_at(dir: &Path) -> ArtifactStore {
    ArtifactStore::new(dir)
}

/// Validate a model-supplied artifact `id` slug before policy/store lookup.
///
/// Accepts `alnum-hyphen-underscore`, 1–64 chars. Rejects empty, `..`, `/`,
/// `\`, and any other traversal/special characters with `invalid-args`.
/// Filenames (`{slug}-{hex}.{ext}`) are validated via [`is_safe_artifact_ref`].
pub fn is_safe_artifact_id(id: &str) -> bool {
    let t = id.trim();
    if t.is_empty() || t.len() > 64 {
        return false;
    }
    if t.contains("..") || t.contains('/') || t.contains('\\') {
        return false;
    }
    t.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Validate an artifact reference: either a bare [`is_safe_artifact_id`] slug
/// or a `{slug}-{hex}.{ext}` filename whose stem/extension are safe and whose
/// full string contains no traversal sequences.
pub fn is_safe_artifact_ref(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || t.len() > 128 {
        return false;
    }
    if t.contains("..") || t.contains('/') || t.contains('\\') {
        return false;
    }
    // Bare id.
    if is_safe_artifact_id(t) {
        return true;
    }
    // Filename: split optional extension, then require safe chars.
    let stem = t
        .rsplit_once('.')
        .filter(|(_, ext)| !ext.is_empty() && ext.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(|(s, _)| s)
        .unwrap_or(t);
    if stem.is_empty() || stem.len() > 96 {
        return false;
    }
    stem.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn reject_unsafe_artifact_ref(raw: &str) -> Result<(), ToolError> {
    if !is_safe_artifact_ref(raw) {
        return Err(ToolError::invalid_args(
            "invalid artifact id: use an id or filename returned by list_artifacts; traversal (.., /, \\) is rejected. To create a new card, omit id. To read the current card, omit id",
        ));
    }
    Ok(())
}

fn optional_artifact_id(obj: &serde_json::Map<String, Value>) -> Option<String> {
    optional_string(obj, "id")
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

async fn run_blocking<T: Send + 'static>(
    label: &'static str,
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ToolError::failed(format!("{label} task failed: {e}")))?
        .map_err(ToolError::failed)
}

fn format_meta_line(meta: &ArtifactMeta, current: Option<&str>) -> String {
    let kind = match meta.kind {
        ArtifactKind::Markdown => "markdown".to_string(),
        ArtifactKind::Code => match &meta.language {
            Some(lang) => format!("code/{lang}"),
            None => "code".into(),
        },
    };
    let pin = if meta.pinned { " pinned" } else { "" };
    let cur = if current == Some(meta.id.as_str()) {
        " current"
    } else {
        ""
    };
    format!(
        "- {} — {} ({kind}, rev {}, {}{pin}{cur})",
        meta.id, meta.title, meta.revision, meta.path
    )
}

fn present_receipt(out: &crate::session::artifacts::PresentedArtifact) -> String {
    let verb = if out.created { "Presented" } else { "Updated" };
    let kind = out.meta.kind.as_str();
    let lang = out
        .meta
        .language
        .as_deref()
        .map(|l| format!("/{l}"))
        .unwrap_or_default();
    format!(
        "{verb} {} · {} ({kind}{lang}) → {}\nDo not read this card aloud.",
        out.meta.id, out.meta.title, out.meta.path
    )
}

/// Create or revise a card. Body is stored on disk; the observation is a pointer.
#[derive(Debug, Clone)]
pub struct PresentArtifactTool {
    dir: PathBuf,
}

impl PresentArtifactTool {
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl Tool for PresentArtifactTool {
    fn name(&self) -> &str {
        "present_artifact"
    }

    fn description(&self) -> &str {
        "Show a visual card (markdown or code) on screen instead of speaking it. \
         Use for scripts, lists, drafts, recipes, tables, or anything the user \
         would want to copy or keep. Then speak 1–2 short sentences pointing at \
         the card — never read the body aloud. Omit id to create; pass a returned id to revise."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "description": "markdown or code"
                },
                "title": {
                    "type": "string",
                    "description": "Short human title (also used in the filename)"
                },
                "body": {
                    "type": "string",
                    "description": "Full card contents (not spoken)"
                },
                "language": {
                    "type": "string",
                    "description": "For kind=code: rust, python, powershell, …"
                },
                "id": {
                    "type": ["string", "null"],
                    "description": "Existing id or filename returned by list_artifacts to revise. Omit, null, or blank to create. Never invent an id."
                },
                "pinned": {
                    "type": "boolean",
                    "description": "Keep this card when a new one becomes current"
                }
            },
            "required": ["kind", "title", "body"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Other)
            .permissions(&[Permission::FsWrite])
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(&self, ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let kind_raw = require_string(obj, "kind")?;
        let kind = ArtifactKind::parse(&kind_raw)
            .ok_or_else(|| ToolError::invalid_args("kind must be markdown or code"))?;
        let title = require_string(obj, "title")?;
        let body = require_string(obj, "body")?;
        if title.trim().is_empty() {
            return Err(ToolError::invalid_args("title must not be empty"));
        }
        if body.trim().is_empty() {
            return Err(ToolError::invalid_args("body must not be empty"));
        }
        if body.chars().count() > MAX_ARTIFACT_BODY_CHARS {
            return Err(ToolError::truncated(format!(
                "artifact body exceeds {MAX_ARTIFACT_BODY_CHARS} characters"
            )));
        }
        let language = optional_string(obj, "language");
        let id = optional_artifact_id(obj);
        let pinned = optional_bool(obj, "pinned");
        let turn_id = ctx.turn_id.clone();
        let dir = self.dir.clone();
        let request = PresentRequest {
            id,
            title,
            kind,
            language,
            body,
            turn_id,
            pinned,
        };
        let validation = request
            .id
            .as_deref()
            .map(reject_unsafe_artifact_ref)
            .transpose();
        let attempt = request.clone();
        let result = match validation {
            Ok(_) => {
                run_blocking("present_artifact", move || store_at(&dir).present(attempt)).await
            }
            Err(error) => Err(error),
        };
        let out = match result {
            Ok(out) => out,
            Err(mut error) => {
                let dir = self.dir.clone();
                let recovery = request.clone();
                let saved = run_blocking("preserve_report", move || {
                    store_at(&dir).preserve_report(recovery)
                })
                .await;
                let meta = saved.as_ref().ok().map(|out| out.meta.clone());
                ctx.report(crate::runtime::ProgressEvent::ReportFallback {
                    meta,
                    title: request.title,
                    kind: request.kind.as_str().into(),
                    language: request.language,
                    body: request.body,
                });
                match saved {
                    Ok(saved) => error.message.push_str(&format!(
                        ". The host preserved this report as {} and delivered it on screen. Do not regenerate the body. At most one corrected presentation retry; otherwise finish with a short spoken status", saved.meta.id)),
                    Err(_) => error.message.push_str(
                        ". The host received the full report for on-screen fallback, but could not save it. Do not regenerate the body. At most one corrected presentation retry; otherwise finish with a short spoken status"),
                }
                return Err(error);
            }
        };

        Ok(truncate_tool_result(present_receipt(&out)))
    }
}

/// List card titles / ids for this session (no bodies).
#[derive(Debug, Clone)]
pub struct ListArtifactsTool {
    dir: PathBuf,
}

impl ListArtifactsTool {
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl Tool for ListArtifactsTool {
    fn name(&self) -> &str {
        "list_artifacts"
    }

    fn description(&self) -> &str {
        "List visual cards in this session (id, title, kind, file). \
         Does not return bodies — call get_artifact for contents."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Other)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(&self, _ctx: &ToolCallContext, _args: Value) -> Result<String, ToolError> {
        let dir = self.dir.clone();
        let index = run_blocking("list_artifacts", move || {
            store_at(&dir).load_display_index()
        })
        .await?;
        if index.items.is_empty() {
            return Ok("No artifacts.".into());
        }
        let current = index.current.clone();
        let mut lines = vec![format!(
            "{} artifact(s). Current: {}.",
            index.items.len(),
            current.as_deref().unwrap_or("(none)")
        )];
        for meta in &index.items {
            lines.push(format_meta_line(meta, current.as_deref()));
        }
        Ok(truncate_tool_result(lines.join("\n")))
    }
}

/// Load one card body by id (or the current card).
#[derive(Debug, Clone)]
pub struct GetArtifactTool {
    dir: PathBuf,
}

impl GetArtifactTool {
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl Tool for GetArtifactTool {
    fn name(&self) -> &str {
        "get_artifact"
    }

    fn description(&self) -> &str {
        "Read a session card's full body. Omit id to get the current card. \
         Use when revising or answering a question about a card — do not speak the whole body."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": ["string", "null"],
                    "description": "Existing artifact id or filename. Omit, null, or blank for the current card."
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Other)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let id = optional_artifact_id(obj);
        if let Some(ref raw) = id {
            reject_unsafe_artifact_ref(raw)?;
        }
        let dir = self.dir.clone();
        let (meta, body) = run_blocking("get_artifact", move || {
            store_at(&dir).get_display(id.as_deref())
        })
        .await?;
        Ok(truncate_tool_result(format!(
            "{} · {} ({})\nFile: {}\n\n{body}",
            meta.id,
            meta.title,
            meta.kind.as_str(),
            meta.path
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "boris-art-tools-{}-{}-{label}",
            std::process::id(),
            nanos
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn present_list_get_roundtrip() {
        let dir = temp_dir("roundtrip");
        let present = PresentArtifactTool::with_dir(&dir);
        let list = ListArtifactsTool::with_dir(&dir);
        let get = GetArtifactTool::with_dir(&dir);
        let ctx = ToolCallContext::new("c1").with_session(None, Some("turn-x".into()));

        let out = present
            .execute(
                &ctx,
                json!({
                    "kind": "code",
                    "title": "Rename photos",
                    "language": "powershell",
                    "body": "Get-ChildItem"
                }),
            )
            .await
            .unwrap();
        assert!(out.contains("Presented"));
        assert!(out.contains("rename-photos-"));
        assert!(out.contains(".ps1"));
        assert!(!out.contains("Get-ChildItem"));

        let listed = list.execute(&ctx, json!({})).await.unwrap();
        assert!(listed.contains("Rename photos"));
        assert!(listed.contains("current"));

        let got = get.execute(&ctx, json!({})).await.unwrap();
        assert!(got.contains("Get-ChildItem"));
        assert!(got.contains("Rename photos"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn present_rejects_bad_kind() {
        let dir = temp_dir("bad-kind");
        let present = PresentArtifactTool::with_dir(&dir);
        let err = present
            .execute(
                &ToolCallContext::new("c"),
                json!({"kind": "html", "title": "x", "body": "<p>"}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), crate::tool::ToolErrorKind::InvalidArgs);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn get_unknown_id_fails() {
        let dir = temp_dir("missing");
        let get = GetArtifactTool::with_dir(&dir);
        let err = get
            .execute(&ToolCallContext::new("c"), json!({"id": "a1f3c9"}))
            .await
            .unwrap_err();
        assert!(err.message.contains("unknown") || err.message.contains("no current"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn blank_and_null_ids_create_and_read_current() {
        let dir = temp_dir("optional-id");
        let present = PresentArtifactTool::with_dir(&dir);
        let get = GetArtifactTool::with_dir(&dir);
        let ctx = ToolCallContext::new("c");
        for id in [json!(""), json!(" \t "), Value::Null] {
            let args = json!({"kind":"markdown", "title":"Report", "body":"Evidence", "id":id});
            crate::tool::validate_args(&present.parameters(), &args, &args.to_string()).unwrap();
            assert!(present
                .execute(&ctx, args)
                .await
                .unwrap()
                .starts_with("Presented"));
            let args = json!({"id":id});
            crate::tool::validate_args(&get.parameters(), &args, &args.to_string()).unwrap();
            assert!(get.execute(&ctx, args).await.unwrap().contains("Evidence"));
        }
        assert_eq!(store_at(&dir).load_index().unwrap().items.len(), 3);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn unknown_update_has_creation_hint_without_creating_a_card() {
        let dir = temp_dir("unknown-update");
        let present = PresentArtifactTool::with_dir(&dir);
        let err = present
            .execute(
                &ToolCallContext::new("c"),
                json!({
                    "kind":"markdown", "title":"Report", "body":"Evidence", "id":"invented-id"
                }),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("omit id"));
        assert!(store_at(&dir).load_index().unwrap().items.is_empty());
        let recovered = store_at(&dir).load_display_index().unwrap();
        assert_eq!(recovered.items.len(), 1);
        assert!(recovered.items[0].id.starts_with("recovery-"));
        assert_eq!(store_at(&dir).get_display(None).unwrap().1, "Evidence");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn report_survives_corrupt_catalog_and_bypasses_progress_throttling() {
        let dir = temp_dir("recovery");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("index.json"), "broken catalog")
            .await
            .unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = crate::runtime::EventProgressSink::new(
            std::sync::Arc::new(move |event| {
                captured.lock().unwrap().push(event);
            }),
            "c",
            "present_artifact",
        )
        .into_arc();
        let ctx = ToolCallContext::new("c").with_progress(Some(sink));
        ctx.report_text("working");
        let err = PresentArtifactTool::with_dir(&dir)
            .execute(
                &ctx,
                json!({
                    "kind":"markdown", "title":"Audit", "body":"Full execution evidence"
                }),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("preserved this report"));
        assert!(events.lock().unwrap().iter().any(|event| matches!(event,
            crate::AgentEvent::ReportFallback { body, meta:Some(_), .. } if body == "Full execution evidence")));
        let reopened = store_at(&dir);
        let index = reopened.load_display_index().unwrap();
        let id = index.current.as_deref().unwrap();
        assert_eq!(
            reopened.get_display(Some(id)).unwrap().1,
            "Full execution evidence"
        );
        assert_eq!(
            tokio::fs::read_to_string(dir.join("index.json"))
                .await
                .unwrap(),
            "broken catalog"
        );
        // Recovery cards can be revised by their returned reference.
        reopened
            .present(PresentRequest {
                id: Some(id.into()),
                title: "Audit".into(),
                kind: ArtifactKind::Markdown,
                language: None,
                body: "Revised evidence".into(),
                turn_id: None,
                pinned: None,
            })
            .unwrap();
        assert_eq!(
            reopened.get_display(Some(id)).unwrap().1,
            "Revised evidence"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn unsaved_report_still_reaches_host_when_storage_is_unavailable() {
        let dir = temp_dir("unwritable");
        tokio::fs::write(&dir, "not a directory").await.unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink = crate::runtime::EventProgressSink::new(
            std::sync::Arc::new(move |event| {
                captured.lock().unwrap().push(event);
            }),
            "c",
            "present_artifact",
        )
        .into_arc();
        let ctx = ToolCallContext::new("c").with_progress(Some(sink));
        let err = PresentArtifactTool::with_dir(&dir)
            .execute(
                &ctx,
                json!({
                    "kind":"markdown", "title":"Audit", "body":"Unsaved execution evidence"
                }),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("could not save"));
        assert!(events.lock().unwrap().iter().any(|event| matches!(event,
            crate::AgentEvent::ReportFallback { body, meta:None, .. } if body == "Unsaved execution evidence")));
        let _ = tokio::fs::remove_file(&dir).await;
    }

    #[tokio::test]
    async fn damaged_recovery_catalog_does_not_hide_healthy_primary_cards() {
        let dir = temp_dir("damaged-recovery");
        let ctx = ToolCallContext::new("c");
        PresentArtifactTool::with_dir(&dir)
            .execute(
                &ctx,
                json!({
                    "kind":"markdown", "title":"Primary card", "body":"Verified content"
                }),
            )
            .await
            .unwrap();
        tokio::fs::create_dir_all(dir.join("recovery"))
            .await
            .unwrap();
        tokio::fs::write(dir.join("recovery/index.json"), "broken")
            .await
            .unwrap();
        assert!(ListArtifactsTool::with_dir(&dir)
            .execute(&ctx, json!({}))
            .await
            .unwrap()
            .contains("Primary card"));
        assert!(GetArtifactTool::with_dir(&dir)
            .execute(&ctx, json!({}))
            .await
            .unwrap()
            .contains("Verified content"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn safe_id_accepts_slug_rejects_traversal() {
        assert!(is_safe_artifact_id("abc123"));
        assert!(is_safe_artifact_id("my-note_1"));
        assert!(!is_safe_artifact_id(""));
        assert!(!is_safe_artifact_id("../evil"));
        assert!(!is_safe_artifact_id("a/b"));
        assert!(!is_safe_artifact_id("a\\b"));
        assert!(!is_safe_artifact_id("a..b"));
        assert!(!is_safe_artifact_id(&"x".repeat(65)));
        assert!(is_safe_artifact_ref("rename-photos-a1f3c9.ps1"));
        assert!(!is_safe_artifact_ref("../../etc/passwd"));
        assert!(!is_safe_artifact_ref(".."));
    }

    #[tokio::test]
    async fn get_rejects_traversal_with_invalid_args() {
        let dir = temp_dir("traversal");
        let get = GetArtifactTool::with_dir(&dir);
        for evil in ["../evil", "..", "a/b", "a\\b", "../../etc/passwd"] {
            let err = get
                .execute(&ToolCallContext::new("c"), json!({"id": evil}))
                .await
                .unwrap_err();
            assert_eq!(
                err.kind(),
                crate::tool::ToolErrorKind::InvalidArgs,
                "id={evil}: {}",
                err.message
            );
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn present_rejects_empty_title_and_body() {
        let dir = temp_dir("empty");
        let present = PresentArtifactTool::with_dir(&dir);
        for args in [
            json!({"kind": "markdown", "title": "", "body": "hi"}),
            json!({"kind": "markdown", "title": "   ", "body": "hi"}),
            json!({"kind": "markdown", "title": "x", "body": ""}),
            json!({"kind": "markdown", "title": "x", "body": "   "}),
        ] {
            let err = present
                .execute(&ToolCallContext::new("c"), args)
                .await
                .unwrap_err();
            assert_eq!(err.kind(), crate::tool::ToolErrorKind::InvalidArgs);
            assert!(
                err.message.contains("must not be empty"),
                "got: {}",
                err.message
            );
        }
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn present_rejects_traversal_id() {
        let dir = temp_dir("present-traversal");
        let present = PresentArtifactTool::with_dir(&dir);
        let err = present
            .execute(
                &ToolCallContext::new("c"),
                json!({"kind": "markdown", "title": "x", "body": "hi", "id": "../evil"}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), crate::tool::ToolErrorKind::InvalidArgs);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
