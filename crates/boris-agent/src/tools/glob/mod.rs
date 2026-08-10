//! Find files by glob pattern (tau-inspired, pure Rust — no gitignore deps).
//!
//! # Tool
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `glob` | [`GlobTool`] | Walk allowed roots; return newest-first paths |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | (this) | Tool surface + execute |
//! | [`walk`] | Directory walk + collect matches |
//!
//! Pattern matching lives in [`crate::tools::path_pattern`] (shared with grep).

mod walk;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tools::files::FsRoots;
use crate::tools::fs_common::resolve_under_roots;

use walk::walk_collect;

const MAX_RESULTS: usize = 200;

#[derive(Debug, Clone)]
pub struct GlobTool {
    roots: FsRoots,
}

impl GlobTool {
    pub fn new(roots: FsRoots) -> Self {
        Self { roots }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern under allowed roots (e.g. '**/*.rs', 'notes/*.md'). \
         Returns paths sorted newest-first. Relative search path defaults to sandbox."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. '**/*.rs' or '*.txt'"
                },
                "path": {
                    "type": "string",
                    "description": "Root directory to search (default: sandbox)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Search)
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
        let pattern = require_string(obj, "pattern")?;
        if pattern.trim().is_empty() {
            return Err(ToolError::invalid_args("pattern is empty"));
        }
        let raw_root = optional_string(obj, "path")
            .unwrap_or_else(|| self.roots.sandbox.to_string_lossy().into_owned());
        let root = resolve_under_roots(&raw_root, &self.roots.readers())?;
        if !root.is_dir() {
            return Err(ToolError::failed(format!(
                "not a directory: {}",
                root.display()
            )));
        }

        let pattern_owned = pattern.clone();
        let root_owned = root.clone();
        let mut matches = tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            walk_collect(&root_owned, &pattern_owned, &mut found);
            found
        })
        .await
        .map_err(|e| ToolError::failed(format!("glob task: {e}")))?;

        if matches.is_empty() {
            return Ok(format!(
                "No files matched pattern '{pattern}' under {}.",
                root.display()
            ));
        }

        matches.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
        let total = matches.len();
        let truncated = total > MAX_RESULTS;
        let shown = &matches[..total.min(MAX_RESULTS)];

        let mut lines = Vec::with_capacity(shown.len());
        for (p, _) in shown {
            lines.push(p.display().to_string());
        }
        let mut out = format!(
            "Glob '{pattern}' under {} — {} match(es):\n{}",
            root.display(),
            total,
            lines.join("\n")
        );
        if truncated {
            out.push_str(&format!("\n…[truncated to {MAX_RESULTS}]"));
        }
        Ok(truncate_tool_result(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::path_pattern::glob_match;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn glob_pattern_smoke() {
        // Keep a tool-level smoke that pattern wiring still works.
        assert!(glob_match("**/*.rs", "src/main.rs"));
    }

    #[tokio::test]
    async fn finds_file_under_sandbox() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-glob-{n}"));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("readme.md"), "# hi").unwrap();

        let roots = FsRoots {
            sandbox: dir.clone(),
            data: vec![],
            allow_read: vec![],
            allow_write: vec![],
        };
        let tool = GlobTool::new(roots);
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "pattern": "**/*.rs", "path": dir.to_string_lossy() }),
            )
            .await
            .unwrap();
        assert!(out.contains("main.rs"), "got: {out}");
        assert!(!out.contains("readme.md"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
