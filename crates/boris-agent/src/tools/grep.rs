//! Content search — prefers `rg` (ripgrep) like tau; pure-Rust fallback if missing.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};
use crate::tools::files::FsRoots;
use crate::tools::fs_common::resolve_under_roots;

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
            .permissions(&[Permission::FsRead])
            .timeout(Duration::from_secs(30))
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
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

async fn run_rg(
    pattern: &str,
    search_path: &std::path::Path,
    ignore_case: bool,
    glob: Option<&str>,
    limit: usize,
) -> Result<String, ToolError> {
    let mut args: Vec<String> = vec![
        "-n".into(),
        "--color=never".into(),
        "--no-heading".into(),
        format!("--max-count={limit}"),
    ];
    if ignore_case {
        args.push("-i".into());
    }
    if let Some(g) = glob {
        args.push("--glob".into());
        args.push(g.to_string());
    }
    args.push(pattern.to_string());
    args.push(search_path.to_string_lossy().into_owned());

    let mut child = Command::new("rg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ToolError::failed(format!("rg spawn: {e}")))?;

    let mut stdout = child.stdout.take().ok_or_else(|| ToolError::failed("no stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| ToolError::failed("no stderr"))?;

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .map_err(|_| {
            let _ = child.start_kill();
            ToolError::timeout("grep timed out after 30s")
        })?
        .map_err(|e| ToolError::failed(format!("rg wait: {e}")))?;

    let out = stdout_task.await.unwrap_or_default();
    let err = stderr_task.await.unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    // rg exit 1 = no matches; 2 = error
    if code == 2 {
        let e = String::from_utf8_lossy(&err);
        return Err(ToolError::failed(format!("rg error: {e}")));
    }
    let text = String::from_utf8_lossy(&out);
    if text.trim().is_empty() {
        return Ok(format!("No matches for pattern '{pattern}'."));
    }
    let lines: Vec<&str> = text.lines().take(limit).collect();
    Ok(truncate_tool_result(lines.join("\n")))
}

fn rust_grep(
    pattern: &str,
    search_path: &std::path::Path,
    ignore_case: bool,
    glob: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let pat_lower = if ignore_case {
        pattern.to_ascii_lowercase()
    } else {
        pattern.to_string()
    };

    let mut out = Vec::new();
    if search_path.is_file() {
        grep_file(search_path, &pat_lower, ignore_case, limit, &mut out);
        return Ok(out);
    }

    let mut stack = vec![search_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if matches!(name, "node_modules" | ".git" | "target" | "__pycache__") {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            if let Some(g) = glob {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let rel = path
                    .strip_prefix(search_path)
                    .unwrap_or(&path)
                    .to_string_lossy();
                if !simple_name_glob(g, name) && !simple_name_glob(g, &rel) {
                    continue;
                }
            }
            grep_file(&path, &pat_lower, ignore_case, limit - out.len(), &mut out);
        }
    }
    Ok(out)
}

fn simple_name_glob(pat: &str, name: &str) -> bool {
    // Reuse minimal matcher for file names like *.rs
    let pat = pat.trim_start_matches("**/");
    if pat.contains('/') {
        return false;
    }
    let pb: Vec<char> = pat.chars().collect();
    let sb: Vec<char> = name.chars().collect();
    let mut pi = 0;
    let mut si = 0;
    let mut star = None;
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

fn grep_file(
    path: &std::path::Path,
    pattern: &str,
    ignore_case: bool,
    remaining: usize,
    out: &mut Vec<String>,
) {
    if remaining == 0 {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if bytes.iter().take(256).any(|&b| b == 0) {
        return;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return;
    };
    for (i, line) in text.lines().enumerate() {
        if out.len() >= remaining {
            break;
        }
        let hay = if ignore_case {
            line.to_ascii_lowercase()
        } else {
            line.to_string()
        };
        // Substring search (not full regex) for fallback.
        if hay.contains(pattern) {
            out.push(format!("{}:{}:{}", path.display(), i + 1, line));
        }
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
            .execute(json!({ "pattern": "findme", "path": dir.to_string_lossy() }))
            .await
            .unwrap();
        assert!(out.contains("findme"), "got: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
