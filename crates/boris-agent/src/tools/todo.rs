//! Lightweight task list stored at an explicit todos file path (session file).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

/// Compat: todos file under a sandbox root (`sandbox/todos.json`).
fn default_todos_path(sandbox: &Path) -> PathBuf {
    sandbox.join("todos.json")
}

/// Atomic write: sibling `*.json.tmp` then rename (mirrors session/store/atomic).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_e) => {
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            Ok(())
        }
    }
}

async fn load_todos(path: &Path) -> Result<Vec<TodoItem>, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(s) if s.trim().is_empty() => Ok(vec![]),
        Ok(s) => {
            serde_json::from_str(&s).map_err(|e| ToolError::failed(format!("parse todos: {e}")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(ToolError::failed(format!("read todos: {e}"))),
    }
}

async fn save_todos(path: &Path, items: &[TodoItem]) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::failed(format!("create todos dir: {e}")))?;
    }
    let s = serde_json::to_string_pretty(items)
        .map_err(|e| ToolError::failed(format!("serialize todos: {e}")))?;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_atomic(&path, s.as_bytes()))
        .await
        .map_err(|e| ToolError::failed(format!("write todos join: {e}")))?
        .map_err(|e| ToolError::failed(format!("write todos: {e}")))
}

fn format_todos(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No todos.".into();
    }
    items
        .iter()
        .map(|t| {
            let mark = match t.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::Done => "[x]",
            };
            format!("{mark} {} — {}", t.id, t.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Read the current todo list.
#[derive(Debug, Clone)]
pub struct TodoReadTool {
    path: PathBuf,
}

impl TodoReadTool {
    /// Exact todos file path (session-bound or sandbox file).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Compat: `sandbox_root/todos.json`.
    pub fn new(sandbox_root: impl Into<PathBuf>) -> Self {
        Self::with_path(default_todos_path(&sandbox_root.into()))
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todo_read"
    }

    fn description(&self) -> &str {
        "Read the current multi-step todo list for this session/work."
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
            .kind(ToolKind::Plan)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        let items = load_todos(&self.path).await?;
        Ok(truncate_tool_result(format_todos(&items)))
    }
}

/// Replace or update the todo list (full list rewrite is fine for voice).
#[derive(Debug, Clone)]
pub struct TodoWriteTool {
    path: PathBuf,
}

impl TodoWriteTool {
    /// Exact todos file path (session-bound or sandbox file).
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Compat: `sandbox_root/todos.json`.
    pub fn new(sandbox_root: impl Into<PathBuf>) -> Self {
        Self::with_path(default_todos_path(&sandbox_root.into()))
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "Update the multi-step todo list. Pass items as a JSON array of {id, content, status} where status is pending or done. Replaces the whole list."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "Full todo list to store",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "content": { "type": "string" },
                            "status": { "type": "string", "description": "pending | done" }
                        },
                        "required": ["id", "content", "status"]
                    }
                },
                "merge_json": {
                    "type": "string",
                    "description": "Optional: raw JSON array string of items (alternative to items)"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Plan)
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
        let items = if let Some(arr) = obj.get("items").and_then(|v| v.as_array()) {
            parse_items_value(arr)?
        } else if let Some(raw) = optional_string(obj, "merge_json") {
            let v: Value = serde_json::from_str(&raw)
                .map_err(|e| ToolError::invalid_args(format!("merge_json parse: {e}")))?;
            let arr = v
                .as_array()
                .ok_or_else(|| ToolError::invalid_args("merge_json must be a JSON array"))?;
            parse_items_value(arr)?
        } else {
            return Err(ToolError::invalid_args(
                "provide items array or merge_json string",
            ));
        };

        if items.len() > 50 {
            return Err(ToolError::invalid_args("max 50 todos"));
        }
        for it in &items {
            if it.content.trim().is_empty() {
                return Err(ToolError::invalid_args("todo content empty"));
            }
        }

        save_todos(&self.path, &items).await?;
        Ok(truncate_tool_result(format!(
            "Saved {} todos.\n{}",
            items.len(),
            format_todos(&items)
        )))
    }
}

fn parse_items_value(arr: &[Value]) -> Result<Vec<TodoItem>, ToolError> {
    let mut out = Vec::with_capacity(arr.len());
    for (i, v) in arr.iter().enumerate() {
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or(&format!("t{i}"))
            .to_string();
        let content = v
            .get("content")
            .and_then(|x| x.as_str())
            .ok_or_else(|| ToolError::invalid_args(format!("items[{i}].content required")))?
            .to_string();
        let status_s = v
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("pending")
            .to_ascii_lowercase();
        let status = match status_s.as_str() {
            "done" | "completed" | "complete" => TodoStatus::Done,
            _ => TodoStatus::Pending,
        };
        out.push(TodoItem {
            id,
            content,
            status,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn todo_roundtrip() {
        let dir = std::env::temp_dir().join(format!("boris-todo-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let write = TodoWriteTool::new(&dir);
        let read = TodoReadTool::new(&dir);

        let out = write
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({
                    "items": [
                        {"id": "1", "content": "ship tools", "status": "pending"},
                        {"id": "2", "content": "test", "status": "done"}
                    ]
                }),
            )
            .await
            .unwrap();
        assert!(out.contains("ship tools"));

        let listed = read
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap();
        assert!(listed.contains("[ ] 1"));
        assert!(listed.contains("[x] 2"));

        // Atomic write should leave no temp sibling after success.
        assert!(!dir.join("todos.json.tmp").exists());
        assert!(dir.join("todos.json").exists());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn with_path_uses_exact_file() {
        let dir = std::env::temp_dir().join(format!("boris-todo-path-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("session-todos.json");
        let write = TodoWriteTool::with_path(&path);
        let read = TodoReadTool::with_path(&path);

        write
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({"items": [{"id": "a", "content": "exact path", "status": "pending"}]}),
            )
            .await
            .unwrap();

        assert!(path.exists());
        assert!(!dir.join("todos.json").exists());
        let listed = read
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap();
        assert!(listed.contains("exact path"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
