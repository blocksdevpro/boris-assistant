//! `remember_note` tool — append one note to the JSONL store.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

use super::store::NotesStore;

/// LLM tool: persist a short note to the local notes file.
#[derive(Debug, Clone)]
pub struct RememberNoteTool {
    store: NotesStore,
}

impl RememberNoteTool {
    /// Build a tool writing to the given JSONL path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            store: NotesStore::new(path),
        }
    }

    /// Shared store (path) this tool writes to.
    pub fn store(&self) -> &NotesStore {
        &self.store
    }
}

#[async_trait]
impl Tool for RememberNoteTool {
    fn name(&self) -> &str {
        "remember_note"
    }

    fn description(&self) -> &str {
        "Save a short note to local memory for later recall. Use for facts, reminders, or preferences the user wants remembered."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "The note text to remember"
                }
            },
            "required": ["note"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let note = require_string(obj, "note")?;
        self.store
            .append(&note)
            .map_err(|e| ToolError::failed(format!("failed to save note: {e}")))?;
        Ok(truncate_tool_result("Saved note.".to_string()))
    }
}
