//! Content search — prefers `rg` (ripgrep) like tau; pure-Rust fallback if missing.
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
//! | [`rg`] | Spawn / parse ripgrep |
//! | [`fallback`] | Pure-Rust walk + substring search |
//!
//! Name filters reuse [`crate::tools::path_pattern`].

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
        "Search file contents by regex/text under allowed paths. Uses ripgrep when available. \
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

        // Try ripgrep first (tau path).
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
                tracing::debug!(error = %e, "rg unavailable or failed; using Rust fallback");
            }
        }

        // Pure Rust fallback
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
}
