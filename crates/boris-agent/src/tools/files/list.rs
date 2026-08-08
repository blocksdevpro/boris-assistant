//! `list_dir` — list files and folders under allowed roots.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

use super::{DEFAULT_LIST_LIMIT, FsRoots, MAX_LIST};

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Clamp a model-supplied list limit into the allowed range.
pub(crate) fn clamp_list_limit(raw: Option<u64>) -> usize {
    raw.map(|n| n as usize)
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST)
}

/// Classify a directory entry into a short label.
pub(crate) fn entry_kind(is_dir: bool, is_file: bool) -> &'static str {
    if is_dir {
        "dir"
    } else if is_file {
        "file"
    } else {
        "other"
    }
}

/// Sort entries by name (case-insensitive ASCII).
pub(crate) fn sort_entries(entries: &mut [(String, String)]) {
    entries.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
}

/// Format a directory listing for the model.
///
/// `entries` are `(name, kind)` pairs already sorted. Applies `limit` and optional
/// truncation marker.
pub(crate) fn format_listing(
    path_display: &str,
    entries: &[(String, String)],
    limit: usize,
) -> String {
    let total = entries.len();
    if total == 0 {
        return format!("Empty directory: {path_display}");
    }

    let shown_len = total.min(limit);
    let truncated = total > limit;
    let shown = &entries[..shown_len];

    let lines: Vec<String> = shown
        .iter()
        .map(|(name, kind)| format!("{kind}\t{name}"))
        .collect();

    let mut out = format!(
        "Listing {path_display} ({shown_len} of {total} entries):\n{}",
        lines.join("\n")
    );
    if truncated {
        out.push_str("\n…[truncated]");
    }
    out
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// List directory entries under sandboxed / allowlisted roots.
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
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = optional_string(obj, "path")
            .unwrap_or_else(|| self.roots.sandbox.to_string_lossy().into_owned());
        let path = resolve_under_roots(&raw, &self.roots.readers())?;
        let limit = clamp_list_limit(obj.get("limit").and_then(|v| v.as_u64()));

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
            .map_err(|e| ToolError::failed(format!("list_dir {}: {e}", path.display())))?;

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
            let kind = entry_kind(ft.is_dir(), ft.is_file()).to_string();
            entries.push((name, kind));
        }
        sort_entries(&mut entries);

        let out = format_listing(&path.display().to_string(), &entries, limit);
        Ok(truncate_tool_result(out))
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_list_limit_defaults_and_bounds() {
        assert_eq!(clamp_list_limit(None), DEFAULT_LIST_LIMIT);
        assert_eq!(clamp_list_limit(Some(0)), 1);
        assert_eq!(clamp_list_limit(Some(50)), 50);
        assert_eq!(clamp_list_limit(Some(9999)), MAX_LIST);
    }

    #[test]
    fn entry_kind_labels() {
        assert_eq!(entry_kind(true, false), "dir");
        assert_eq!(entry_kind(false, true), "file");
        assert_eq!(entry_kind(false, false), "other");
    }

    #[test]
    fn sort_entries_case_insensitive() {
        let mut entries = vec![
            ("Zebra".into(), "file".into()),
            ("apple".into(), "dir".into()),
            ("Banana".into(), "file".into()),
        ];
        sort_entries(&mut entries);
        assert_eq!(entries[0].0, "apple");
        assert_eq!(entries[1].0, "Banana");
        assert_eq!(entries[2].0, "Zebra");
    }

    #[test]
    fn format_listing_empty() {
        let out = format_listing("/tmp/x", &[], 80);
        assert!(out.contains("Empty directory"));
        assert!(out.contains("/tmp/x"));
    }

    #[test]
    fn format_listing_truncates() {
        let entries: Vec<(String, String)> = (0..5)
            .map(|i| (format!("f{i}"), "file".into()))
            .collect();
        let out = format_listing("/sandbox", &entries, 2);
        assert!(out.contains("2 of 5 entries"));
        assert!(out.contains("file\tf0"));
        assert!(out.contains("file\tf1"));
        assert!(!out.contains("f4"));
        assert!(out.contains("…[truncated]"));
    }

    #[test]
    fn format_listing_no_truncation_marker_when_within_limit() {
        let entries = vec![("a.txt".into(), "file".into())];
        let out = format_listing("/s", &entries, 10);
        assert!(out.contains("1 of 1 entries"));
        assert!(!out.contains("…[truncated]"));
    }

    #[tokio::test]
    async fn list_dir_smoke() {
        let (roots, dir) = crate::tools::files::test_util::temp_roots();
        std::fs::write(dir.join("hello.txt"), "x").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let list = ListDirTool::new(roots);
        let listing = list
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({}),
            )
            .await
            .unwrap();
        assert!(listing.contains("hello.txt"));
        assert!(listing.contains("sub"));
        assert!(listing.contains("file\t") || listing.contains("dir\t"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
