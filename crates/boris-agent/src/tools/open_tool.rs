//! Open URLs and local paths in the default OS handler.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

/// Open an http(s) URL in the default browser. Requires voice confirmation.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenUrlTool;

#[async_trait]
impl Tool for OpenUrlTool {
    fn name(&self) -> &str {
        "open_url"
    }

    fn description(&self) -> &str {
        "Open a web URL in the user's default browser. Requires user confirmation. Only http and https."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Full http or https URL to open"
                }
            },
            "required": ["url"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::Web)
            .permissions(&[Permission::UiControl])
            .confirm(true)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let url = require_string(obj, "url")?;
        let url = url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ToolError::invalid_args(
                "url must start with http:// or https://",
            ));
        }
        if url.len() > 2048 {
            return Err(ToolError::invalid_args("url too long"));
        }

        // `open` is sync; run off the async worker so we don't block the runtime badly.
        let url_owned = url.to_string();
        tokio::task::spawn_blocking(move || open::that(&url_owned))
            .await
            .map_err(|e| ToolError::failed(format!("open task failed: {e}")))?
            .map_err(|e| ToolError::failed(format!("failed to open url: {e}")))?;

        Ok(truncate_tool_result(format!("Opened URL: {url}")))
    }
}

/// Open a local path in the default app. Path must be under allowed read roots.
#[derive(Debug, Clone)]
pub struct OpenPathTool {
    read_roots: Vec<PathBuf>,
}

impl OpenPathTool {
    pub fn new(read_roots: Vec<PathBuf>) -> Self {
        Self { read_roots }
    }
}

#[async_trait]
impl Tool for OpenPathTool {
    fn name(&self) -> &str {
        "open_path"
    }

    fn description(&self) -> &str {
        "Open a local file or folder in the default app. Path must be under allowed directories. Requires user confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or root-relative path to open"
                }
            },
            "required": ["path"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::System)
            .permissions(&[Permission::FsRead, Permission::UiControl])
            .confirm(true)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let path = resolve_under_roots(&raw, &self.read_roots)?;
        if !path.exists() {
            return Err(ToolError::failed(format!(
                "path does not exist: {}",
                path.display()
            )));
        }

        let path_display = path.display().to_string();
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || open::that(path_clone))
            .await
            .map_err(|e| ToolError::failed(format!("open task failed: {e}")))?
            .map_err(|e| ToolError::failed(format!("failed to open path: {e}")))?;

        Ok(truncate_tool_result(format!("Opened path: {path_display}")))
    }
}
