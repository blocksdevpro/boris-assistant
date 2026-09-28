//! Shared path resolution for file / open / glob / grep tools.
//!
//! All FS tools resolve model-supplied paths through [`resolve_under_roots`] so
//! relative paths join an allowed root and absolute paths must sit under one.
//!
//! After lexical resolve we re-check the **canonical** path when the filesystem
//! allows it, so a symlink under an allowed root cannot escape.
//! **TOCTOU residual**: the path may change between this check and later I/O.

use std::path::{Path, PathBuf};

use crate::runtime::policy::{normalize_path, path_is_within, resolve_path_for_policy};
use crate::tool::ToolError;

/// Resolve `raw` to a normalized absolute path that sits under one of `roots`.
///
/// # Rules
/// - Empty / NUL-containing paths are rejected as invalid args.
/// - Absolute paths must already fall under some root (lexical + best-effort real path).
/// - Relative paths are joined to each root in order; first in-bounds win.
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

    if normalized.is_absolute() {
        return accept_if_within_roots(normalized, roots);
    }

    // Relative: try under each root.
    for root in roots {
        let joined = root.join(&normalized);
        let joined_n = normalize_path(&joined).map_err(ToolError::invalid_args)?;
        if let Ok(accepted) = accept_if_within_roots(joined_n, roots) {
            return Ok(accepted);
        }
    }

    Err(ToolError::failed(format!(
        "path `{raw}` is outside allowed roots"
    )))
}

