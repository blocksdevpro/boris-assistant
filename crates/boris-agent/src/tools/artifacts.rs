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
         the card — never read the body aloud. Pass id to revise the current/same card."
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
                    "type": "string",
                    "description": "Existing artifact id to revise (omit to create)"
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
        if body.chars().count() > MAX_ARTIFACT_BODY_CHARS {
            return Err(ToolError::truncated(format!(
                "artifact body exceeds {MAX_ARTIFACT_BODY_CHARS} characters"
            )));
        }
        let language = optional_string(obj, "language");
        let id = optional_string(obj, "id");
        let pinned = optional_bool(obj, "pinned");
        let turn_id = ctx.turn_id.clone();
        let dir = self.dir.clone();

        let out = run_blocking("present_artifact", move || {
            store_at(&dir).present(PresentRequest {
                id,
                title,
                kind,
                language,
                body,
                turn_id,
                pinned,
            })
        })
        .await?;

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
        let index = run_blocking("list_artifacts", move || store_at(&dir).load_index()).await?;
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
                    "type": "string",
                    "description": "Artifact id or filename. Omit for the current card."
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
        let id = optional_string(obj, "id");
        let dir = self.dir.clone();
        let (meta, body) =
            run_blocking("get_artifact", move || store_at(&dir).get(id.as_deref())).await?;
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
}
