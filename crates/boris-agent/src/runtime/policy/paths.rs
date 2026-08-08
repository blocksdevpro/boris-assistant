//! Path normalization and sandbox root checks for tool policy.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::SandboxConfig;

/// Common Windows/user document folders for read-only file tools.
pub fn default_user_read_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    if let Some(home) = user {
        for name in ["Desktop", "Documents", "Downloads"] {
            let p = home.join(name);
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    roots
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PathAccess {
    Read,
    Write,
}

pub(super) fn args_path_string(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    for key in [
        "path",
        "file",
        "filepath",
        "file_path",
        "dir",
        "directory",
        "cwd",
    ] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

pub(super) fn check_path_allowed(
    config: &SandboxConfig,
    raw: &str,
    access: PathAccess,
) -> Result<(), String> {
    let resolved = normalize_path(Path::new(raw))?;
    let roots: Vec<PathBuf> = match access {
        PathAccess::Read => {
            let mut r = Vec::new();
            r.push(config.sandbox_root.clone());
            r.extend(config.boris_data_roots.iter().cloned());
            r.extend(config.allow_read.iter().cloned());
            r.extend(config.allow_write.iter().cloned());
            r
        }
        PathAccess::Write => {
            let mut r = Vec::new();
            r.push(config.sandbox_root.clone());
            r.extend(config.boris_data_roots.iter().cloned());
            r.extend(config.allow_write.iter().cloned());
            r
        }
    };

    for root in &roots {
        let root_n = normalize_path(root).unwrap_or_else(|_| root.clone());
        if path_is_within(&resolved, &root_n) {
            return Ok(());
        }
    }

    Err(format!(
        "path `{}` is outside allowed roots",
        resolved.display()
    ))
}

/// Normalize a path without requiring it to exist (no symlink resolve).
///
/// Rejects empty paths and keeps `..` from escaping via component folding.
pub fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }

    let mut out = PathBuf::new();
    // Preserve absolute prefix (drive / root).
    for (i, comp) in path.components().enumerate() {
        match comp {
            Component::Prefix(p) => {
                if i == 0 {
                    out.push(p.as_os_str());
                }
            }
            Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err("path escapes with `..`".into());
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("path resolved empty".into());
    }
    Ok(out)
}

/// True if `path` is equal to `root` or a descendant (component-wise).
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    let path_c: Vec<_> = path.components().collect();
    let root_c: Vec<_> = root.components().collect();
    if root_c.is_empty() {
        return false;
    }
    if path_c.len() < root_c.len() {
        return false;
    }
    path_c
        .iter()
        .zip(root_c.iter())
        .all(|(a, b)| a.as_os_str() == b.as_os_str())
}

/// Public helper for future file tools: resolve `raw` under write/read roots.
pub fn resolve_in_roots(
    config: &SandboxConfig,
    raw: &str,
    write: bool,
) -> Result<PathBuf, String> {
    let access = if write {
        PathAccess::Write
    } else {
        PathAccess::Read
    };
    check_path_allowed(config, raw, access)?;
    normalize_path(Path::new(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_escape_rejected() {
        let err = normalize_path(Path::new("C:\\Users\\me\\.boris\\sandbox\\..\\..\\Windows"));
        // Folded path may still normalize; path_is_within should fail.
        let n = err.expect("normalize folds ..");
        let root = PathBuf::from("C:\\Users\\me\\.boris\\sandbox");
        assert!(!path_is_within(&n, &root));
    }
}
