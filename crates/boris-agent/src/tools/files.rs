//! Filesystem tools — tau-style `file_read` / `file_write` / `file_edit` + `list_dir`.
//!
//! Paths resolve under sandboxed roots (relative paths join the sandbox first).

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tools::fs_common::{read_roots, resolve_under_roots, write_roots};

const MAX_LIST: usize = 200;
const MAX_READ_LINES: usize = 2000;
const MAX_READ_BYTES: usize = 200 * 1024;
const MAX_WRITE_BYTES: usize = 512 * 1024;

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

// ── list_dir ─────────────────────────────────────────────────────────────────

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
        "List files and folders in a directory under allowed paths. \
         Defaults to the Boris sandbox. Returns name + type (dir/file)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory to list (default: sandbox root). Relative paths are under the sandbox."
                },
                "limit": {
                    "type": "number",
                    "description": "Max entries (default 80, max 200)"
                }
            },
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Read)
            .permissions(&[Permission::FsRead])
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = optional_string(obj, "path")
            .unwrap_or_else(|| self.roots.sandbox.to_string_lossy().into_owned());
        let path = resolve_under_roots(&raw, &self.roots.readers())?;
        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(80)
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

        let mut entries: Vec<(String, String)> = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ToolError::failed(format!("list_dir entry: {e}")))?
        {
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
            entries.push((name, kind.to_string()));
        }
        entries.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));

        let total = entries.len();
        let truncated = total > limit;
        let shown = &entries[..total.min(limit)];
        if shown.is_empty() {
            return Ok(format!("Empty directory: {}", path.display()));
        }
        let lines: Vec<String> = shown
            .iter()
            .map(|(n, k)| format!("{k}\t{n}"))
            .collect();
        let mut out = format!(
            "Listing {} ({} of {} entries):\n{}",
            path.display(),
            shown.len(),
            total,
            lines.join("\n")
        );
        if truncated {
            out.push_str("\n…[truncated]");
        }
        Ok(truncate_tool_result(out))
    }
}

// ── file_read ────────────────────────────────────────────────────────────────

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
        "file_read"
    }

    fn description(&self) -> &str {
        "Read a text file under allowed paths. Returns numbered lines (LINE\\tcontent). \
         Use offset/limit for large files. Relative paths resolve under the sandbox."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to read"
                },
                "offset": {
                    "type": "number",
                    "description": "1-based start line (default 1)"
                },
                "limit": {
                    "type": "number",
                    "description": "Max lines to return (default 200, max 2000)"
                }
            },
            "required": ["path"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Read)
            .permissions(&[Permission::FsRead])
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
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
            .unwrap_or(200)
            .clamp(1, MAX_READ_LINES);

        if !path.exists() {
            return Err(ToolError::failed(format!(
                "File not found: {}",
                path.display()
            )));
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::failed(format!("read {}: {e}", path.display())))?;
        if bytes.iter().take(512).any(|&b| b == 0) {
            return Err(ToolError::failed("File appears to be binary"));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| ToolError::failed("File appears to be binary (invalid UTF-8)"))?;

        if content.is_empty() {
            return Ok("(empty file)".into());
        }

        let all_lines: Vec<&str> = content.lines().collect();
        let total = all_lines.len();
        if offset > total {
            return Err(ToolError::failed(format!(
                "Offset {offset} exceeds file length ({total} lines)"
            )));
        }

        let start_idx = offset.saturating_sub(1);
        let end_idx = (start_idx + limit).min(total);
        let selected = &all_lines[start_idx..end_idx];

        let mut output = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = start_idx + i + 1;
            output.push_str(&format!("{line_num}\t{line}\n"));
        }

        if output.len() > MAX_READ_BYTES {
            output = output.chars().take(MAX_READ_BYTES).collect();
            output.push_str("\n…[truncated by bytes]");
        }

        if end_idx < total {
            output.push_str(&format!(
                "\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                start_idx + 1,
                end_idx,
                total,
                end_idx + 1
            ));
        }

        Ok(truncate_tool_result(output))
    }
}

// ── file_write ───────────────────────────────────────────────────────────────

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
        "file_write"
    }

    fn description(&self) -> &str {
        "Write content to a file under the write sandbox (default ~/.boris/sandbox), \
         creating parent directories as needed. Overwrites by default. Requires confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute or relative path to write"
                },
                "content": {
                    "type": "string",
                    "description": "Full file content to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::Write)
            .permissions(&[Permission::FsWrite])
            .confirm(true)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let content = require_string(obj, "content")?;

        if content.len() > MAX_WRITE_BYTES {
            return Err(ToolError::invalid_args(format!(
                "content exceeds {MAX_WRITE_BYTES} bytes"
            )));
        }

        let path = resolve_under_roots(&raw, &self.roots.writers())?;
        let created = !path.exists();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::failed(format!("create parent dirs: {e}")))?;
        }
        tokio::fs::write(&path, content.as_bytes())
            .await
            .map_err(|e| ToolError::failed(format!("write {}: {e}", path.display())))?;

        let action = if created { "Created" } else { "Wrote" };
        Ok(truncate_tool_result(format!(
            "{action} {} bytes to {}",
            content.len(),
            path.display()
        )))
    }
}