fn accept_if_within_roots(normalized: PathBuf, roots: &[PathBuf]) -> Result<PathBuf, ToolError> {
    // Prefer real path (symlink target) for the containment check.
    let real = resolve_path_for_policy(&normalized).unwrap_or_else(|_| normalized.clone());

    for root in roots {
        let root_n = resolve_path_for_policy(root)
            .or_else(|_| normalize_path(root))
            .unwrap_or_else(|_| root.clone());
        if path_is_within(&real, &root_n) || path_is_within(&normalized, &root_n) {
            // Return real path when it stayed in-bounds so tools open the true target.
            if path_is_within(&real, &root_n) {
                return Ok(real);
            }
            return Ok(normalized);
        }
    }
    Err(ToolError::failed(format!(
        "path `{}` is outside allowed roots",
        real.display()
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

/// Case-insensitive Levenshtein distance (chars, not bytes).
///
/// Small dependency-free helper for voice-STT mishear recovery
/// (`report final` vs `report_final`). O(n*m) but inputs are file names.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_ascii_lowercase().chars().collect();
    let b: Vec<char> = b.to_ascii_lowercase().chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(cur[j] + 1).min(prev[j + 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Rank `candidates` by similarity to `target` (best first, max `max_n`).
///
/// Scoring: exact-substring containment wins (STT often drops separators),
/// then normalized Levenshtein. Filters to plausible matches only so the
/// model doesn't get noise.
pub fn suggest_names(target: &str, candidates: &[String], max_n: usize) -> Vec<String> {
    let target = target.trim().to_ascii_lowercase();
    if target.is_empty() || candidates.is_empty() || max_n == 0 {
        return Vec::new();
    }
    // Normalize separators: voice often turns `my file` / `my-file` / `my_file`.
    let norm_target: String = target
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    let mut scored: Vec<(i64, String)> = Vec::new();
    for c in candidates {
        let lower = c.to_ascii_lowercase();
        let norm_c: String = lower
            .chars()
            .map(|ch| if ch == '_' || ch == '-' { ' ' } else { ch })
            .collect();
        let score = if lower == target {
            10_000
        } else if norm_c == norm_target {
            9_000
        } else if lower.contains(&target) || target.contains(&lower) {
            5_000 - (lower.len() as i64 - target.len() as i64).abs()
        } else {
            let dist = levenshtein(&norm_target, &norm_c);
            let max_len = norm_target.chars().count().max(norm_c.chars().count()) as i64;
            if max_len == 0 {
                continue;
            }
            // Only keep reasonably close matches (<= ~40% edits).
            if dist as i64 * 5 > max_len * 2 {
                continue;
            }
            1_000 - (dist as i64 * 100 / max_len.max(1))
        };
        scored.push((score, c.clone()));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(max_n).map(|(_, s)| s).collect()
}

/// List sibling file names for a missing path (best-effort, sync).
///
/// Reads the parent directory if it exists, else falls back to nothing.
/// Never walks recursively — cheap enough for the not-found error path.
pub fn sibling_names(missing: &Path) -> Vec<String> {
    let parent = missing.parent().filter(|p| !p.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        out.push(entry.file_name().to_string_lossy().into_owned());
    }
    out
}

/// Build a `did you mean` suffix for not-found errors (empty when nothing close).
pub fn did_you_mean_suffix(missing: &Path, max_n: usize) -> String {
    let target = missing
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.is_empty() {
        return String::new();
    }
    let suggestions = suggest_names(&target, &sibling_names(missing), max_n);
    if suggestions.is_empty() {
        return String::new();
    }
    format!("\nDid you mean: {}?", suggestions.join(", "))
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

    #[test]
    fn rejects_empty_path() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let err = resolve_under_roots("   ", &roots).unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn rejects_nul_in_path() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let err = resolve_under_roots("foo\0bar", &roots).unwrap_err();
        assert!(err.message.contains("NUL"));
    }

    #[test]
    fn read_roots_includes_all() {
        let sandbox = PathBuf::from("/s");
        let data = vec![PathBuf::from("/d")];
        let allow_read = vec![PathBuf::from("/r")];
        let allow_write = vec![PathBuf::from("/w")];
        let roots = read_roots(&sandbox, &data, &allow_read, &allow_write);
        assert_eq!(roots.len(), 4);
        assert!(roots.contains(&sandbox));
        assert!(roots.contains(&PathBuf::from("/d")));
        assert!(roots.contains(&PathBuf::from("/r")));
        assert!(roots.contains(&PathBuf::from("/w")));
    }

    #[test]
    fn write_roots_excludes_allow_read() {
        let sandbox = PathBuf::from("/s");
        let data = vec![PathBuf::from("/d")];
        let allow_write = vec![PathBuf::from("/w")];
        let roots = write_roots(&sandbox, &data, &allow_write);
        assert_eq!(roots.len(), 3);
        assert!(!roots.iter().any(|p| p == Path::new("/r")));
    }

    #[test]
    #[cfg(windows)]
    fn case_insensitive_accept_on_windows() {
        let roots = vec![PathBuf::from("C:\\Users\\me\\.boris\\sandbox")];
        let p = resolve_under_roots("c:\\users\\me\\.boris\\sandbox\\Note.TXT", &roots);
        assert!(p.is_ok());
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        // Case-insensitive.
        assert_eq!(levenshtein("Report", "report"), 0);
    }

    #[test]
    fn suggest_names_prefers_containment_and_separators() {
        let cands = vec![
            "report_final.md".to_string(),
            "report-draft.md".to_string(),
            "unrelated.txt".to_string(),
        ];
        // STT separator confusion: space vs underscore.
        let out = suggest_names("report final.md", &cands, 3);
        assert_eq!(out[0], "report_final.md");
        // Substring still ranks.
        let out = suggest_names("report", &cands, 2);
        assert!(out.iter().any(|s| s.contains("report")));
        // Far strings filtered.
        let out = suggest_names("zzzqqq", &cands, 3);
        assert!(out.is_empty());
    }

    #[test]
    fn did_you_mean_lists_siblings() {
        let dir = std::env::temp_dir().join(format!("boris-suggest-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("grocery-list.md"), "x").unwrap();
        std::fs::write(dir.join("notes.md"), "y").unwrap();
        let missing = dir.join("grocery list.md");
        let suffix = did_you_mean_suffix(&missing, 3);
        assert!(suffix.contains("grocery-list.md"), "got: {suffix}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
