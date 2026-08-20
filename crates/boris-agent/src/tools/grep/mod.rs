//! Content search — ripgrep when it helps; regex-capable in-process fallback.
//!
//! # Tool
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `grep` | [`GrepTool`] | Regex/text search under allowed roots |
//!
//! Schema follows Grok Build / Claude Code: `-A`/`-B`/`-C`/`-i`, `type`,
//! `multiline`, `output_mode`, `head_limit`. Boris aliases (`ignore_case`,
//! `context`, `limit`) still work.
//!
//! Directory searches prefer `rg` (gitignore, speed). Single files and missing
//! `rg` use the `regex` crate — never a silent substring fallback for regex.

mod fallback;
mod format;
mod query;
mod rg;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    require_object, truncate_tool_result, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tools::files::FsRoots;
use crate::tools::fs_common::resolve_under_roots;

use fallback::rust_grep;
use query::GrepQuery;
use rg::run_rg;

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;
const MAX_CONTEXT: usize = 20;
const MAX_LINE_CHARS: usize = 500;

#[derive(Debug, Clone)]
pub struct GrepTool {
    roots: FsRoots,
}

impl GrepTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regular expressions (ripgrep).

- Full regex syntax, so escape literal special characters: `functionCall\\(`, or `interface\\{\\}` to find interface{} in Go.
- Pass pattern as a raw regex string — no surrounding quotes.
- Respects .gitignore unless you pass a broad glob like '--glob *'.
- Only filter by 'type' or 'glob' when you are sure of the file type; import paths may not match source file types (.js vs .ts).
- Output is ripgrep-style: ':' marks match lines, '-' marks context lines, grouped by file. Large results are capped and report \"at least\" counts.
- Use this instead of bash grep/rg/findstr. Batch several greps in one multi-tool message."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regular expression pattern to search for in file contents (rg --regexp)"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in (rg pattern -- PATH). Defaults to workspace path."
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern (rg --glob GLOB -- PATH) to filter files (e.g. \"*.js\", \"*.{ts,tsx}\")."
                },
                "-B": {
                    "type": "number",
                    "description": "Number of lines to show before each match (rg -B)."
                },
                "-A": {
                    "type": "number",
                    "description": "Number of lines to show after each match (rg -A)."
                },
                "-C": {
                    "type": "number",
                    "description": "Number of lines to show before and after each match (rg -C)."
                },
                "-i": {
                    "type": "boolean",
                    "description": "Case insensitive search (rg -i)."
                },
                "type": {
                    "type": "string",
                    "description": "File type to search (rg --type). Common types: js, py, rust, go, java, etc. More efficient than glob for standard file types."
                },
                "head_limit": {
                    "type": "number",
                    "description": "Limit output to first N lines/entries, equivalent to \"| head -N\". Defaults to 200 lines or 500 entries."
                },
                "multiline": {
                    "type": "boolean",
                    "description": "Enable multiline mode where . matches newlines and patterns can span lines (rg -U --multiline-dotall)."
                },
                "output_mode": {
                    "type": "string",
                    "description": "content (default, matching lines), files_with_matches (paths only), or count (per-file counts)."
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Alias of -i."
                },
                "context": {
                    "type": "number",
                    "description": "Alias of -C."
                },
                "limit": {
                    "type": "number",
                    "description": "Alias of head_limit."
                }
            },
            "required": ["pattern"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Search)
            .permissions(&[Permission::FsRead])
            .timeout(Duration::from_secs(30))
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let query = GrepQuery::parse(obj)?;
        let raw_path = query
            .path
            .clone()
            .unwrap_or_else(|| self.roots.sandbox.to_string_lossy().into_owned());
        let search_path = resolve_under_roots(&raw_path, &self.roots.readers())?;
        if !search_path.exists() {
            return Ok(format!(
                "Path not found: {}. Check the path, or glob / list_dir from a parent directory first.",
                search_path.display()
            ));
        }

        let display = search_path.display().to_string();
        let glob = query.glob.clone();
        let pattern = query.pattern.clone();

        if should_spawn_rg(&search_path) {
            match run_rg(&query, &search_path).await {
                Ok(hits) => {
                    return Ok(truncate_tool_result(hits.render(
                        &pattern,
                        &display,
                        glob.as_deref(),
                    )));
                }
                Err(e) if e.kind() == crate::tool::ToolErrorKind::InvalidArgs => {
                    return Err(e);
                }
                Err(e) => {
                    tracing::debug!(error = %e, "rg failed; using Rust fallback");
                    // Invalid regex from rg should not silently substring-match.
                    if e.message.to_ascii_lowercase().contains("regex")
                        || e.message.contains("regex parse")
                    {
                        return Err(e);
                    }
                }
            }
        }

        let query_owned = query;
        let path_owned = search_path.clone();
        let hits = tokio::task::spawn_blocking(move || rust_grep(&query_owned, &path_owned))
            .await
            .map_err(|e| ToolError::failed(format!("grep task: {e}")))??;

        Ok(truncate_tool_result(hits.render(
            &pattern,
            &display,
            glob.as_deref(),
        )))
    }
}

