//! Shell execution (PowerShell on Windows). Always requires confirmation.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::tool::{
    optional_string, require_object, require_string, truncate_tool_result, Permission, Tool,
    ToolError, ToolMeta, ToolRisk,
};
use crate::tools::fs_common::resolve_under_roots;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_OUTPUT_BYTES: usize = 8 * 1024;

/// Hard deny patterns (best-effort; HITL is the real safety net).
fn is_denied_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_ascii_lowercase();
    let needles = [
        ("format-volume", "format-volume"),
        ("format c:", "format disk"),
        ("remove-item -recurse", "recursive delete"),
        ("rm -rf", "recursive delete"),
        ("del /f /s", "force delete"),
        ("rmdir /s", "recursive rmdir"),
        ("invoke-webrequest", "remote script download"),
        ("iwr ", "remote script download"),
        ("iex ", "invoke-expression"),
        ("invoke-expression", "invoke-expression"),
        ("downloadstring", "remote script"),
        ("reg delete", "registry delete"),
        ("shutdown", "shutdown"),
        ("stop-computer", "shutdown"),
        ("restart-computer", "reboot"),
        ("remove-windows", "windows removal"),
    ];
    for (n, reason) in needles {
        if lower.contains(n) {
            return Some(reason);
        }
    }
    None
}

/// Run a shell command with timeout and output caps. Always confirms.
#[derive(Debug, Clone)]
pub struct RunCommandTool {
    /// Allowed cwd roots (sandbox + read roots).
    cwd_roots: Vec<PathBuf>,
    default_cwd: PathBuf,
}

impl RunCommandTool {
    pub fn new(cwd_roots: Vec<PathBuf>, default_cwd: PathBuf) -> Self {
        Self {
            cwd_roots,
            default_cwd,
        }
    }
}

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Run a shell command (PowerShell on Windows). Always requires user confirmation. Prefer simple read-only commands. Output is truncated."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to run"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory under allowed roots (default sandbox)"
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Timeout in seconds (default 30, max 120)"
                }
            },
            "required": ["command"]
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::Shell])
            .confirm(true)
            .timeout(Duration::from_secs(90))
    }

    async fn execute(&self, args: Value) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let command = require_string(obj, "command")?;
        let command = command.trim();
        if command.is_empty() {
            return Err(ToolError::invalid_args("command is empty"));
        }
        if command.len() > 4000 {
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
            return Err(ToolError::failed(format!(
                "cwd is not a directory: {}",
                cwd.display()
            )));
        }

        let timeout_secs = obj
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, 120);

        #[cfg(windows)]
        let mut child_cmd = {
            let mut c = Command::new("powershell.exe");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                command,
            ]);
            c
        };
        #[cfg(not(windows))]
        let mut child_cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", command]);
            c
        };

        child_cmd
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .stdin(Stdio::null());

        let mut child = child_cmd
            .spawn()
            .map_err(|e| ToolError::failed(format!("spawn failed: {e}")))?;

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let reader = async {
            let mut out_buf = Vec::new();
            let mut err_buf = Vec::new();
            if let Some(ref mut s) = stdout {
                let _ = s.read_to_end(&mut out_buf).await;
            }
            if let Some(ref mut s) = stderr {
                let _ = s.read_to_end(&mut err_buf).await;
            }
            (out_buf, err_buf)
        };

        let wait = async {
            child
                .wait()
                .await
                .map_err(|e| ToolError::failed(format!("wait failed: {e}")))
        };

        let (status, out_buf, err_buf) =
            match timeout(Duration::from_secs(timeout_secs), async {
                let ((out_buf, err_buf), status) = tokio::join!(reader, wait);
                Ok::<_, ToolError>((status?, out_buf, err_buf))
            })
            .await
            {
                Ok(r) => r?,
                Err(_) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return Err(ToolError::timeout(format!(
                        "command timed out after {timeout_secs}s"
                    )));
                }
            };

        let mut out = String::from_utf8_lossy(&out_buf).into_owned();
        let err = String::from_utf8_lossy(&err_buf);
        if !err.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("--- stderr ---\n");
            out.push_str(&err);
        }
        if out.len() > MAX_OUTPUT_BYTES {
            out.truncate(MAX_OUTPUT_BYTES);
            out.push_str("\n…[truncated]");
        }
        if out.trim().is_empty() {
            out = "(no output)".into();
        }

        let code = status.code().unwrap_or(-1);
        Ok(truncate_tool_result(format!(
            "exit={code} cwd={}\n{out}",
            cwd.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_rm_rf() {
        assert!(is_denied_command("rm -rf /").is_some());
        assert!(is_denied_command("Get-ChildItem").is_none());
    }

    #[tokio::test]
    async fn echo_works() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("boris-shell-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = RunCommandTool::new(vec![dir.clone()], dir.clone());
        #[cfg(windows)]
        let cmd = "Write-Output 'hello-boris'";
        #[cfg(not(windows))]
        let cmd = "echo hello-boris";
        let out = tool
            .execute(json!({ "command": cmd }))
            .await
            .expect("run");
        assert!(out.contains("hello-boris"), "got: {out}");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
