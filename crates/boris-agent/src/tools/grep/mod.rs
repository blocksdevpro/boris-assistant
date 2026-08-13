//! Content search — in-process first; `rg` only when a regex would need it.
//!
//! # Tool
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `grep` | [`GrepTool`] | Regex/text search under allowed roots |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | (this) | Tool surface + execute orchestration |
//! | [`rg`] | Spawn / parse ripgrep (regex / large trees) |
//! | [`fallback`] | Pure-Rust walk + substring search |
//!
//! Name filters reuse [`crate::tools::path_pattern`].
//!
//! Spawning `rg` on Windows is ~30–60ms even for one file, so literals and
//! single-file searches stay in-process. `rg` is used only when the pattern
//! looks like a regex *and* `rg` is on PATH (probed once, cached).

mod fallback;
mod rg;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tools::files::FsRoots;
use crate::tools::fs_common::resolve_under_roots;

use fallback::rust_grep;
use rg::run_rg;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;

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
        "Search file contents by regex/text under allowed paths. \
         Returns path:line:content matches."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search (default: sandbox)"
                },
                "glob": {
                    "type": "string",
                    "description": "Optional file filter, e.g. '*.rs'"
                },
                "ignore_case": {
                    "type": "boolean",
                    "description": "Case-insensitive search (default false)"
                },
                "limit": {
                    "type": "number",
                    "description": "Max matching lines (default 100)"
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
        let pattern = require_string(obj, "pattern")?;
        if pattern.trim().is_empty() {
            return Err(ToolError::invalid_args("pattern is empty"));
        }
        let raw_path = optional_string(obj, "path")
            .unwrap_or_else(|| self.roots.sandbox.to_string_lossy().into_owned());
        let search_path = resolve_under_roots(&raw_path, &self.roots.readers())?;
        let ignore_case = obj
            .get("ignore_case")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        let glob_filter = optional_string(obj, "glob");

        // Process spawn is the expensive part (~30–60ms on Windows). Stay
        // in-process for literals and single files; only pay `rg` when the
        // pattern actually needs a regex engine.
        if should_spawn_rg(&search_path, &pattern) {
            match run_rg(
                &pattern,
                &search_path,
                ignore_case,
                glob_filter.as_deref(),
                limit,
            )
            .await
            {
                Ok(out) => return Ok(out),
                Err(e) => {
                    tracing::debug!(error = %e, "rg failed; using Rust fallback");
                }
            }
        }

        // In-process walk (substring). Fast on sandbox-sized trees.
        let pattern_owned = pattern.clone();
        let path_owned = search_path.clone();
        let glob_owned = glob_filter.clone();
        let matches = tokio::task::spawn_blocking(move || {
            rust_grep(
                &pattern_owned,
                &path_owned,
                ignore_case,
                glob_owned.as_deref(),
                limit,
            )
        })
        .await
        .map_err(|e| ToolError::failed(format!("grep task: {e}")))??;

        if matches.is_empty() {
            return Ok(format!("No matches for pattern '{pattern}'."));
        }
        Ok(truncate_tool_result(matches.join("\n")))
    }
}

/// Whether this call should pay for an `rg` process.
///
/// Literals and single-file searches are faster in-process. `rg` is worth it
/// when the pattern uses regex metacharacters *and* ripgrep is installed.
pub(super) fn should_spawn_rg(search_path: &std::path::Path, pattern: &str) -> bool {
    if search_path.is_file() {
        return false;
    }
    looks_like_regex(pattern) && rg::rg_available()
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

    #[tokio::test]
    async fn rust_fallback_finds_line() {
        let dir = std::env::temp_dir().join(format!("boris-grep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\nfindme please\nbye\n").unwrap();
        let roots = FsRoots {
            sandbox: dir.clone(),
            data: vec![],
            allow_read: vec![],
            allow_write: vec![],
        };
        let tool = GrepTool::new(roots);
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": "findme", "path": dir.to_string_lossy() }),
            )
            .await
            .unwrap();
        assert!(out.contains("findme"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_pattern_rejected() {
        let roots = FsRoots {
            sandbox: std::env::temp_dir(),
            data: vec![],
            allow_read: vec![],
            allow_write: vec![],
        };
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
            !should_spawn_rg(&file, "foo.*bar"),
            "single-file grep must stay in-process even for regex"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
