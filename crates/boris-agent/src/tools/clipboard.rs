//! System clipboard get/set (voice-friendly).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolMeta,
    ToolRisk,
};

const MAX_CLIP_CHARS: usize = 2000;

/// Read text from the system clipboard.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClipboardGetTool;

#[async_trait]
impl Tool for ClipboardGetTool {
    fn name(&self) -> &str {
        "clipboard_get"
    }

    fn description(&self) -> &str {
        "Read the current text from the system clipboard."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::Clipboard])
    }

    async fn execute(&self, _args: Value) -> Result<String, ToolError> {
        let text = tokio::task::spawn_blocking(|| {
            let mut clip = arboard::Clipboard::new()
                .map_err(|e| format!("clipboard unavailable: {e}"))?;
            clip.get_text()
                .map_err(|e| format!("clipboard get failed: {e}"))
        })
        .await
        .map_err(|e| ToolError::failed(format!("clipboard task failed: {e}")))?
        .map_err(ToolError::failed)?;

        if text.is_empty() {
            return Ok("Clipboard is empty.".into());
        }
        let truncated = if text.chars().count() > MAX_CLIP_CHARS {
            let head: String = text.chars().take(MAX_CLIP_CHARS).collect();
            format!("{head}\n…[clipboard truncated]")
        } else {
            text
        };
        Ok(truncate_tool_result(format!("Clipboard text:\n{truncated}")))
    }
}

/// Write text to the system clipboard.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClipboardSetTool;

#[async_trait]
impl Tool for ClipboardSetTool {
    fn name(&self) -> &str {
        "clipboard_set"
    }

    fn description(&self) -> &str {
        "Copy short text onto the system clipboard so the user can paste it."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to place on the clipboard"
                }
            },
            "required": ["text"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::Clipboard])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let text = require_string(obj, "text")?;
        if text.chars().count() > 20_000 {
            return Err(ToolError::invalid_args("text too long for clipboard"));
        }
        let text_clone = text.clone();
        tokio::task::spawn_blocking(move || {
            let mut clip = arboard::Clipboard::new()
                .map_err(|e| format!("clipboard unavailable: {e}"))?;
            clip.set_text(text_clone)
                .map_err(|e| format!("clipboard set failed: {e}"))
        })
        .await
        .map_err(|e| ToolError::failed(format!("clipboard task failed: {e}")))?
        .map_err(ToolError::failed)?;

        Ok(truncate_tool_result(format!(
            "Copied {} characters to clipboard.",
            text.chars().count()
        )))
    }
}
