//! `recall_notes` tool — list recent or search notes.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

use super::format::{format_notes_list, parse_limit};
use super::store::NotesStore;

/// LLM tool: list recent notes or search by substring.
#[derive(Debug, Clone)]
pub struct RecallNotesTool {
    store: NotesStore,
}

impl RecallNotesTool {
    /// Build a tool reading from the given JSONL path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            store: NotesStore::new(path),
        }
    }

    /// Shared store (path) this tool reads from.
    pub fn store(&self) -> &NotesStore {
        &self.store
    }
}

#[async_trait]
impl Tool for RecallNotesTool {
    fn name(&self) -> &str {
        "recall_notes"
    }

    fn description(&self) -> &str {
        "Recall notes from local memory. Optionally filter with a case-insensitive query; otherwise returns the most recent notes."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional case-insensitive substring to search for"
                },
                "limit": {
                    "type": "number",
                    "description": "Max notes to return (default 5, max 20)"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Memory)
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
        let limit = parse_limit(obj)?;
        let query = optional_string(obj, "query");

        let notes = match query {
            Some(q) if !q.trim().is_empty() => self
                .store
                .search(q.trim(), limit)
                .map_err(|e| ToolError::failed(format!("failed to search notes: {e}")))?,
            _ => self
                .store
                .list_recent(limit)
                .map_err(|e| ToolError::failed(format!("failed to list notes: {e}")))?,
        };

        Ok(truncate_tool_result(format_notes_list(&notes)))
    }
}
