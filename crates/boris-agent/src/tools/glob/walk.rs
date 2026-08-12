//! Directory walk for the glob tool.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::tools::path_pattern::{glob_match, is_common_skip_dir};

/// Recursively collect files under `root` whose relative path matches `pattern`.
pub(super) fn walk_collect(root: &Path, pattern: &str, out: &mut Vec<(PathBuf, SystemTime)>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                // Skip common junk (+ `.boris` for glob walks only).
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if is_common_skip_dir(name) || name == ".boris" {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_s = rel.to_string_lossy();
            if glob_match(pattern, &rel_s) {
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((path, mtime));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn walk_collect_matches_nested() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-glob-walk-{n}"));
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        std::fs::write(dir.join("a").join("b").join("x.txt"), "x").unwrap();
        std::fs::write(dir.join("a").join("y.md"), "y").unwrap();
        // Skipped dir should not be entered
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target").join("z.txt"), "z").unwrap();

        let mut found = Vec::new();
        walk_collect(&dir, "**/*.txt", &mut found);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("x.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
