//! Bash tool type, process spawn, and execute path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

use super::jobs::ShellSession;
use super::output::{parse_timeout_secs, truncate_output};
use super::policy::validate_command;
use super::steer::steer_simple_command;
use super::CAPTURE_MAX_BYTES;
use crate::runtime::ProgressEvent;
use crate::tool::{
    optional_bool, optional_string, optional_string_keys, require_object, soft_wrap_text,
    truncate_tool_result, Permission, Tool, ToolError, ToolKind, ToolMeta, ToolRisk,
    DEFAULT_SOFT_WRAP_WIDTH,
};
use crate::tool_context::ToolCallContext;
use crate::tools::fs_common::resolve_under_roots;

/// Capacity of the progress channel (drop-on-full via `try_send`).
const PROGRESS_CHANNEL_CAP: usize = 32;

const BASH_DESCRIPTION: &str = "Run a bash command and return its output.

Use this only for real system commands (git, cargo, npm, python, builds, tests). \
Prefer dedicated tools when they exist:
- file_read instead of cat/head/tail/type
- grep instead of grep/rg/findstr
- glob / list_dir instead of find/ls/dir
- file_edit / file_write instead of sed/awk
NEVER use bash echo (or any shell) to talk to the user — speak after tools.

Git Bash is invoked as `bash -c` (not a login shell). Sequential commands may use `&&`. \
If output exceeds 30KB / 2000 lines the middle is truncated (head and tail are kept).

Working directory: set `cwd` once; later calls in this session reuse the last cwd until you \
set another (default: the Boris sandbox). A `cd` inside a command does not carry over.
Long-running work (dev servers, watchers, long builds): pass `run_in_background: true` to get \
a job id at once, then read new output with bash_output and stop it with bash_kill. Background \
jobs end with the session. Always requires user confirmation.";

#[cfg(windows)]
const POWERSHELL_DESCRIPTION: &str = "Run a shell command and return its output.

This session's fallback shell is PowerShell. The Unix utilities `grep`, `head`, `tail`, \
`sed`, `awk`, and `find` are NOT available — use the dedicated tools instead:
- file_read instead of Get-Content/type/cat
- grep instead of Select-String/findstr
- glob / list_dir instead of Get-ChildItem/dir
- file_edit / file_write instead of rewriting files by hand
NEVER use echo/Write-Output to talk to the user — speak after tools.

Use this only for real system commands (git, cargo, npm, python, builds, tests). \
If output exceeds 30KB / 2000 lines the middle is truncated (head and tail are kept).

Working directory: set `cwd` once; later calls in this session reuse the last cwd until you \
set another (default: the Boris sandbox). A `cd`/Set-Location inside a command does not carry over.
Long-running work (dev servers, watchers, long builds): pass `run_in_background: true` to get \
a job id at once, then read new output with bash_output and stop it with bash_kill. Background \
jobs end with the session. Always requires user confirmation.";

/// Run a bash/shell command with timeout and output caps.
#[derive(Debug, Clone)]
pub struct BashTool {
    /// Allowed cwd roots (sandbox + read roots).
    cwd_roots: Vec<PathBuf>,
    default_cwd: PathBuf,
    /// Session cwd + background jobs, shared with `bash_output` / `bash_kill`.
    shell: Arc<ShellSession>,
}

impl BashTool {
    /// Standalone tool with its own [`ShellSession`].
    pub fn new(cwd_roots: Vec<PathBuf>, default_cwd: PathBuf) -> Self {
        Self::with_session(cwd_roots, default_cwd, ShellSession::new())
    }

    /// Tool bound to a shared [`ShellSession`] (the agent's).
    pub fn with_session(
        cwd_roots: Vec<PathBuf>,
        default_cwd: PathBuf,
        shell: Arc<ShellSession>,
    ) -> Self {
        Self {
            cwd_roots,
            default_cwd,
            shell,
        }
    }

    fn build_command(command: &str, cwd: &Path) -> Command {
        piped(shell_command(command, cwd))
    }

    fn build_fallback(command: &str, cwd: &Path) -> Command {
        piped(fallback_command(command, cwd))
    }

