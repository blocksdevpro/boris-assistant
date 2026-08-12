//! `file_read` — read text files under allowed roots with numbered lines.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

use super::{FsRoots, DEFAULT_READ_LINES, MAX_READ_BYTES, MAX_READ_LINES};

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Parsed / clamped offset + limit for a `file_read` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadWindow {
    /// 1-based start line.
    pub offset: usize,
    /// Max lines to return.
    pub limit: usize,
}

/// Clamp model-supplied offset/limit into valid ranges.
pub(crate) fn parse_read_window(offset: Option<u64>, limit: Option<u64>) -> ReadWindow {
    ReadWindow {
        offset: offset.map(|n| n as usize).unwrap_or(1).max(1),
        limit: limit
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_READ_LINES)
            .clamp(1, MAX_READ_LINES),
    }
}

/// Heuristic: treat as binary if any NUL byte appears in the first 512 bytes.
pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(512).any(|&b| b == 0)
}

/// Result of slicing a file into a line window for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineSlice<'a> {
    /// Selected lines (references into the full line list).
    pub lines: &'a [&'a str],
    /// 1-based first line number included.
    pub start_line: usize,
    /// Exclusive end index into the full line list.
    pub end_idx: usize,
    /// Total line count in the file.
    pub total: usize,
}

/// Select a 1-based `[offset, offset+limit)` line window from full content lines.
///
/// Returns an error message when `offset` is past the end of the file.
pub(crate) fn select_line_window<'a>(
    all_lines: &'a [&'a str],
    window: ReadWindow,
) -> Result<LineSlice<'a>, String> {
    let total = all_lines.len();
    if window.offset > total {
        return Err(format!(
            "Offset {} exceeds file length ({total} lines)",
            window.offset
        ));
    }
    let start_idx = window.offset.saturating_sub(1);
    let end_idx = (start_idx + window.limit).min(total);
    Ok(LineSlice {
        lines: &all_lines[start_idx..end_idx],
        start_line: start_idx + 1,
        end_idx,
        total,
    })
}

/// Format lines as `LINE\tcontent\n` (1-based numbering starting at `start_line`).
pub(crate) fn format_numbered_lines(lines: &[&str], start_line: usize) -> String {
    let mut output = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line_num = start_line + i;
        output.push_str(&format!("{line_num}\t{line}\n"));
    }
    output
}

/// Soft-truncate by char count (stable UTF-8; mirrors prior MAX_READ_BYTES char take).
pub(crate) fn truncate_by_read_budget(mut output: String, max_chars: usize) -> String {
    if output.chars().count() > max_chars {
        output = output.chars().take(max_chars).collect();
        output.push_str("\n…[truncated by bytes]");
    }
    output
}

/// Append a "showing lines X–Y of Z" footer when more content remains.
pub(crate) fn continuation_footer(
    start_line: usize,
    end_idx: usize,
    total: usize,
) -> Option<String> {
    if end_idx < total {
        Some(format!(
            "\n[Showing lines {start_line}-{end_idx} of {total}. Use offset={} to continue.]",
            end_idx + 1
        ))
    } else {
        None
    }
}

