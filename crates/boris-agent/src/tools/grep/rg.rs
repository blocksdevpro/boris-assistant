//! Ripgrep (`rg`) invocation for the grep tool.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tool::{truncate_tool_result, ToolError};

/// Cached "is `rg` on PATH?" probe. Never spawns — just looks for the binary.
pub(super) fn rg_available() -> bool {
    cached_rg().is_some()
}

fn cached_rg() -> Option<&'static Path> {
    static RG: OnceLock<Option<PathBuf>> = OnceLock::new();
    RG.get_or_init(find_rg).as_deref()
}

fn find_rg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(rg_exe_name());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn rg_exe_name() -> &'static str {
    if cfg!(windows) {
        "rg.exe"
    } else {
        "rg"
    }
}

fn apply_no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// Run `rg` and return a truncated match listing, or an error to trigger fallback.
pub(super) async fn run_rg(
    pattern: &str,
    search_path: &Path,
    ignore_case: bool,
    glob: Option<&str>,
    limit: usize,
) -> Result<String, ToolError> {
    let exe = cached_rg().ok_or_else(|| ToolError::failed("rg not on PATH"))?;

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

    let mut cmd = Command::new(exe);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    apply_no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::failed(format!("rg spawn: {e}")))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::failed("no stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::failed("no stderr"))?;

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
