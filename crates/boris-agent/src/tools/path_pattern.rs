//! Minimal glob / name-pattern matchers shared by `glob` and `grep` tools.
//!
//! No gitignore, no external crates — pure Rust matching for `*`, `?`, and `**`.

/// Match a path relative to a search root against a simple glob (`*`, `**`, `?`).
///
/// Backslashes are normalized to `/`. Leading `./` on the path is stripped.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    glob_match_inner(pattern.as_str(), path)
}

fn glob_match_inner(pattern: &str, path: &str) -> bool {
    // Handle ** specially
    if let Some(rest) = pattern.strip_prefix("**/") {
        // Match rest at any depth
        if glob_match_inner(rest, path) {
            return true;
        }
        // Consume one path segment and retry
        if let Some((_, tail)) = path.split_once('/') {
            return glob_match_inner(pattern, tail);
        }
        return glob_match_inner(rest, path);
    }
    if pattern == "**" {
        return true;
    }

    let (pat_seg, pat_rest) = match pattern.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (pattern, None),
    };
    let (path_seg, path_rest) = match path.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    };

    if !seg_match(pat_seg, path_seg) {
        return false;
    }
    match (pat_rest, path_rest) {
        (None, None) => true,
        (Some(pr), Some(phr)) => glob_match_inner(pr, phr),
        (None, Some(_)) => false,
        (Some(pr), None) => pr.is_empty() || pr == "**" || pr.starts_with("**/"),
    }
}

/// Single path-segment match with `*` and `?` (no `/`).
pub(crate) fn seg_match(pat: &str, seg: &str) -> bool {
    if pat == "*" {
        return true;
    }
    let pb: Vec<char> = pat.chars().collect();
    let sb: Vec<char> = seg.chars().collect();
    let mut pi = 0usize;
    let mut si = 0usize;
    let mut star = None::<(usize, usize)>;
    while si < sb.len() {
        if pi < pb.len() && (pb[pi] == sb[si] || pb[pi] == '?') {
            pi += 1;
            si += 1;
        } else if pi < pb.len() && pb[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, si));
        } else {
            return false;
        }
    }
    while pi < pb.len() && pb[pi] == '*' {
        pi += 1;
    }
    pi == pb.len()
}

/// Grep-style file filter: `*.rs` or basename-only patterns (no multi-segment paths).
///
/// Strips a leading `**/` so `--glob '**/*.rs'` still filters basenames.
pub(crate) fn simple_name_glob(pat: &str, name: &str) -> bool {
    let pat = pat.trim_start_matches("**/");
    if pat.contains('/') {
        return false;
    }
    seg_match(pat, name)
}

/// Directory names skipped while walking for glob/grep (best-effort noise filter).
///
/// Note: glob also skips `.boris`; grep historically did not — callers choose.
pub(crate) fn is_common_skip_dir(name: &str) -> bool {
    matches!(name, "node_modules" | ".git" | "target" | "__pycache__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basic() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match("src/**/*.rs", "src/a/b.rs"));
        assert!(!glob_match("src/**/*.rs", "lib/a.rs"));
    }

    #[test]
    fn glob_normalizes_slashes() {
        assert!(glob_match("src\\*.rs", "src/main.rs"));
        assert!(glob_match("**\\*.txt", "a/b.txt"));
    }

    #[test]
    fn glob_strips_dot_slash() {
        assert!(glob_match("*.rs", "./main.rs"));
    }

    #[test]
    fn seg_match_star_and_question() {
        assert!(seg_match("*.rs", "main.rs"));
        assert!(seg_match("a?c", "abc"));
        assert!(!seg_match("a?c", "ac"));
        assert!(seg_match("*", "anything"));
        assert!(seg_match("pre*", "prefix"));
        assert!(!seg_match("pre*", "xprefix"));
    }

    #[test]
    fn simple_name_glob_basename_only() {
        assert!(simple_name_glob("*.rs", "main.rs"));
        assert!(simple_name_glob("**/*.rs", "main.rs"));
        assert!(!simple_name_glob("src/*.rs", "main.rs")); // multi-segment → false
        assert!(!simple_name_glob("*.rs", "main.txt"));
    }

    #[test]
    fn common_skip_dirs() {
        assert!(is_common_skip_dir("node_modules"));
        assert!(is_common_skip_dir("target"));
        assert!(!is_common_skip_dir("src"));
        assert!(!is_common_skip_dir(".boris")); // caller-specific
    }
}
