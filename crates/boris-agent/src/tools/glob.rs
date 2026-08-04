//! Find files by glob pattern (tau-inspired, pure Rust — no gitignore deps).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};
use crate::tools::files::FsRoots;
use crate::tools::fs_common::resolve_under_roots;

const MAX_RESULTS: usize = 200;

/// Match a path relative to root against a simple glob (`*`, `**`, `?`).
fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    // Strip leading ./
    let path = path.strip_prefix("./").unwrap_or(&path);
    glob_match_inner(pattern.as_str(), path)
}

fn glob_match_inner(pattern: &str, path: &str) -> bool {
    // Handle ** specially
    if let Some(rest) = pattern.strip_prefix("**/") {
        // Match rest at any depth
        if glob_match_inner(rest, path) {
            return true;
        }
        // Consume one path segment and retry
        if let Some((_, tail)) = path.split_once('/') {
            return glob_match_inner(pattern, tail);
        }
        return glob_match_inner(rest, path);
    }
    if pattern == "**" {
        return true;
    }

    let (pat_seg, pat_rest) = match pattern.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (pattern, None),
    };
    let (path_seg, path_rest) = match path.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };

    if !seg_match(pat_seg, path_seg) {
        return false;
    }
    match (pat_rest, path_rest) {
        (None, None) => true,
        (Some(pr), Some(phr)) => glob_match_inner(pr, phr),
        (None, Some(_)) => false,
        (Some(pr), None) => pr.is_empty() || pr == "**" || pr.starts_with("**/"),
    }
}

fn seg_match(pat: &str, seg: &str) -> bool {
    if pat == "*" {
        return true;
    }
    let pb: Vec<char> = pat.chars().collect();
    let sb: Vec<char> = seg.chars().collect();
    let mut pi = 0usize;
    let mut si = 0usize;
    let mut star = None::<(usize, usize)>;
    while si < sb.len() {
        if pi < pb.len() && (pb[pi] == sb[si] || pb[pi] == '?') {
            pi += 1;
            si += 1;
        } else if pi < pb.len() && pb[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, si));
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == '*' {
        pi += 1;
    }
    pi == pb.len()
}

fn walk_collect(root: &Path, pattern: &str, out: &mut Vec<(PathBuf, SystemTime)>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                // Skip common junk
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if matches!(
                        name,
                        "node_modules" | ".git" | "target" | ".boris" | "__pycache__"
                    ) {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_s = rel.to_string_lossy();
            if glob_match(pattern, &rel_s) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, mtime));
            }
        }
    }
}

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
        ToolMeta::with_risk(ToolRisk::Safe).permissions(&[Permission::FsRead])
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
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

    #[test]
    fn glob_basic() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b.rs"));
        assert!(!glob_match("src/**/*.rs", "lib/a.rs"));
    }
}
