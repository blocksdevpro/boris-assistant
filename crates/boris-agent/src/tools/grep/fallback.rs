//! Pure-Rust content search when ripgrep is unavailable.

use crate::tool::ToolError;
use crate::tools::path_pattern::{is_common_skip_dir, simple_name_glob};

/// Walk `search_path` and collect `path:line:content` hits (substring, not full regex).
pub(super) fn rust_grep(
    pattern: &str,
    search_path: &std::path::Path,
    ignore_case: bool,
    glob: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, ToolError> {
    let pat_lower = if ignore_case {
        pattern.to_ascii_lowercase()
    } else {
        pattern.to_string()
    };

    let mut out = Vec::new();
    if search_path.is_file() {
        grep_file(search_path, &pat_lower, ignore_case, limit, &mut out);
        return Ok(out);
    }

    let mut stack = vec![search_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    // Preserve historical grep skip set (no `.boris`).
                    if is_common_skip_dir(name) {
                        continue;
                    }
                }
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            if let Some(g) = glob {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let rel = path
                    .strip_prefix(search_path)
                    .unwrap_or(&path)
                    .to_string_lossy();
                if !simple_name_glob(g, name) && !simple_name_glob(g, &rel) {
                    continue;
                }
            }
            grep_file(&path, &pat_lower, ignore_case, limit - out.len(), &mut out);
        }
    }
    Ok(out)
}

fn grep_file(
    path: &std::path::Path,
    pattern: &str,
    ignore_case: bool,
    remaining: usize,
    out: &mut Vec<String>,
) {
    if remaining == 0 {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if bytes.iter().take(256).any(|&b| b == 0) {
        return;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return;
    };
    for (i, line) in text.lines().enumerate() {
        if out.len() >= remaining {
            break;
        }
        let hay = if ignore_case {
            line.to_ascii_lowercase()
        } else {
            line.to_string()
        };
        // Substring search (not full regex) for fallback.
        if hay.contains(pattern) {
            out.push(format!("{}:{}:{}", path.display(), i + 1, line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn finds_line_in_file() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-grep-fb-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello\nFINDME please\nbye\n").unwrap();
        let hits = rust_grep("findme", &dir, true, None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].contains("FINDME"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_filter_restricts_files() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-grep-glob-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "needle here\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle here\n").unwrap();
        let hits = rust_grep("needle", &dir, false, Some("*.rs"), 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].contains("a.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_binary_files() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-grep-bin-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        let mut bytes = b"needle".to_vec();
        bytes.push(0);
        bytes.extend_from_slice(b"more");
        std::fs::write(dir.join("x.bin"), bytes).unwrap();
        let hits = rust_grep("needle", &dir, false, None, 10).unwrap();
        assert!(hits.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn respects_limit() {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-grep-lim-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x\nx\nx\nx\n").unwrap();
        let hits = rust_grep("x", &dir, false, None, 2).unwrap();
        assert_eq!(hits.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
