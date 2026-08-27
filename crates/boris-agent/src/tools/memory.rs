//! Tools for Boris's canonical SQLite memory store.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::MemoryStore;
use crate::tool::{
    optional_bool, optional_string, require_object, require_string, truncate_tool_result,
    Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;

pub type SharedMemoryStore = Arc<MemoryStore>;

pub struct MemorySearchTool {
    store: SharedMemoryStore,
}

impl MemorySearchTool {
    pub fn new(store: SharedMemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search Boris's durable memory for current evidence-backed facts, projects, decisions, and past events. Use when prior context matters. Do not use old conversations as proof unless this search returns a record."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to recall" },
                "max_results": { "type": "integer", "description": "1-10, default 5" }
            },
            "required": ["query"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let query = require_string(obj, "query")?;
        let max_results = obj
            .get("max_results")
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or(5)
            .clamp(1, 10);
        let hits = self
            .store
            .search(&query, max_results)
            .map_err(ToolError::failed)?;
        if hits.is_empty() {
            return Ok(format!("No active memories matched: {query}"));
        }
        let mut out = format!("{} active memory record(s):\n", hits.len());
        for (index, hit) in hits.iter().enumerate() {
            out.push_str(&format!(
                "{}. [{} | {} | confidence {:.0}%] {}\n   id: {}\n",
                index + 1,
                format_kind(hit.record.kind),
                format_scope(hit.record.scope),
                hit.record.confidence * 100.0,
                hit.record.text,
                hit.record.id,
            ));
        }
        Ok(truncate_tool_result(out))
    }
}

pub struct MemoryGetTool {
    store: SharedMemoryStore,
}

impl MemoryGetTool {
    pub fn new(store: SharedMemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
    }

    fn description(&self) -> &str {
        "Read one active memory record returned by memory_search. Use the exact record id."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "id": { "type": "string", "description": "Memory record id" } },
            "required": ["id"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let id = require_string(obj, "id")?;
        let Some(record) = self.store.get(&id).map_err(ToolError::failed)? else {
            return Ok("Memory record not found or no longer active.".into());
        };
        Ok(truncate_tool_result(format!(
            "<memory_record id=\"{}\" kind=\"{}\" scope=\"{}\">\n{}\n</memory_record>",
            record.id,
            format_kind(record.kind),
            format_scope(record.scope),
            record.text,
        )))
    }
}

/// Deliberately requires an explicit memory instruction from the human. This
/// route performs a real canonical/index deletion, unlike the legacy profile
/// tombstone operation.
pub struct ForgetMemoryTool {
    store: SharedMemoryStore,
}

impl ForgetMemoryTool {
    pub fn new(store: SharedMemoryStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ForgetMemoryTool {
    fn name(&self) -> &str {
        "forget_memory"
    }

    fn description(&self) -> &str {
        "Permanently remove Boris's durable memory only when the human explicitly asks to forget a fact/topic or everything. This deletes matching canonical memory and its search-index entries."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Exact fact or topic to permanently forget" },
                "all": { "type": "boolean", "description": "True only for an explicit request to forget all memory" }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Memory)
            .permissions(&[Permission::FsWrite])
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(&self, _ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let all = optional_bool(obj, "all").unwrap_or(false);
        let query = optional_string(obj, "query").unwrap_or_default();
        if !all && query.trim().is_empty() {
            return Err(ToolError::invalid_args("provide query or all=true"));
        }
        if all {
            self.store.forget_all().map_err(ToolError::failed)?;
            return Ok("Permanently erased all durable Boris memory.".into());
        }
        let deleted = self
            .store
            .forget_matching(&query)
            .map_err(ToolError::failed)?;
        Ok(format!(
            "Permanently forgot {deleted} matching memory record(s)."
        ))
    }
}

pub fn memory_tools(store: SharedMemoryStore) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(MemorySearchTool::new(store.clone())),
        Box::new(MemoryGetTool::new(store.clone())),
        Box::new(ForgetMemoryTool::new(store)),
    ]
}

fn format_kind(kind: crate::memory::MemoryKind) -> &'static str {
    match kind {
        crate::memory::MemoryKind::Semantic => "fact",
        crate::memory::MemoryKind::Episodic => "event",
        crate::memory::MemoryKind::Project => "project",
        crate::memory::MemoryKind::Procedural => "preference",
    }
}

fn format_scope(scope: crate::memory::MemoryScope) -> &'static str {
    match scope {
        crate::memory::MemoryScope::User => "user",
        crate::memory::MemoryScope::Session => "session",
        crate::memory::MemoryScope::Workspace => "workspace",
        crate::memory::MemoryScope::Agent => "agent",
    }
}
