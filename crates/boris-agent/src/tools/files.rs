//! Sandboxed list / read / write file tools.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};
use crate::tools::fs_common::{resolve_under_roots, read_roots, write_roots};

const MAX_LIST: usize = 100;
const MAX_READ_LINES: usize = 200;
const MAX_READ_BYTES: usize = 16 * 1024;
const MAX_WRITE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct FsRoots {
    pub sandbox: PathBuf,
    pub data: Vec<PathBuf>,
    pub allow_read: Vec<PathBuf>,
    pub allow_write: Vec<PathBuf>,
}

impl FsRoots {
    pub fn readers(&self) -> Vec<PathBuf> {
        read_roots(
            &self.sandbox,
            &self.data,
            &self.allow_read,
            &self.allow_write,
        )
    }

    pub fn writers(&self) -> Vec<PathBuf> {
        write_roots(&self.sandbox, &self.data, &self.allow_write)
    }
}

/// List directory entries under allowed roots.
#[derive(Debug, Clone)]
pub struct ListDirTool {
    roots: FsRoots,
}

impl ListDirTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List files and folders in a directory under allowed paths. Defaults to the Boris sandbox if path is omitted."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: sandbox root)"
                },
                "limit": {
                    "type": "number",
                    "description": "Max entries (default 50, max 100)"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsRead])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = optional_string(obj, "path").unwrap_or_else(|| {
            self.roots.sandbox.to_string_lossy().into_owned()
        });
        let path = resolve_under_roots(&raw, &self.roots.readers())?;
        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(50)
            .clamp(1, MAX_LIST);

        let meta = tokio::fs::metadata(&path)
            .await
            .map_err(|e| ToolError::failed(format!("stat {}: {e}", path.display())))?;
        if !meta.is_dir() {
            return Err(ToolError::failed(format!(
                "not a directory: {}",
                path.display()
            )));
        }

        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| ToolError::failed(format!("list_dir: {e}")))?;

        let mut lines = Vec::new();
        let mut count = 0usize;
        let mut truncated = false;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ToolError::failed(format!("list_dir entry: {e}")))?
        {
            if count >= limit {
                truncated = true;
                break;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let ft = entry
                .file_type()
                .await
                .map_err(|e| ToolError::failed(format!("file_type: {e}")))?;
            let kind = if ft.is_dir() {
                "dir"
            } else if ft.is_file() {
                "file"
            } else {
                "other"
            };
            lines.push(format!("{kind}\t{name}"));
            count += 1;
        }

        if lines.is_empty() {
            return Ok(format!("Empty directory: {}", path.display()));
        }
        let mut out = format!("Listing {} ({} entries):\n{}", path.display(), count, lines.join("\n"));
        if truncated {
            out.push_str("\n…[truncated]");
        }
        Ok(truncate_tool_result(out))
    }
}

/// Read a text file under allowed roots.
#[derive(Debug, Clone)]
pub struct ReadFileTool {
    roots: FsRoots,
}

impl ReadFileTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a text file under allowed paths. Optional line offset and limit for large files."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "offset": {
                    "type": "number",
                    "description": "1-based start line (default 1)"
                },
                "limit": {
                    "type": "number",
                    "description": "Max lines to return (default 100, max 200)"
                }
            },
            "required": ["path"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsRead])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let path = resolve_under_roots(&raw, &self.roots.readers())?;
        let offset = obj
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(1)
            .max(1);
        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(100)
            .clamp(1, MAX_READ_LINES);

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::failed(format!("read {}: {e}", path.display())))?;
        if bytes.iter().take(512).any(|&b| b == 0) {
            return Err(ToolError::failed("binary file (refusing to read)"));
        }
        let text = String::from_utf8_lossy(&bytes);
        let all_lines: Vec<&str> = text.lines().collect();
        let start = offset.saturating_sub(1).min(all_lines.len());
        let end = (start + limit).min(all_lines.len());
        let slice = &all_lines[start..end];
        let mut body = slice.join("\n");
        if body.len() > MAX_READ_BYTES {
            body = body.chars().take(MAX_READ_BYTES).collect();
            body.push_str("\n…[truncated by bytes]");
        }
        let header = format!(
            "File {} lines {}-{} of {}:\n",
            path.display(),
            start + 1,
            end,
            all_lines.len()
        );
        Ok(truncate_tool_result(format!("{header}{body}")))
    }
}

/// Write a text file under write roots (sandbox by default). Requires confirmation.
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
        "write_file"
    }

    fn description(&self) -> &str {
        "Write a text file under the write sandbox (default ~/.boris/sandbox). Requires user confirmation. Set overwrite true to replace an existing file."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" },
                "overwrite": {
                    "type": "boolean",
                    "description": "If false (default), fail when the file exists"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::FsWrite])
            .confirm(true)
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let content = require_string(obj, "content")?;
        let overwrite = obj
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError::invalid_args(format!(
                "content exceeds {MAX_WRITE_BYTES} bytes"
            )));
        }

        let path = resolve_under_roots(&raw, &self.roots.writers())?;
        if path.exists() && !overwrite {
            return Err(ToolError::failed(format!(
                "file exists (set overwrite=true): {}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::failed(format!("create parent: {e}")))?;
        }
        tokio::fs::write(&path, content.as_bytes())
            .await
            .map_err(|e| ToolError::failed(format!("write {}: {e}", path.display())))?;

        Ok(truncate_tool_result(format!(
            "Wrote {} bytes to {}",
            content.len(),
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_roots() -> (FsRoots, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "boris-fs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp sandbox");
        (
            FsRoots {
                sandbox: dir.clone(),
                data: vec![],
                allow_read: vec![],
                allow_write: vec![],
            },
            dir,
        )
    }

    #[tokio::test]
    async fn write_read_list() {
        let (roots, dir) = temp_roots();
        let write = WriteFileTool::new(roots.clone());
        let read = ReadFileTool::new(roots.clone());
        let list = ListDirTool::new(roots);

        write
            .execute(json!({
                "path": "hello.txt",
                "content": "hi boris\nline2",
                "overwrite": true
            }))
            .await
            .unwrap();

        let body = read
            .execute(json!({ "path": "hello.txt" }))
            .await
            .unwrap();
        assert!(body.contains("hi boris"));

        let listing = list.execute(json!({})).await.unwrap();
        assert!(listing.contains("hello.txt"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn outside_root_denied() {
        let (roots, dir) = temp_roots();
        let read = ReadFileTool::new(roots);
        let err = read
            .execute(json!({ "path": "C:\\Windows\\System32\\drivers\\etc\\hosts" }))
            .await
            .unwrap_err();
        assert!(err.message.contains("outside") || err.message.contains("path"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
