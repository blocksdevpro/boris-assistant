//! Lightweight task list stored under the sandbox.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, truncate_tool_result, Permission, Tool, ToolError, ToolMeta,
    ToolRisk,
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

fn default_todos_path(sandbox: &std::path::Path) -> PathBuf {
    sandbox.join("todos.json")
}

async fn load_todos(path: &std::path::Path) -> Result<Vec<TodoItem>, ToolError> {
    match tokio::fs::read_to_string(path).await {
        Ok(s) if s.trim().is_empty() => Ok(vec![]),
        Ok(s) => serde_json::from_str(&s)
            .map_err(|e| ToolError::failed(format!("parse todos: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(ToolError::failed(format!("read todos: {e}"))),
    }
}

async fn save_todos(path: &std::path::Path, items: &[TodoItem]) -> Result<(), ToolError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ToolError::failed(format!("create todos dir: {e}")))?;
    }
    let s = serde_json::to_string_pretty(items)
        .map_err(|e| ToolError::failed(format!("serialize todos: {e}")))?;
    tokio::fs::write(path, s)
        .await
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
    pub fn new(sandbox_root: impl Into<PathBuf>) -> Self {
        Self {
            path: default_todos_path(&sandbox_root.into()),
        }
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
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsRead])
    }

    async fn execute(&self, _args: Value) -> Result<String, ToolError> {
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
    pub fn new(sandbox_root: impl Into<PathBuf>) -> Self {
        Self {
            path: default_todos_path(&sandbox_root.into()),
        }
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
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsWrite])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
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
            .execute(json!({
                "items": [
                    {"id": "1", "content": "ship tools", "status": "pending"},
                    {"id": "2", "content": "test", "status": "done"}
                ]
            }))
            .await
            .unwrap();
        assert!(out.contains("ship tools"));

        let listed = read.execute(json!({})).await.unwrap();
        assert!(listed.contains("[ ] 1"));
        assert!(listed.contains("[x] 2"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
