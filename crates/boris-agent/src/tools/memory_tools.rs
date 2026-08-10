//! Cross-session markdown memory tools (`memory_search`, `memory_get`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::long_term::LongTermMemory;
use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Tool, ToolError,
    ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;

pub type SharedLongTermMemory = Arc<LongTermMemory>;

pub struct MemorySearchTool {
    memory: SharedLongTermMemory,
}

impl MemorySearchTool {
    pub fn new(memory: SharedLongTermMemory) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search markdown memory: global MEMORY.md plus each chat's session/{{id}}/memory.md turn logs. \
         Use when the user references prior work, prefs not in personal_context, or after a long gap. \
         Arg: query (required), max_results (optional 1–10)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Keywords to search for"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Max hits (default 5)"
                }
            },
            "required": ["query"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).kind(ToolKind::Memory)
            .read_only(true)
            .max_concurrency(8)
    }

    fn should_list(&self, _ctx: &crate::runtime::ListToolsContext) -> bool {
        true // soft-core when progressive listing is on
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let query = require_string(obj, "query")?;
        let max = obj
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(5);
        let hits = self
            .memory
            .search(&query, max)
            .map_err(ToolError::failed)?;
        if hits.is_empty() {
            return Ok(truncate_tool_result(format!(
                "No memory hits for: {query}"
            )));
        }
        let mut out = format!("{} hit(s) for: {query}\n", hits.len());
        for (i, h) in hits.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} (score {})\n   {}\n",
                i + 1,
                h.path,
                h.score,
                h.snippet
            ));
        }
        out.push_str("Use memory_get with a path above for full text.");
        Ok(truncate_tool_result(out))
    }
}

pub struct MemoryGetTool {
    memory: SharedLongTermMemory,
}

impl MemoryGetTool {
    pub fn new(memory: SharedLongTermMemory) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
    }

    fn description(&self) -> &str {
        "Read a markdown memory file by path from memory_search hits \
         (e.g. MEMORY.md, desktop/MEMORY.md, or session/{uuid}/memory.md). \
         Optional max_chars (default 6000)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Hit path: MEMORY.md or session/{id}/memory.md"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Max characters to return (default 6000)"
                }
            },
            "required": ["path"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).kind(ToolKind::Memory)
            .read_only(true)
            .max_concurrency(8)
    }

    fn should_list(&self, _ctx: &crate::runtime::ListToolsContext) -> bool {
        true // soft-core when progressive listing is on
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let path = require_string(obj, "path")?;
        let max = obj
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(6000);
        let _ = optional_string(obj, "unused");
        let body = self
            .memory
            .get(&path, max)
            .map_err(ToolError::failed)?;
        Ok(truncate_tool_result(format!(
            "<memory_file path=\"{}\">\n{body}\n</memory_file>",
            path.trim()
        )))
    }
}

pub fn memory_tools(memory: SharedLongTermMemory) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(MemorySearchTool::new(memory.clone())),
        Box::new(MemoryGetTool::new(memory)),
    ]
}