/// Whether this call should pay for an `rg` process.
///
/// Single-file searches stay in-process (spawn is the expensive part on Windows).
/// Directory trees use `rg` whenever it is installed — literals included — so
/// gitignore and large walks stay fast.
pub(super) fn should_spawn_rg(search_path: &std::path::Path) -> bool {
    !search_path.is_file() && rg::rg_available()
}

/// True when `pattern` looks like a regex, not a plain keyword / `foo.rs` path.
///
/// A lone `.` (file extension) is *not* treated as regex — `file.rs` is a
/// literal. `.` only counts when it starts a quantifier (`.*`, `.+`, `.?`, `.{`).
pub(super) fn looks_like_regex(pattern: &str) -> bool {
    let b = pattern.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'*' | b'+' | b'?' | b'[' | b']' | b'(' | b')' | b'{' | b'}' | b'|' | b'^' | b'$'
            | b'\\' => return true,
            b'.' => {
                if matches!(b.get(i + 1), Some(b'*' | b'+' | b'?' | b'{')) {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::files::FsRoots;
    use serde_json::json;

    fn roots_at(dir: std::path::PathBuf) -> FsRoots {
        FsRoots {
            sandbox: dir,
            data: vec![],
            allow_read: vec![],
            allow_write: vec![],
        }
    }

    #[tokio::test]
    async fn rust_fallback_finds_line() {
        let dir = std::env::temp_dir().join(format!("boris-grep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\nfindme please\nbye\n").unwrap();
        let tool = GrepTool::new(roots_at(dir.clone()));
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": "findme", "path": dir.to_string_lossy() }),
            )
            .await
            .unwrap();
        assert!(out.contains("findme"), "got: {out}");
        assert!(out.contains("<workspace_result"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn regex_fallback_matches_fn_defs() {
        let dir = std::env::temp_dir().join(format!("boris-grep-re-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lib.rs"),
            "fn main() {}\nconst X = 1;\nfn helper() {}\n",
        )
        .unwrap();
        let tool = GrepTool::new(roots_at(dir.clone()));
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": r"fn\s+\w+", "path": dir.to_string_lossy() }),
            )
            .await
            .unwrap();
        assert!(out.contains("fn main"), "got: {out}");
        assert!(out.contains("fn helper"), "got: {out}");
        assert!(out.contains("Found 2"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn grok_context_and_ignore_case_aliases() {
        let dir = std::env::temp_dir().join(format!("boris-grep-c-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\nFINDME\nomega\n").unwrap();
        let tool = GrepTool::new(roots_at(dir.clone()));
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({
                    "pattern": "findme",
                    "path": dir.to_string_lossy(),
                    "-i": true,
                    "-C": 1
                }),
            )
            .await
            .unwrap();
        assert!(out.contains("alpha"), "got: {out}");
        assert!(out.contains("FINDME"), "got: {out}");
        assert!(out.contains("omega"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_pattern_rejected() {
        let roots = roots_at(std::env::temp_dir());
        let tool = GrepTool::new(roots);
        let err = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": "  " }),
            )
            .await
            .unwrap_err();
        assert!(err.message.contains("empty") || err.message.contains("pattern"));
    }

    #[tokio::test]
    async fn missing_path_is_actionable() {
        let dir = std::env::temp_dir().join(format!("boris-grep-miss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = GrepTool::new(roots_at(dir.clone()));
        let missing = dir.join("nope-not-here");
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": "x", "path": missing.to_string_lossy() }),
            )
            .await
            .unwrap();
        assert!(out.to_ascii_lowercase().contains("not found"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_detect_literals_vs_meta() {
        assert!(!looks_like_regex("TODO"));
        assert!(!looks_like_regex("needle-small"));
        assert!(!looks_like_regex("file.rs"));
        assert!(!looks_like_regex("foo.bar.baz"));
        assert!(looks_like_regex("foo.*bar"));
        assert!(looks_like_regex("a.+b"));
        assert!(looks_like_regex("^fn"));
        assert!(looks_like_regex("end$"));
        assert!(looks_like_regex("a|b"));
        assert!(looks_like_regex(r"\d+"));
        assert!(looks_like_regex("foo[0-9]"));
    }

    #[test]
    fn never_spawn_rg_for_a_single_file() {
        let dir = std::env::temp_dir().join(format!("boris-grep-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("only.txt");
        std::fs::write(&file, "needle\n").unwrap();
        assert!(
            !should_spawn_rg(&file),
            "single-file grep must stay in-process"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
