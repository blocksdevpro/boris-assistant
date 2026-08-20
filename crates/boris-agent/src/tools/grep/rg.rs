//! Ripgrep (`rg`) invocation for the grep tool.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tool::ToolError;

use super::format::GrepHits;
use super::query::{GrepQuery, OutputMode};
use super::{looks_like_regex, MAX_LINE_CHARS};

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

/// Run `rg` and return structured hits, or an error to trigger fallback / surface.
pub(super) async fn run_rg(query: &GrepQuery, search_path: &Path) -> Result<GrepHits, ToolError> {
    let exe = cached_rg().ok_or_else(|| ToolError::failed("rg not on PATH"))?;

    let mut args: Vec<String> = vec![
        "--line-number".into(),
        "--color=never".into(),
        "--no-heading".into(),
        "--with-filename".into(),
        "--max-columns".into(),
        MAX_LINE_CHARS.to_string(),
        "--max-columns-preview".into(),
        "--max-filesize".into(),
        "2M".into(),
    ];
    if query.ignore_case {
        args.push("-i".into());
    }
    if let Some(g) = query.glob.as_deref() {
        args.push("--glob".into());
        args.push(g.to_string());
    }
    if let Some(t) = query.file_type.as_deref() {
        args.push("--type".into());
        args.push(t.to_string());
    }
    if query.multiline {
        args.push("-U".into());
        args.push("--multiline-dotall".into());
    }
    if query.before > 0 {
        args.push(format!("-B{}", query.before));
    }
    if query.after > 0 {
        args.push(format!("-A{}", query.after));
    }
    match query.output_mode {
        OutputMode::FilesWithMatches => args.push("-l".into()),
        OutputMode::Count => args.push("-c".into()),
        OutputMode::Content => {}
    }
    // Literals stay literals — `foo.rs` must not become `foo<any>rs`.
    if !query.multiline && !looks_like_regex(&query.pattern) {
        args.push("-F".into());
    }
    args.push("-e".into());
    args.push(query.pattern.clone());
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

    let status = tokio::time::timeout(Duration::from_secs(20), child.wait())
        .await
        .map_err(|_| {
            let _ = child.start_kill();
            ToolError::timeout(
                "grep timed out after 20s. Narrow the path, glob, or pattern and retry.",
            )
        })?
        .map_err(|e| ToolError::failed(format!("rg wait: {e}")))?;

    let out = stdout_task.await.unwrap_or_default();
    let err = stderr_task.await.unwrap_or_default();
    let code = status.code().unwrap_or(-1);
    // rg exit 1 = no matches; 2 = error
    if code == 2 {
        let e = String::from_utf8_lossy(&err);
        return Err(ToolError::failed(format!(
            "rg error: {}. Check the regex (escape metacharacters), glob, type, or path.",
            e.trim()
        )));
    }
    if code != 0 && code != 1 {
        let e = String::from_utf8_lossy(&err);
        return Err(ToolError::failed(format!(
            "rg failed (exit {code}): {}",
            e.trim()
        )));
    }

    let text = String::from_utf8_lossy(&out);
    let mut lines: Vec<String> = Vec::new();
    let mut truncated = false;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        // Count mode reports files with 0 matches too; drop them.
        if query.output_mode == OutputMode::Count && line.ends_with(":0") {
            continue;
        }
        if lines.len() >= query.limit {
            truncated = true;
            break;
        }
        lines.push(line.to_string());
    }

    let (match_count, file_count) = summarize(&lines, query.output_mode);
    Ok(GrepHits {
        lines,
        match_count,
        file_count,
        truncated,
    })
}

fn summarize(lines: &[String], mode: OutputMode) -> (usize, usize) {
    match mode {
        OutputMode::FilesWithMatches => (lines.len(), lines.len()),
        OutputMode::Count => {
            let mut matches = 0usize;
            for line in lines {
                if let Some(n) = line
                    .rsplit(':')
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    matches += n;
                }
            }
            (matches, lines.len())
        }
        OutputMode::Content => {
            let matches = lines.iter().filter(|l| !is_context_line(l)).count();
            (matches, super::format::count_files(lines))
        }
    }
}

fn is_context_line(line: &str) -> bool {
    // `path:12-context` — a digit followed by `-` (not `:`) marks context.
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate().rev() {
        if *b == b'-' && i > 0 && bytes[i - 1].is_ascii_digit() {
            return true;
        }
        if *b == b':' && i > 0 && bytes[i - 1].is_ascii_digit() {
            return false;
        }
    }
    false
}
