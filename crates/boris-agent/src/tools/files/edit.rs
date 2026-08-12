//! `file_edit` — exact (or trim-end fuzzy) single-occurrence string replace.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

use super::FsRoots;

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// Successful in-memory edit result (not yet written to disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppliedEdit {
    pub new_content: String,
    /// Match strategy label (`exact` or `trim_end`).
    pub strategy: &'static str,
    pub old_bytes: usize,
    pub new_bytes: usize,
}

/// Failure reasons for applying an edit (mapped to [`ToolError`] by the tool).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditError {
    EmptyOldString,
    IdenticalStrings,
    NotFound { preview: String },
    Ambiguous { count: usize },
}

impl EditError {
    pub fn into_tool_error(self, path_display: &str) -> ToolError {
        match self {
            EditError::EmptyOldString => ToolError::invalid_args("old_string must not be empty"),
            EditError::IdenticalStrings => {
                ToolError::invalid_args("old_string and new_string are identical")
            }
            EditError::NotFound { preview } => ToolError::failed(format!(
                "old_string not found in {path_display}.\n\nFile preview:\n{preview}"
            )),
            EditError::Ambiguous { count } => ToolError::failed(format!(
                "Found {count} occurrences of old_string; must be exactly 1. \
                 Make old_string more specific."
            )),
        }
    }
}

