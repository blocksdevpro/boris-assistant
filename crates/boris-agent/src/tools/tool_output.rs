//! `get_tool_output` — reread spilled truncated tool outputs.
//!
//! When a tool observation is truncated, the runtime spills the full text to
//! `{store_dir}/tool_*.log` and appends a hint. This read-only tool pages
//! that file back without re-running the original tool (voice "tell me more").

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_u64_strict, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolKind, ToolMeta, ToolRisk, ToolOutputStore, MAX_TOOL_RESULT_CHARS,
};

/// Read a spilled tool output file.
#[derive(Debug, Clone)]
pub struct GetToolOutputTool {
    dir: PathBuf,
}

impl GetToolOutputTool {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn with_dir(dir: &std::path::Path) -> Self {
        Self::new(dir.to_path_buf())
    }
}

#[async_trait]
impl Tool for GetToolOutputTool {
    fn name(&self) -> &str {
        "get_tool_output"
    }

    fn description(&self) -> &str {
        "Read a saved full tool output after truncation. Use when an observation says \
         full output was saved. Args: path (from the hint), offset (1-based char, default 1), \
         limit (max chars, default 12000). Read-only."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Spilled output file path from the truncation hint" },
                "offset": { "type": "integer", "description": "1-based char offset (default 1)" },
                "limit": { "type": "integer", "description": "Max chars (default 12000, max 256000)" }
            },
            "required": ["path"]
        })
    }

    fn meta(&self) -> ToolMeta {
        // `Other` (not `Read`) so VoiceSafe keeps reread ability like artifacts.
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Other)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        if raw.trim().is_empty() {
            return Err(ToolError::invalid_args("path is empty"));
        }
        let offset = optional_u64_strict(obj, "offset")?
            .map(|n| n as usize)
            .unwrap_or(1)
            .max(1);
        let limit = optional_u64_strict(obj, "limit")?
            .map(|n| n as usize)
            .unwrap_or(MAX_TOOL_RESULT_CHARS)
            .clamp(1, crate::tool::MAX_STORED_CHARS);

        // Resolve under the store dir (lexical + canonical containment in read()).
        let candidate = PathBuf::from(raw.trim());
        let path = if candidate.is_absolute() {
            candidate
        } else {
            self.dir.join(candidate)
        };
        let store = ToolOutputStore::new(&self.dir);
        let out = store
            .read(&path, offset, limit)
            .map_err(ToolError::failed)?;
        Ok(truncate_tool_result(out))
    }
}

/// Tools bound to an explicit output dir (session-local path).
pub fn output_tools_at(dir: &std::path::Path) -> Vec<Box<dyn Tool>> {
    vec![Box::new(GetToolOutputTool::with_dir(dir))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn reread_spilled_output() {
        let dir = std::env::temp_dir().join(format!("boris-getout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = ToolOutputStore::new(&dir);
        let path = store.save("bash", "hello world").unwrap();

        let tool = GetToolOutputTool::new(&dir);
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({"path": path.to_string_lossy()}),
            )
            .await
            .unwrap();
        assert!(out.contains("hello world"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rejects_escape() {
        let dir = std::env::temp_dir().join(format!("boris-getout-esc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let tool = GetToolOutputTool::new(&dir);
        let err = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({"path": "C:\\Windows\\System32\\config"}),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("outside") || err.message.contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
