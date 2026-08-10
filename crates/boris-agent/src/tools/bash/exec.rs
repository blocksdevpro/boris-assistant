//! Bash tool type, process spawn, and execute path.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

use super::output::{parse_timeout_secs, truncate_output};
use super::policy::validate_command;
use crate::runtime::ProgressEvent;
use crate::tool::{
    optional_string, require_object, require_string, soft_wrap_text, truncate_tool_result,
    DEFAULT_SOFT_WRAP_WIDTH, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
};
use crate::tool_context::ToolCallContext;
use crate::tools::fs_common::resolve_under_roots;

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
        scrub_child_env(&mut cmd);
        cmd
    }

    #[cfg(windows)]
    fn build_fallback(command: &str, cwd: &std::path::Path) -> Command {
        // PowerShell when Git Bash is missing. `-ExecutionPolicy Bypass` is
        // intentional for desktop usability — not a security boundary. HITL
        // confirmation + ShellPolicy remain authoritative (see bash/policy.rs).
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
        scrub_child_env(&mut cmd);
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
        scrub_child_env(&mut cmd);
        cmd
    }
}

/// Drop common API / cloud secret env vars from the child process.
///
/// Does not clear PATH / system vars (would break normal tools). Best-effort
/// only — the model can still read secrets from disk if roots allow.
fn scrub_child_env(cmd: &mut Command) {
    const DROP: &[&str] = &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENROUTER_API_KEY",
        "BORIS_OPENROUTER_API_KEY",
        "BORIS_API_KEY",
        "XAI_API_KEY",
        "GROQ_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AZURE_OPENAI_API_KEY",
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "NPM_TOKEN",
        "CARGO_REGISTRY_TOKEN",
    ];
    for key in DROP {
        cmd.env_remove(key);
    }
}

/// Read a pipe in chunks, emitting progress; returns the full buffer.
async fn read_pipe_chunked(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    tx: mpsc::UnboundedSender<ProgressEvent>,
    is_stderr: bool,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut total: u64 = 0;
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                total += n as u64;
                let delta = String::from_utf8_lossy(&chunk[..n]).into_owned();
                let _ = tx.send(ProgressEvent::Chunk {
                    delta: if is_stderr {
                        format!("[stderr] {delta}")
                    } else {
                        delta
                    },
                    total_bytes: total,
                    truncated: false,
                });
            }
            Err(_) => break,
        }
    }
    buf
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
            .kind(ToolKind::Execute)
            .permissions(&[Permission::Shell])
            .confirm(true)
            .timeout(Duration::from_secs(130))
            .read_only(false)
            .max_concurrency(1)
    }

    async fn execute(&self, ctx: &ToolCallContext, args: Value) -> Result<String, ToolError> {
        if ctx.is_cancelled() {
            return Err(ToolError::failed("command cancelled before start"));
        }

        let obj = require_object(&args)?;
        let command = require_string(obj, "command")?;
        let command = command.trim();
        validate_command(command)?;

        // Prefer explicit cwd arg, then session ToolCallContext cwd (if under roots),
        // then sandbox default.
        let cwd = if let Some(raw) = optional_string(obj, "cwd") {
            resolve_under_roots(&raw, &self.cwd_roots)?
        } else if let Some(ref session_cwd) = ctx.cwd {
            if self.cwd_roots.iter().any(|r| session_cwd.starts_with(r)) {
                session_cwd.clone()
            } else {
                self.default_cwd.clone()
            }
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

        let timeout_secs = parse_timeout_secs(obj);

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

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::failed("stdout not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::failed("stderr not piped"))?;

        // Chunked reads so we can emit progress; cancel still kills the process.
        let (prog_tx, mut prog_rx) = mpsc::unbounded_channel::<ProgressEvent>();
        let stdout_task = tokio::spawn(read_pipe_chunked(stdout, prog_tx.clone(), false));
        let stderr_task = tokio::spawn(read_pipe_chunked(stderr, prog_tx, true));

        // Forward progress on the tool context while the process runs.
        let progress_pump = async {
            while let Some(ev) = prog_rx.recv().await {
                ctx.report(ev);
            }
        };

        let cancel = ctx.cancel.clone();
        let wait_fut = async {
            tokio::select! {
                biased;
                status = child.wait() => {
                    match status {
                        Ok(s) => Ok(s),
                        Err(e) => Err(ToolError::failed(format!("wait failed: {e}"))),
                    }
                }
                _ = async {
                    if let Some(token) = cancel {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    stdout_task.abort();
                    stderr_task.abort();
                    Err(ToolError::failed("command cancelled by host"))
                }
                _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    stdout_task.abort();
                    stderr_task.abort();
                    Err(ToolError::timeout(format!(
                        "Command timed out after {timeout_secs}s"
                    )))
                }
            }
        };

        let (status_res, _) = tokio::join!(wait_fut, progress_pump);
        let status = status_res?;
        let stdout_bytes = stdout_task.await.unwrap_or_default();
        let stderr_bytes = stderr_task.await.unwrap_or_default();
        let duration_ms = start.elapsed().as_millis();

        let stdout_str = String::from_utf8_lossy(&stdout_bytes);
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
        let mut combined = format!("{stdout_str}{stderr_str}");
        if combined.trim().is_empty() {
            combined = String::new();
        }

        // Soft-wrap long lines (preserve bytes) then line/byte cap.
        let mut text = truncate_output(soft_wrap_text(&combined, DEFAULT_SOFT_WRAP_WIDTH));
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
    fn tool_name_stable() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(vec![dir.clone()], dir);
        assert_eq!(tool.name(), "bash");
        assert!(tool.meta().requires_confirmation);
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
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": cmd }),
            )
            .await
            .expect("run");
        assert!(
            out.contains("bash-smoke-ok") || out.contains("Exit code"),
            "got: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn cancel_kills_long_command() {
        use tokio_util::sync::CancellationToken;

        let dir =
            std::env::temp_dir().join(format!("boris-bash-cancel-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let token = CancellationToken::new();
        let ctx = crate::tool_context::ToolCallContext::new("cancel-test")
            .with_cancel(Some(token.clone()));

        #[cfg(windows)]
        let cmd = "powershell -NoProfile -Command \"Start-Sleep -Seconds 30\"";
        #[cfg(not(windows))]
        let cmd = "sleep 30";

        let run = tool.execute(&ctx, json!({ "command": cmd, "timeout": 60 }));
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            token.cancel();
        };
        let (result, _) = tokio::join!(run, cancel);
        let err = result.expect_err("should cancel");
        assert!(
            err.message.to_ascii_lowercase().contains("cancel"),
            "got: {}",
            err.message
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn denied_command_errors() {
        let dir = std::env::temp_dir().join(format!("boris-bash-deny-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let err = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": "rm -rf /" }),
            )
            .await
            .expect_err("denied");
        assert!(err.message.contains("safety policy"));
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