// ── file_edit ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct EditFileTool {
    roots: FsRoots,
}

impl EditFileTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "file_edit"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file (old_string → new_string). \
         old_string must match exactly once (including whitespace). Requires confirmation."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find (must appear exactly once)"
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text (may be empty to delete)"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .kind(ToolKind::Write)
            .permissions(&[Permission::FsWrite])
            .confirm(true)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let old_string = require_string(obj, "old_string")?;
        let new_string = require_string(obj, "new_string")?;

        if old_string.is_empty() {
            return Err(ToolError::invalid_args("old_string must not be empty"));
        }
        if old_string == new_string {
            return Err(ToolError::invalid_args(
                "old_string and new_string are identical",
            ));
        }

        let path = resolve_under_roots(&raw, &self.roots.writers())?;
        if !path.exists() {
            return Err(ToolError::failed(format!(
                "File not found: {}",
                path.display()
            )));
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::failed(format!("read {}: {e}", path.display())))?;

        let count = content.matches(&old_string).count();
        let (matched, strategy) = if count == 1 {
            (old_string.clone(), "exact")
        } else if count == 0 {
            // Fuzzy: trim trailing whitespace per line
            match fuzzy_unique(&content, &old_string) {
                Some((m, s)) => (m, s),
                None => {
                    let preview: String = content.lines().take(12).collect::<Vec<_>>().join("\n");
                    return Err(ToolError::failed(format!(
                        "old_string not found in {}.\n\nFile preview:\n{preview}",
                        path.display()
                    )));
                }
            }
        } else {
            return Err(ToolError::failed(format!(
                "Found {count} occurrences of old_string; must be exactly 1. Make old_string more specific."
            )));
        };

        let old_bytes = content.len();
        let new_content = content.replacen(&matched, &new_string, 1);
        let new_bytes = new_content.len();
        tokio::fs::write(&path, new_content.as_bytes())
            .await
            .map_err(|e| ToolError::failed(format!("write {}: {e}", path.display())))?;

        Ok(truncate_tool_result(format!(
            "Replaced 1 occurrence in {} (match={strategy}). {old_bytes} → {new_bytes} bytes",
            path.display()
        )))
    }
}

fn normalize_trim_end(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fuzzy_unique(content: &str, old_string: &str) -> Option<(String, &'static str)> {
    let norm_content = normalize_trim_end(content);
    let norm_old = normalize_trim_end(old_string);
    if norm_old.is_empty() {
        return None;
    }
    if norm_content.matches(&norm_old).count() != 1 {
        return None;
    }
    // Map back: find the norm match, then use original span via line alignment.
    // Simpler: if trim_end of whole content match works, replace using original lines block.
    // For reliability, search original with flexible line ends.
    let old_lines: Vec<&str> = old_string.lines().collect();
    if old_lines.is_empty() {
        return None;
    }
    let content_lines: Vec<&str> = content.lines().collect();
    let mut found: Option<usize> = None;
    for i in 0..=content_lines.len().saturating_sub(old_lines.len()) {
        let mut ok = true;
        for (j, ol) in old_lines.iter().enumerate() {
            if content_lines[i + j].trim_end() != ol.trim_end() {
                ok = false;
                break;
            }
        }
        if ok {
            if found.is_some() {
                return None; // not unique
            }
            found = Some(i);
        }
    }
    let start = found?;
    // Reconstruct original matched substring including newlines as in content.
    let mut matched = String::new();
    for (j, _) in old_lines.iter().enumerate() {
        if j > 0 {
            matched.push('\n');
        }
        matched.push_str(content_lines[start + j]);
    }
    // If original used trailing newline after block, leave as-is (replacen on exact lines).
    Some((matched, "trim_end"))
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
    async fn write_read_list_edit() {
        let (roots, dir) = temp_roots();
        let write = WriteFileTool::new(roots.clone());
        let read = ReadFileTool::new(roots.clone());
        let list = ListDirTool::new(roots.clone());
        let edit = EditFileTool::new(roots);

        write
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({
                "path": "hello.txt",
                "content": "hi boris\nline2\n"
            }))
            .await
            .unwrap();

        let body = read
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({ "path": "hello.txt" }))
            .await
            .unwrap();
        assert!(body.contains("hi boris"));
        assert!(body.contains("1\t") || body.contains("1\thi"));

        let listing = list.execute(&crate::tool_context::ToolCallContext::new("t"), json!({})).await.unwrap();
        assert!(listing.contains("hello.txt"));

        edit.execute(&crate::tool_context::ToolCallContext::new("t"), json!({
            "path": "hello.txt",
            "old_string": "hi boris",
            "new_string": "hello world"
        }))
        .await
        .unwrap();

        let body2 = read
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({ "path": "hello.txt" }))
            .await
            .unwrap();
        assert!(body2.contains("hello world"));
        assert!(!body2.contains("hi boris"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn outside_root_denied() {
        let (roots, dir) = temp_roots();
        let read = ReadFileTool::new(roots);
        let err = read
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({ "path": "C:\\Windows\\System32\\drivers\\etc\\hosts" }))
            .await
            .unwrap_err();
        assert!(err.message.contains("outside") || err.message.contains("path"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
