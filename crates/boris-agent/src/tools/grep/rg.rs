//! Ripgrep (`rg`) invocation for the grep tool.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tool::{truncate_tool_result, ToolError};

/// Run `rg` and return a truncated match listing, or an error to trigger fallback.
pub(super) async fn run_rg(
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
