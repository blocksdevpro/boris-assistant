//! `file_write` — create or overwrite files under write roots.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

use super::{FsRoots, MAX_WRITE_BYTES};

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Validate write content size against the hard byte budget.
pub(crate) fn validate_write_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_WRITE_BYTES {
        return Err(format!("content exceeds {MAX_WRITE_BYTES} bytes"));
    }
    Ok(())
}

/// Success message after a write.
pub(crate) fn format_write_result(created: bool, byte_len: usize, path_display: &str) -> String {
    let action = if created { "Created" } else { "Wrote" };
    format!("{action} {byte_len} bytes to {path_display}")
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// Write (create/overwrite) a file under write-allowed roots.
#[derive(Debug, Clone)]
pub struct WriteFileTool {
    roots: FsRoots,
}

impl WriteFileTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file under the write sandbox (default ~/.boris/sandbox), \
         creating parent directories as needed. Overwrites by default. Requires confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to write"
                },
                "content": {
                    "type": "string",
                    "description": "Full file content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::Write)
            .permissions(&[Permission::FsWrite])
            .confirm(true)
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let content = require_string(obj, "content")?;

        validate_write_content(&content).map_err(ToolError::invalid_args)?;

        let path = resolve_under_roots(&raw, &self.roots.writers())?;
        let created = !path.exists();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::failed(format!("create parent dirs: {e}")))?;
        }
        tokio::fs::write(&path, content.as_bytes())
            .await
            .map_err(|e| ToolError::failed(format!("write {}: {e}", path.display())))?;

        Ok(truncate_tool_result(format_write_result(
            created,
            content.len(),
            &path.display().to_string(),
        )))
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_write_content_ok() {
        assert!(validate_write_content("hello").is_ok());
        assert!(validate_write_content("").is_ok());
    }

    #[test]
    fn validate_write_content_rejects_oversize() {
        let big = "x".repeat(MAX_WRITE_BYTES + 1);
        let err = validate_write_content(&big).unwrap_err();
        assert!(err.contains("exceeds"));
        assert!(err.contains(&MAX_WRITE_BYTES.to_string()));
    }

    #[test]
    fn format_write_result_created_vs_wrote() {
        assert_eq!(
            format_write_result(true, 12, "/s/a.txt"),
            "Created 12 bytes to /s/a.txt"
        );
        assert_eq!(
            format_write_result(false, 3, "/s/b.txt"),
            "Wrote 3 bytes to /s/b.txt"
        );
    }

    #[tokio::test]
    async fn write_creates_and_overwrites() {
        let (roots, dir) = crate::tools::files::test_util::temp_roots();
        let write = WriteFileTool::new(roots);

        let msg = write
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({
                    "path": "nested/hello.txt",
                    "content": "hi boris\n"
                }),
            )
            .await
            .unwrap();
        assert!(msg.contains("Created"));
        assert!(msg.contains("hello.txt"));

        let body = std::fs::read_to_string(dir.join("nested/hello.txt")).unwrap();
        assert_eq!(body, "hi boris\n");

        let msg2 = write
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({
                    "path": "nested/hello.txt",
                    "content": "overwrite"
                }),
            )
            .await
            .unwrap();
        assert!(msg2.contains("Wrote"));

        let body2 = std::fs::read_to_string(dir.join("nested/hello.txt")).unwrap();
        assert_eq!(body2, "overwrite");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