    /// Explicit `cwd` arg, else the session's last cwd, else the host cwd,
    /// else the sandbox — each only while it is a directory under the roots.
    fn pick_cwd(
        &self,
        ctx: &ToolCallContext,
        obj: &serde_json::Map<String, Value>,
    ) -> Result<PathBuf, ToolError> {
        if let Some(raw) = optional_string(obj, "cwd") {
            return resolve_under_roots(&raw, &self.cwd_roots);
        }
        let under_roots = |p: &PathBuf| self.cwd_roots.iter().any(|r| p.starts_with(r));
        if let Some(last) = self.shell.cwd().filter(|p| p.is_dir() && under_roots(p)) {
            return Ok(last);
        }
        if let Some(session_cwd) = ctx.cwd.as_ref().filter(|p| under_roots(p)) {
            return Ok(session_cwd.clone());
        }
        Ok(self.default_cwd.clone())
    }
}

/// The resolved shell running a user hook command (stdio not configured).
pub(crate) fn hook_command(command: &str, cwd: &Path) -> std::process::Command {
    shell_command(command, cwd)
}

/// `std` command for the resolved shell (stdio not configured yet).
fn shell_command(command: &str, cwd: &Path) -> std::process::Command {
    match resolved_shell() {
        ResolvedShell::Bash(path) => bash_command(path, command, cwd),
        #[cfg(windows)]
        ResolvedShell::PowerShell => fallback_command(command, cwd),
        #[cfg(not(windows))]
        ResolvedShell::Sh => fallback_command(command, cwd),
    }
}

#[cfg(windows)]
fn fallback_command(command: &str, cwd: &Path) -> std::process::Command {
    // PowerShell when Git Bash is missing. `-ExecutionPolicy Bypass` is
    // intentional for desktop usability — not a security boundary. HITL
    // confirmation + ShellPolicy remain authoritative (see bash/policy.rs).
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        command,
    ])
    .current_dir(cwd);
    harden(&mut cmd);
    cmd
}

#[cfg(not(windows))]
fn fallback_command(command: &str, cwd: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    harden(&mut cmd);
    cmd
}

/// Foreground form: piped output, null stdin, killed when dropped.
fn piped(cmd: std::process::Command) -> Command {
    let mut cmd = Command::from(cmd);
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    cmd
}

fn harden(cmd: &mut std::process::Command) {
    apply_no_window(cmd);
    scrub_child_env(cmd);
}

/// Human-readable name of the shell the `bash` tool actually runs, for the
/// system prompt (the process `COMSPEC` is never used by this tool).
pub fn shell_label() -> String {
    match resolved_shell() {
        ResolvedShell::Bash(path) => format!("bash (bash -c via {})", path.display()),
        #[cfg(windows)]
        ResolvedShell::PowerShell => {
            "Windows PowerShell (no Git Bash found; commands run with powershell -Command)".into()
        }
        #[cfg(not(windows))]
        ResolvedShell::Sh => "sh (sh -c)".into(),
    }
}

/// Which launcher to use. Resolved once — never probe WSL `bash.exe` per call.
#[derive(Debug, Clone)]
enum ResolvedShell {
    Bash(PathBuf),
    #[cfg(windows)]
    PowerShell,
    #[cfg(not(windows))]
    Sh,
}

fn resolved_shell() -> &'static ResolvedShell {
    static SHELL: OnceLock<ResolvedShell> = OnceLock::new();
    SHELL.get_or_init(detect_shell)
}

fn detect_shell() -> ResolvedShell {
    if let Some(path) = find_real_bash() {
        return ResolvedShell::Bash(path);
    }
    #[cfg(windows)]
    {
        ResolvedShell::PowerShell
    }
    #[cfg(not(windows))]
    {
        ResolvedShell::Sh
    }
}

