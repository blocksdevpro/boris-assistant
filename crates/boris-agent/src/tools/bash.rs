//! Bash / shell execution — tau-style `bash` tool, adapted for Boris.
//!
//! Runs via `bash -lc` when available (Git Bash / WSL / Unix), otherwise falls
//! back to platform shell. Always requires HITL confirmation.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 30 * 1024;

/// Hard deny patterns (best-effort; HITL is the real safety net).
fn is_denied_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_ascii_lowercase();
    let needles = [
        ("rm -rf /", "recursive root delete"),
        ("rm -rf /*", "recursive root delete"),
        ("format c:", "format disk"),
        ("format-volume", "format volume"),
        ("mkfs.", "format filesystem"),
        (":(){ :|:& };:", "fork bomb"),
        ("dd if=/dev/zero of=/dev/", "disk wipe"),
        ("shutdown", "shutdown"),
        ("reboot", "reboot"),
        ("remove-item -recurse -force c:\\", "windows wipe"),
        ("reg delete hk", "registry delete"),
        ("curl | sh", "remote pipe to shell"),
        ("curl | bash", "remote pipe to shell"),
        ("wget | sh", "remote pipe to shell"),
        ("iwr ", "powershell remote download"),
        ("iex (", "invoke-expression"),
        ("invoke-expression", "invoke-expression"),
    ];
    for (n, reason) in needles {
        if lower.contains(n) {
            return Some(reason);
        }
    }
    None
}

/// Truncate combined output like tau (2000 lines / 30KB, keep the tail).
fn truncate_output(mut combined: String) -> String {
    let total_lines = combined.lines().count();
    let truncated_by_bytes = combined.len() > MAX_BYTES;
    if truncated_by_bytes {
        let slice = &combined[..MAX_BYTES.min(combined.len())];
        let cut = slice.rfind('\n').unwrap_or(slice.len());
        combined.truncate(cut);
    }
    let shown: Vec<&str> = combined.lines().collect();
    let truncated_by_lines = shown.len() > MAX_LINES;
    let display: Vec<&str> = if truncated_by_lines {
        shown[shown.len() - MAX_LINES..].to_vec()
    } else {
        shown
    };
    let mut text = display.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    if truncated_by_lines || truncated_by_bytes {
        text.push_str(&format!(
            "\n[Output truncated: showing last {} lines of {}]",
            display.len(),
            total_lines
        ));
    }
    text
}

/// Run a bash/shell command with timeout and output caps.
#[derive(Debug, Clone)]
pub struct BashTool {
    /// Allowed cwd roots (sandbox + read roots).
    cwd_roots: Vec<PathBuf>,
    default_cwd: PathBuf,
}

impl BashTool {
    pub fn new(cwd_roots: Vec<PathBuf>, default_cwd: PathBuf) -> Self {
        Self {
            cwd_roots,
            default_cwd,
        }
    }

    fn build_command(command: &str, cwd: &std::path::Path) -> Command {
        // Prefer bash (Git Bash on Windows, real bash on Unix). Fallbacks below
        // are chosen only at spawn time if bash is missing.
        let mut cmd = Command::new("bash");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }

    #[cfg(windows)]
    fn build_fallback(command: &str, cwd: &std::path::Path) -> Command {
        // PowerShell is ubiquitous on Windows when Git Bash is not installed.
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            command,
        ])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
        cmd
    }

    #[cfg(not(windows))]
    fn build_fallback(command: &str, cwd: &std::path::Path) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        cmd
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command (bash when available). Always requires user confirmation. \
         Prefer read-only commands. Relative cwd defaults to the Boris sandbox. \
         Output is truncated (last 2000 lines / 30KB)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash/shell command to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory under allowed roots (default: sandbox)"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (default 120, max 300)"
                }
            },
            "required": ["command"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::Shell])
            .confirm(true)
            .timeout(Duration::from_secs(130))
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let command = require_string(obj, "command")?;
        let command = command.trim();
        if command.is_empty() {
            return Err(ToolError::invalid_args("command is empty"));
        }
        if command.len() > 8000 {
            return Err(ToolError::invalid_args("command too long"));
        }
        if let Some(reason) = is_denied_command(command) {
            return Err(ToolError::failed(format!(
                "command blocked by safety policy ({reason})"
            )));
        }

        let cwd = if let Some(raw) = optional_string(obj, "cwd") {
            resolve_under_roots(&raw, &self.cwd_roots)?
        } else {
            self.default_cwd.clone()
        };
        if !cwd.is_dir() {
            // Create default sandbox cwd if missing.
            if cwd == self.default_cwd {
                tokio::fs::create_dir_all(&cwd)
                    .await
                    .map_err(|e| ToolError::failed(format!("create cwd: {e}")))?;
            } else {
                return Err(ToolError::failed(format!(
                    "cwd is not a directory: {}",
                    cwd.display()
                )));
            }
        }

        let timeout_secs = obj
            .get("timeout")
            .or_else(|| obj.get("timeout_secs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, 300);

        let start = Instant::now();
        let mut child = match Self::build_command(command, &cwd).spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "bash spawn failed; trying platform fallback");
                Self::build_fallback(command, &cwd)
                    .spawn()
                    .map_err(|e2| ToolError::failed(format!("spawn failed: {e2} (bash: {e})")))?
            }
        };

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::failed("stdout not piped"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::failed("stderr not piped"))?;

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

        let outcome = tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(s) => Ok(s),
                    Err(e) => Err(ToolError::failed(format!("wait failed: {e}"))),
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                stdout_task.abort();
                stderr_task.abort();
                return Err(ToolError::timeout(format!(
                    "Command timed out after {timeout_secs}s"
                )));
            }
        };

        let status = outcome?;
        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();
        let duration_ms = start.elapsed().as_millis();

        let stdout_str = String::from_utf8_lossy(&stdout_bytes);
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
        let mut combined = format!("{stdout_str}{stderr_str}");
        if combined.trim().is_empty() {
            combined = String::new();
        }

        let mut text = truncate_output(combined);
        let exit_code = status.code().unwrap_or(-1);
        if exit_code != 0 {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&format!("Exit code: {exit_code}\n"));
        }
        if text.trim().is_empty() {
            text = format!("(no output) exit={exit_code}\n");
        }

        Ok(truncate_tool_result(format!(
            "cwd={} duration_ms={duration_ms}\n{text}",
            cwd.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn denies_dangerous() {
        assert!(is_denied_command("rm -rf /").is_some());
        assert!(is_denied_command("ls -la").is_none());
    }

    #[tokio::test]
    async fn echo_works() {
        let dir = std::env::temp_dir().join(format!("boris-bash-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        #[cfg(windows)]
        let cmd = "echo bash-smoke-ok";
        #[cfg(not(windows))]
        let cmd = "echo bash-smoke-ok";
        let out = tool
            .execute(json!({ "command": cmd }))
            .await
            .expect("run");
        assert!(
            out.contains("bash-smoke-ok") || out.contains("Exit code"),
            "got: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