/// Build the full `file_read` observation from file text + a window.
///
/// Empty files return a fixed sentinel. Does **not** apply the global tool-result
/// character cap (`truncate_tool_result`); the caller should.
pub(crate) fn build_read_output(content: &str, window: ReadWindow) -> Result<String, String> {
    if content.is_empty() {
        return Ok("(empty file)".into());
    }

    let all_lines: Vec<&str> = content.lines().collect();
    let slice = select_line_window(&all_lines, window)?;
    let mut output = format_numbered_lines(slice.lines, slice.start_line);
    output = truncate_by_read_budget(output, MAX_READ_BYTES);
    if let Some(footer) = continuation_footer(slice.start_line, slice.end_idx, slice.total) {
        output.push_str(&footer);
    }
    Ok(output)
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// Read a text file under sandboxed / allowlisted roots.
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
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let path = resolve_under_roots(&raw, &self.roots.readers())?;
        let window = parse_read_window(
            obj.get("offset").and_then(|v| v.as_u64()),
            obj.get("limit").and_then(|v| v.as_u64()),
        );

        if !path.exists() {
            return Err(ToolError::failed(format!(
                "File not found: {}",
                path.display()
            )));
        }

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| ToolError::failed(format!("read {}: {e}", path.display())))?;
        if looks_binary(&bytes) {
            return Err(ToolError::failed("File appears to be binary"));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| ToolError::failed("File appears to be binary (invalid UTF-8)"))?;

        let output = build_read_output(&content, window).map_err(ToolError::failed)?;
        Ok(truncate_tool_result(output))
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_read_window_defaults_and_bounds() {
        let w = parse_read_window(None, None);
        assert_eq!(w.offset, 1);
        assert_eq!(w.limit, DEFAULT_READ_LINES);

        assert_eq!(parse_read_window(Some(0), Some(0)).offset, 1);
        assert_eq!(parse_read_window(Some(0), Some(0)).limit, 1);
        assert_eq!(
            parse_read_window(Some(10), Some(99999)).limit,
            MAX_READ_LINES
        );
    }

    #[test]
    fn looks_binary_detects_nul() {
        assert!(!looks_binary(b"hello world"));
        assert!(looks_binary(b"abc\0def"));
        assert!(!looks_binary(&[]));
    }

    #[test]
    fn select_line_window_basic() {
        let lines = ["a", "b", "c", "d"];
        let refs: Vec<&str> = lines.to_vec();
        let slice = select_line_window(
            &refs,
            ReadWindow {
                offset: 2,
                limit: 2,
            },
        )
        .unwrap();
        assert_eq!(slice.lines, &["b", "c"]);
        assert_eq!(slice.start_line, 2);
        assert_eq!(slice.end_idx, 3);
        assert_eq!(slice.total, 4);
    }

    #[test]
    fn select_line_window_offset_past_end() {
        let lines = ["only"];
        let refs: Vec<&str> = lines.to_vec();
        let err = select_line_window(
            &refs,
            ReadWindow {
                offset: 5,
                limit: 10,
            },
        )
        .unwrap_err();
        assert!(err.contains("exceeds"));
        assert!(err.contains("1 lines"));
    }

    #[test]
    fn select_line_window_clamps_to_end() {
        let lines = ["a", "b"];
        let refs: Vec<&str> = lines.to_vec();
        let slice = select_line_window(
            &refs,
            ReadWindow {
                offset: 1,
                limit: 100,
            },
        )
        .unwrap();
        assert_eq!(slice.lines.len(), 2);
        assert_eq!(slice.end_idx, 2);
    }

    #[test]
    fn format_numbered_lines_uses_tabs() {
        let out = format_numbered_lines(&["hello", "world"], 3);
        assert_eq!(out, "3\thello\n4\tworld\n");
    }

    #[test]
    fn truncate_by_read_budget_appends_marker() {
        let long: String = "x".repeat(50);
        let out = truncate_by_read_budget(long, 10);
        assert!(out.ends_with("\n…[truncated by bytes]"));
        assert!(out.starts_with("xxxxxxxxxx"));
    }

    #[test]
    fn truncate_by_read_budget_noop_when_small() {
        let s = "short".to_string();
        assert_eq!(truncate_by_read_budget(s.clone(), 100), s);
    }

    #[test]
    fn continuation_footer_present_when_more() {
        let f = continuation_footer(1, 2, 10).unwrap();
        assert!(f.contains("1-2 of 10"));
        assert!(f.contains("offset=3"));
    }

    #[test]
    fn continuation_footer_none_at_end() {
        assert!(continuation_footer(1, 5, 5).is_none());
    }

    #[test]
    fn build_read_output_empty() {
        assert_eq!(
            build_read_output(
                "",
                ReadWindow {
                    offset: 1,
                    limit: 10
                }
            )
            .unwrap(),
            "(empty file)"
        );
    }

    #[test]
    fn build_read_output_window_and_footer() {
        let content = "a\nb\nc\nd\ne\n";
        let out = build_read_output(
            content,
            ReadWindow {
                offset: 2,
                limit: 2,
            },
        )
        .unwrap();
        assert!(out.contains("2\tb"));
        assert!(out.contains("3\tc"));
        assert!(!out.contains("1\ta"));
        assert!(out.contains("offset=4"));
        assert!(out.contains("of 5"));
    }

    #[test]
    fn build_read_output_offset_edge() {
        let err = build_read_output(
            "only\n",
            ReadWindow {
                offset: 99,
                limit: 1,
            },
        )
        .unwrap_err();
        assert!(err.contains("exceeds"));
    }

    #[tokio::test]
    async fn read_write_roundtrip_and_outside_denied() {
        let (roots, dir) = crate::tools::files::test_util::temp_roots();
        std::fs::write(dir.join("hello.txt"), "hi boris\nline2\n").unwrap();

        let read = ReadFileTool::new(roots.clone());
        let body = read
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "path": "hello.txt" }),
            )
            .await
            .unwrap();
        assert!(body.contains("hi boris"));
        assert!(body.contains("1\t") || body.contains("1\thi"));

        let err = read
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "path": "C:\\Windows\\System32\\drivers\\etc\\hosts" }),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("outside") || err.message.contains("path"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
