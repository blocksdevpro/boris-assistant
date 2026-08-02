//! Shared path resolution for file / open tools.

use std::path::{Path, PathBuf};

use crate::runtime::policy::{normalize_path, path_is_within};
use crate::tool::ToolError;

/// Resolve `raw` to an absolute-ish normalized path that sits under one of `roots`.
pub fn resolve_under_roots(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, ToolError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ToolError::invalid_args("path is empty"));
    }
    if raw.contains('\0') {
        return Err(ToolError::invalid_args("path contains NUL"));
    }

    let candidate = PathBuf::from(raw);
    let normalized = normalize_path(&candidate).map_err(ToolError::invalid_args)?;

    // Also try absolute join if relative: relative paths are resolved against each root.
    if normalized.is_absolute() {
        for root in roots {
            let root_n = normalize_path(root).unwrap_or_else(|_| root.clone());
            if path_is_within(&normalized, &root_n) {
                return Ok(normalized);
            }
        }
        return Err(ToolError::failed(format!(
            "path `{}` is outside allowed roots",
            normalized.display()
        )));
    }

    // Relative: try under each root.
    for root in roots {
        let joined = root.join(&normalized);
        let joined_n = normalize_path(&joined).map_err(ToolError::invalid_args)?;
        let root_n = normalize_path(root).unwrap_or_else(|_| root.clone());
        if path_is_within(&joined_n, &root_n) {
            return Ok(joined_n);
        }
    }

    Err(ToolError::failed(format!(
        "path `{raw}` is outside allowed roots"
    )))
}

/// All roots a tool may read from (sandbox + data + allow_read + allow_write).
pub fn read_roots(
    sandbox: &Path,
    data: &[PathBuf],
    allow_read: &[PathBuf],
    allow_write: &[PathBuf],
) -> Vec<PathBuf> {
    let mut r = Vec::with_capacity(1 + data.len() + allow_read.len() + allow_write.len());
    r.push(sandbox.to_path_buf());
    r.extend(data.iter().cloned());
    r.extend(allow_read.iter().cloned());
    r.extend(allow_write.iter().cloned());
    r
}

/// Roots a tool may write to (sandbox + data + allow_write).
pub fn write_roots(sandbox: &Path, data: &[PathBuf], allow_write: &[PathBuf]) -> Vec<PathBuf> {
    let mut r = Vec::with_capacity(1 + data.len() + allow_write.len());
    r.push(sandbox.to_path_buf());
    r.extend(data.iter().cloned());
    r.extend(allow_write.iter().cloned());
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_escape() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let err = resolve_under_roots("C:\\Windows\\System32", &roots).unwrap_err();
        assert!(err.message.contains("outside") || err.message.contains("path"));
    }

    #[test]
    fn accepts_under_sandbox() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let p = resolve_under_roots("C:\\Users\\me\\.boris\\sandbox\\note.txt", &roots).unwrap();
        assert!(p.to_string_lossy().contains("note.txt"));
    }

    #[test]
    fn relative_joins_root() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let p = resolve_under_roots("hello.txt", &roots).unwrap();
        assert!(p.ends_with("hello.txt"));
    }
}