/// Normalize by trimming trailing whitespace on each line (preserves line breaks).
pub(crate) fn normalize_trim_end(text: &str) -> String {
    text.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Fuzzy unique match: find a unique multi-line span where each line matches
/// after trailing-whitespace trim. Returns the **original** matched substring
/// (so `replacen` can apply against the real file content) and strategy label.
pub(crate) fn fuzzy_unique(content: &str, old_string: &str) -> Option<(String, &'static str)> {
    let norm_content = normalize_trim_end(content);
    let norm_old = normalize_trim_end(old_string);
    if norm_old.is_empty() {
        return None;
    }
    if norm_content.matches(&norm_old).count() != 1 {
        return None;
    }

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

    let mut matched = String::new();
    for (j, _) in old_lines.iter().enumerate() {
        if j > 0 {
            matched.push('\n');
        }
        matched.push_str(content_lines[start + j]);
    }
    Some((matched, "trim_end"))
}

/// First ~`n` lines of content for error previews.
pub(crate) fn content_preview(content: &str, max_lines: usize) -> String {
    content
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply a single unique replacement in memory.
///
/// Strategy:
/// 1. Exact match of `old_string` once → replace.
/// 2. Zero exact matches → try trim-end fuzzy unique match.
/// 3. Multiple exact matches → ambiguous error.
pub(crate) fn apply_edit(
    content: &str,
    old_string: &str,
    new_string: &str,
) -> Result<AppliedEdit, EditError> {
    if old_string.is_empty() {
        return Err(EditError::EmptyOldString);
    }
    if old_string == new_string {
        return Err(EditError::IdenticalStrings);
    }

    let count = content.matches(old_string).count();
    let (matched, strategy) = if count == 1 {
        (old_string.to_string(), "exact")
    } else if count == 0 {
        match fuzzy_unique(content, old_string) {
            Some((m, s)) => (m, s),
            None => {
                return Err(EditError::NotFound {
                    preview: content_preview(content, 12),
                });
            }
        }
    } else {
        return Err(EditError::Ambiguous { count });
    };

    let old_bytes = content.len();
    let new_content = content.replacen(&matched, new_string, 1);
    let new_bytes = new_content.len();
    Ok(AppliedEdit {
        new_content,
        strategy,
        old_bytes,
        new_bytes,
    })
}

/// Success observation after a disk write.
pub(crate) fn format_edit_result(
    path_display: &str,
    strategy: &str,
    old_bytes: usize,
    new_bytes: usize,
) -> String {
    format!(
        "Replaced 1 occurrence in {path_display} (match={strategy}). \
         {old_bytes} → {new_bytes} bytes"
    )
}

// ── Tool ─────────────────────────────────────────────────────────────────────

/// Edit a file by replacing an exact (or trim-end fuzzy) unique string.
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
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let raw = require_string(obj, "path")?;
        let old_string = require_string(obj, "old_string")?;
        let new_string = require_string(obj, "new_string")?;

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

        let applied = apply_edit(&content, &old_string, &new_string)
            .map_err(|e| e.into_tool_error(&path.display().to_string()))?;

        tokio::fs::write(&path, applied.new_content.as_bytes())
            .await
            .map_err(|e| ToolError::failed(format!("write {}: {e}", path.display())))?;

        Ok(truncate_tool_result(format_edit_result(
            &path.display().to_string(),
            applied.strategy,
            applied.old_bytes,
            applied.new_bytes,
        )))
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_edit_exact_once() {
        let r = apply_edit("hello world\n", "world", "boris").unwrap();
        assert_eq!(r.new_content, "hello boris\n");
        assert_eq!(r.strategy, "exact");
        assert_eq!(r.old_bytes, "hello world\n".len());
        assert_eq!(r.new_bytes, "hello boris\n".len());
    }

    #[test]
    fn apply_edit_rejects_empty_old() {
        assert_eq!(
            apply_edit("abc", "", "x").unwrap_err(),
            EditError::EmptyOldString
        );
    }

    #[test]
    fn apply_edit_rejects_identical() {
        assert_eq!(
            apply_edit("abc", "ab", "ab").unwrap_err(),
            EditError::IdenticalStrings
        );
    }

    #[test]
    fn apply_edit_not_found() {
        match apply_edit("line1\nline2\n", "missing", "x") {
            Err(EditError::NotFound { preview }) => {
                assert!(preview.contains("line1"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn apply_edit_ambiguous() {
        match apply_edit("aa aa aa", "aa", "b") {
            Err(EditError::Ambiguous { count }) => assert_eq!(count, 3),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn apply_edit_delete_with_empty_new() {
        let r = apply_edit("keep REMOVE me", "REMOVE ", "").unwrap();
        assert_eq!(r.new_content, "keep me");
        assert_eq!(r.strategy, "exact");
    }

    #[test]
    fn fuzzy_unique_trim_end_match() {
        // File lines have trailing spaces; old_string does not.
        let content = "fn main() {  \n    println!(\"hi\");\n}\n";
        let old = "fn main() {\n    println!(\"hi\");\n}";
        let (matched, strategy) = fuzzy_unique(content, old).unwrap();
        assert_eq!(strategy, "trim_end");
        assert!(matched.contains("fn main() {"));

        let r = apply_edit(content, old, "fn main() {\n    println!(\"bye\");\n}").unwrap();
        assert_eq!(r.strategy, "trim_end");
        assert!(r.new_content.contains("bye"));
        assert!(!r.new_content.contains("hi"));
    }

    #[test]
    fn fuzzy_unique_rejects_ambiguous() {
        let content = "foo  \nbar\nfoo  \nbar\n";
        let old = "foo\nbar";
        assert!(fuzzy_unique(content, old).is_none());
    }

    #[test]
    fn normalize_trim_end_strips_trailing_ws() {
        assert_eq!(normalize_trim_end("a  \nb\t\n"), "a\nb");
    }

    #[test]
    fn content_preview_limits_lines() {
        let p = content_preview("1\n2\n3\n4\n", 2);
        assert_eq!(p, "1\n2");
    }

    #[test]
    fn format_edit_result_shape() {
        let s = format_edit_result("/s/a.txt", "exact", 10, 12);
        assert!(s.contains("Replaced 1 occurrence"));
        assert!(s.contains("match=exact"));
        assert!(s.contains("10 → 12 bytes"));
    }

    #[test]
    fn edit_error_messages() {
        let e = EditError::EmptyOldString.into_tool_error("/x");
        assert!(e.message.contains("empty"));
        let e = EditError::Ambiguous { count: 4 }.into_tool_error("/x");
        assert!(e.message.contains('4'));
        let e = EditError::NotFound {
            preview: "prev".into(),
        }
        .into_tool_error("/path/f.txt");
        assert!(e.message.contains("/path/f.txt"));
        assert!(e.message.contains("prev"));
    }

    #[tokio::test]
    async fn edit_file_tool_roundtrip() {
        let (roots, dir) = crate::tools::files::test_util::temp_roots();
        std::fs::write(dir.join("hello.txt"), "hi boris\nline2\n").unwrap();

        let edit = EditFileTool::new(roots);
        let msg = edit
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({
                    "path": "hello.txt",
                    "old_string": "hi boris",
                    "new_string": "hello world"
                }),
            )
            .await
            .unwrap();
        assert!(msg.contains("Replaced 1 occurrence"));
        assert!(msg.contains("exact"));

        let body = std::fs::read_to_string(dir.join("hello.txt")).unwrap();
        assert!(body.contains("hello world"));
        assert!(!body.contains("hi boris"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