/// Prefer a real bash binary. On Windows, skip WSL / Store stubs so a voice
/// turn never boots a Linux VM just to run `echo`.
fn find_real_bash() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        for candidate in git_bash_candidates() {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("bash.exe");
            if candidate.is_file() && !is_wsl_or_store_bash(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("bash");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

#[cfg(windows)]
fn git_bash_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    const REL: &[&str] = &[
        // usr\bin is the real MSYS bash; Git\bin\bash.exe is a slower wrapper.
        r"Git\usr\bin\bash.exe",
        r"Git\bin\bash.exe",
    ];
    for root in [
        std::env::var_os("ProgramFiles"),
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("LOCALAPPDATA").map(|p| {
            let mut b = PathBuf::from(p);
            b.push("Programs");
            b.into_os_string()
        }),
    ]
    .into_iter()
    .flatten()
    {
        let root = PathBuf::from(root);
        for rel in REL {
            out.push(root.join(rel));
        }
    }
    out
}

/// WSL (`System32\bash.exe`) and the Microsoft Store stub. Hitting either
/// from a voice tool is a multi-second (or hanging) disaster.
pub(super) fn is_wsl_or_store_bash(path: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let lower = lower.replace('/', r"\");
    lower.contains(r"\windows\system32\bash.exe")
        || lower.contains(r"\windows\sysnative\bash.exe")
        || lower.contains(r"\windowsapps\")
        || lower.ends_with(r"\system32\bash.exe")
}

fn bash_command(bash: &Path, command: &str, cwd: &Path) -> std::process::Command {
    // `-c` not `-lc`: a login shell sources profile and was ~10× slower
    // (measured ~230ms vs ~24ms for Git Bash `echo` on Windows).
    let mut cmd = std::process::Command::new(bash);
    cmd.arg("-c").arg(command).current_dir(cwd);
    harden(&mut cmd);
    cmd
}

fn apply_no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// Drop common API / cloud secret env vars from the child process.
///
/// Does not clear PATH / system vars (would break normal tools). Best-effort
/// only — the model can still read secrets from disk if roots allow.
fn scrub_child_env(cmd: &mut std::process::Command) {
    const DROP: &[&str] = &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENROUTER_API_KEY",
        "BORIS_OPENROUTER_API_KEY",
        "BORIS_API_KEY",
        "XAI_API_KEY",
        "EXA_API_KEY",
        "BORIS_EXA_API_KEY",
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

/// Read a pipe in chunks, emitting progress; returns a **capped** buffer.
///
/// After [`CAPTURE_MAX_BYTES`] the buffer stops growing, but the pipe is still
/// drained so the child does not block on a full OS pipe. Progress after the
/// cap is drop-on-full / sparse so flood output cannot OOM the host.
async fn read_pipe_chunked(
    mut pipe: impl tokio::io::AsyncRead + Unpin,
    tx: mpsc::Sender<ProgressEvent>,
    is_stderr: bool,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut total: u64 = 0;
    let mut truncated = false;
    let mut sent_cap_notice = false;
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if buf.len() < CAPTURE_MAX_BYTES {
                    let room = CAPTURE_MAX_BYTES - buf.len();
                    let take = n.min(room);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }

                if !truncated {
                    let delta = String::from_utf8_lossy(&chunk[..n]).into_owned();
                    let _ = tx.try_send(ProgressEvent::Chunk {
                        delta: if is_stderr {
                            format!("[stderr] {delta}")
                        } else {
                            delta
                        },
                        total_bytes: total,
                        truncated: false,
                    });
                } else if !sent_cap_notice {
                    // One lightweight notice; further drain chunks skip progress
                    // allocations so malicious floods cannot OOM via the channel.
                    sent_cap_notice = true;
                    let _ = tx.try_send(ProgressEvent::Chunk {
                        delta: if is_stderr {
                            "[stderr] [capture capped]".into()
                        } else {
                            "[capture capped]".into()
                        },
                        total_bytes: total,
                        truncated: true,
                    });
                }
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
        match resolved_shell() {
            ResolvedShell::Bash(_) => BASH_DESCRIPTION,
            #[cfg(windows)]
            ResolvedShell::PowerShell => POWERSHELL_DESCRIPTION,
            #[cfg(not(windows))]
            ResolvedShell::Sh => BASH_DESCRIPTION,
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute. Real system commands only (git, cargo, npm, python, builds, tests). Never cat/grep/find/ls/echo."
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory under allowed roots (default: sandbox). Set this when the work is not in the sandbox."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default 120, max 300). Foreground commands are killed at the deadline. Must be an integer; floats are rejected."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Start the command as a background job and return its job id immediately (no timeout). Use for dev servers, watchers, and long builds; read output with bash_output, stop with bash_kill."
                }
            },
            "required": ["command"]
        })
    }

    fn arg_aliases(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("cmd", "command"),
            ("shell", "command"),
            ("timeout_secs", "timeout"),
        ]
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
        let command = optional_string_keys(obj, &["command", "cmd", "shell"])
            .ok_or_else(|| ToolError::invalid_args("missing required string argument `command`"))?;
        let command = command.trim();
        validate_command(command)?;
        if let Some(steer) = steer_simple_command(command) {
            return Ok(steer);
        }

        let cwd = self.pick_cwd(ctx, obj)?;

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

        // The next call in this session starts here unless it sets `cwd`.
        self.shell.remember_cwd(&cwd);

        if optional_bool(obj, "run_in_background").unwrap_or(false) {
            let shell = Arc::clone(&self.shell);
            let cmd = shell_command(command, &cwd);
            let (command, cwd, fallback) =
                (command.to_string(), cwd.clone(), self.default_cwd.clone());
            return tokio::task::spawn_blocking(move || {
                shell.spawn(cmd, &command, &cwd, &fallback)
            })
            .await
            .map_err(|e| ToolError::failed(format!("spawn task failed: {e}")))?;
        }

        let timeout_secs = parse_timeout_secs(obj)?;

        let start = Instant::now();
        let mut child = match Self::build_command(command, &cwd).spawn() {
            Ok(c) => c,
            Err(e) => {
                if matches!(resolved_shell(), ResolvedShell::Bash(_)) {
                    tracing::debug!(error = %e, "bash spawn failed; trying platform fallback");
                    Self::build_fallback(command, &cwd).spawn().map_err(|e2| {
                        ToolError::failed(format!("spawn failed: {e2} (bash: {e})"))
                    })?
                } else {
                    return Err(ToolError::failed(format!("spawn failed: {e}")));
                }
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
        // Bounded channel + try_send (drop-on-full) avoids progress-side OOM.
        let (prog_tx, mut prog_rx) = mpsc::channel::<ProgressEvent>(PROGRESS_CHANNEL_CAP);
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
        if text.trim().is_empty() {
            text = "(no output)\n".into();
        } else if !text.ends_with('\n') {
            text.push('\n');
        }

        let hint = if exit_code != 0 {
            " Command failed — read the error, change the command or cwd, and retry. Do not repeat the exact same failing call."
        } else {
            ""
        };

        let output = truncate_tool_result(format!(
            "Exit code: {exit_code}{hint}\n\
             cwd: {}\n\
             duration_ms: {duration_ms}\n\n\
             {text}",
            cwd.display()
        ));
        if exit_code == 0 {
            Ok(output)
        } else {
            Err(ToolError::failed(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn skips_wsl_and_store_bash() {
        assert!(is_wsl_or_store_bash(Path::new(
            r"C:\Windows\System32\bash.exe"
        )));
        assert!(is_wsl_or_store_bash(Path::new(
            r"C:\WINDOWS\system32\bash.exe"
        )));
        assert!(is_wsl_or_store_bash(Path::new(
            r"C:\Program Files\WindowsApps\CanonicalGroupLimited.Ubuntu_bash.exe"
        )));
        assert!(!is_wsl_or_store_bash(Path::new(
            r"C:\Program Files\Git\usr\bin\bash.exe"
        )));
        assert!(!is_wsl_or_store_bash(Path::new(
            r"C:\Program Files\Git\bin\bash.exe"
        )));
    }

    #[test]
    fn tool_name_stable() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(vec![dir.clone()], dir);
        assert_eq!(tool.name(), "bash");
        assert!(tool.meta().requires_confirmation);
    }

    #[test]
    fn schema_advertises_one_canonical_name_per_arg() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(vec![dir.clone()], dir);
        let schema = tool.parameters();
        let props = schema["properties"].as_object().unwrap();
        assert_eq!(props["timeout"]["type"], "integer");
        assert_eq!(schema["required"], json!(["command"]));
        for alias in ["cmd", "shell", "timeout_secs"] {
            assert!(!props.contains_key(alias), "alias {alias} advertised");
        }
        let text = schema.to_string();
        assert!(!text.contains("Alias"), "schema mentions aliases: {text}");
    }

    #[test]
    fn aliases_canonicalize_before_validation() {
        let dir = std::env::temp_dir();
        let tool = BashTool::new(vec![dir.clone()], dir);
        // Alias only: renamed to `command`, so `required` passes.
        let mut args = json!({ "cmd": "git status", "timeout_secs": 30 });
        crate::tool::canonicalize_arg_aliases(tool.arg_aliases(), &mut args);
        assert_eq!(args, json!({ "command": "git status", "timeout": 30 }));
        crate::tool::validate_args(&tool.parameters(), &args, "").expect("valid");
        // Both sent: the canonical value wins and the alias disappears.
        let mut args = json!({ "cmd": "ls", "command": "git log" });
        crate::tool::canonicalize_arg_aliases(tool.arg_aliases(), &mut args);
        assert_eq!(args, json!({ "command": "git log" }));
    }

    #[tokio::test]
    async fn pwd_smoke_works() {
        let dir = std::env::temp_dir().join(format!("boris-bash-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": "pwd" }),
            )
            .await
            .expect("run");
        assert!(
            out.contains("Exit code: 0") && !out.contains("Command was not run."),
            "got: {out}"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn nonzero_exit_preserves_failure_diagnostics() {
        let dir = std::env::temp_dir().join(format!("boris-bash-failed-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let err = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("failed-command"),
                json!({ "command": "pwd; exit 7" }),
            )
            .await
            .expect_err("a nonzero exit must be a failed tool operation");

        assert_eq!(err.kind(), crate::tool::ToolErrorKind::Failed);
        assert!(err.message.contains("Exit code: 7"), "got: {}", err.message);
        assert!(err.message.contains("cwd:"), "got: {}", err.message);
        assert!(
            err.message.contains("boris-bash-failed-"),
            "got: {}",
            err.message
        );
        assert!(
            err.message.contains("duration_ms:") && !err.message.contains("(no output)"),
            "got: {}",
            err.message
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn cmd_alias_and_float_timeout_rejected() {
        let dir = std::env::temp_dir().join(format!("boris-bash-alias-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        // `cmd` alias reaches the same path as `command`.
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "cmd": "pwd" }),
            )
            .await
            .expect("cmd alias");
        assert!(out.contains("Exit code: 0"), "got: {out}");
        // `shell` alias also works.
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "shell": "pwd" }),
            )
            .await
            .expect("shell alias");
        assert!(out.contains("Exit code: 0"), "got: {out}");
        // Float timeout is invalid_args, not a silent default.
        let err = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": "pwd", "timeout": 12.5 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), crate::tool::ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("integer"), "got: {}", err.message);
        // Numeric string timeout is accepted.
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": "pwd", "timeout": "30" }),
            )
            .await
            .expect("string timeout");
        assert!(out.contains("Exit code: 0"), "got: {out}");
        // Missing command (all aliases absent) is invalid_args.
        let err = tool
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), crate::tool::ToolErrorKind::InvalidArgs);
        assert!(err.message.contains("command"), "got: {}", err.message);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn cwd_persists_across_calls_in_a_session() {
        let dir = std::env::temp_dir().join(format!("boris-bash-cwd-{}", std::process::id()));
        let sub = dir.join("subproj");
        tokio::fs::create_dir_all(&sub).await.unwrap();
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let ctx = crate::tool_context::ToolCallContext::new("t");
        let first = tool
            .execute(
                &ctx,
                json!({ "command": "pwd", "cwd": sub.display().to_string() }),
            )
            .await
            .expect("explicit cwd");
        assert!(first.contains("subproj"), "got: {first}");
        // No cwd: the last one is reused, not the sandbox default.
        let second = tool
            .execute(&ctx, json!({ "command": "pwd" }))
            .await
            .expect("reused cwd");
        let cwd_line = second.lines().find(|l| l.starts_with("cwd:")).unwrap();
        assert!(cwd_line.contains("subproj"), "got: {second}");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn run_in_background_returns_a_job_and_kill_stops_it() {
        let dir = std::env::temp_dir().join(format!("boris-bash-bg-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let shell = ShellSession::new();
        let tool = BashTool::with_session(vec![dir.clone()], dir.clone(), Arc::clone(&shell));
        let ctx = crate::tool_context::ToolCallContext::new("t");
        // `sleep` exists in Git Bash and as a PowerShell alias.
        let started = tokio::time::timeout(
            Duration::from_secs(10),
            tool.execute(
                &ctx,
                json!({ "command": "sleep 30", "run_in_background": true }),
            ),
        )
        .await
        .expect("returns at once")
        .expect("started");
        assert!(
            started.contains("job-1") && started.contains("bash_output"),
            "got: {started}"
        );
        assert_eq!(shell.running_jobs(), 1);
        let killed = super::super::BashKillTool::new(Arc::clone(&shell))
            .execute(&ctx, json!({ "job_id": "job-1" }))
            .await
            .expect("kill");
        assert!(killed.contains("Killed job job-1"), "got: {killed}");
        assert_eq!(shell.running_jobs(), 0);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn echo_is_steered_to_speech() {
        let dir = std::env::temp_dir().join(format!("boris-bash-steer-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());
        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": "echo hello-from-bash" }),
            )
            .await
            .expect("steer");
        assert!(out.contains("Command was not run."), "got: {out}");
        assert!(
            out.contains("spoken") || out.contains("present_artifact"),
            "got: {out}"
        );
        assert!(!out.contains("Exit code:"), "got: {out}");
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn cancel_kills_long_command() {
        use tokio_util::sync::CancellationToken;

        let dir = std::env::temp_dir().join(format!("boris-bash-cancel-{}", std::process::id()));
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

    /// High-volume pipe must not grow the capture buffer past CAPTURE_MAX_BYTES.
    #[tokio::test]
    async fn read_pipe_chunked_caps_buffer() {
        // ~3× the hard cap so we prove drain-without-grow.
        let flood_len = CAPTURE_MAX_BYTES * 3;
        let flood = vec![b'x'; flood_len];
        let (tx, mut rx) = mpsc::channel::<ProgressEvent>(PROGRESS_CHANNEL_CAP);

        let captured = read_pipe_chunked(flood.as_slice(), tx, false).await;
        assert_eq!(
            captured.len(),
            CAPTURE_MAX_BYTES,
            "buffer must stop at capture cap"
        );
        assert!(captured.iter().all(|&b| b == b'x'));

        // Progress must report truncated at least once; channel is bounded so
        // we only assert we got a capped notice rather than every byte.
        let mut saw_truncated = false;
        while let Ok(ev) = rx.try_recv() {
            if let ProgressEvent::Chunk { truncated, .. } = ev {
                if truncated {
                    saw_truncated = true;
                }
            }
        }
        assert!(saw_truncated, "expected a truncated progress event");
    }

    /// End-to-end: flooding stdout still returns a finite truncated tool result.
    #[tokio::test]
    async fn high_volume_output_is_truncated_not_unbounded() {
        let dir = std::env::temp_dir().join(format!("boris-bash-flood-{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tool = BashTool::new(vec![dir.clone()], dir.clone());

        // ~1MB of 'a' — well above CAPTURE_MAX_BYTES (120KB) and MAX_BYTES (30KB).
        #[cfg(windows)]
        let cmd = "python -c \"print('a' * 1000000)\"";
        #[cfg(not(windows))]
        let cmd = "python3 -c \"print('a' * 1000000)\" || python -c \"print('a' * 1000000)\" || yes a | head -c 1000000";

        let out = tool
            .execute(
                &crate::tool_context::ToolCallContext::new("t"),
                json!({ "command": cmd, "timeout": 30 }),
            )
            .await;

        // Skip if python/yes unavailable on this host — unit test above covers the cap.
        let Ok(out) = out else {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return;
        };
        // truncate_tool_result + truncate_output keep the result modest.
        assert!(
            out.len() < CAPTURE_MAX_BYTES * 2,
            "result too large: {} bytes",
            out.len()
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
