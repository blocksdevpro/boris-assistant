//! Pure-ish keyword scoring and snippet extraction for markdown memory files.

use std::fs;
use std::path::Path;

use super::MemoryHit;

/// Score one markdown file against a lowercased query; push a hit when matched.
pub(super) fn score_file(
    path: &Path,
    root: &Path,
    query_lc: &str,
    hits: &mut Vec<MemoryHit>,
) -> Result<(), String> {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let boost_session = rel.contains("sessions/") || rel.ends_with("/memory.md");
    let boost_curated = rel.ends_with("MEMORY.md");
    score_file_as(path, rel, query_lc, hits, boost_session, boost_curated)
}

/// Score with an explicit display path (used for session logs outside the memory root).
pub(super) fn score_file_as(
    path: &Path,
    display_path: String,
    query_lc: &str,
    hits: &mut Vec<MemoryHit>,
    boost_session: bool,
    boost_curated: bool,
) -> Result<(), String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let score = score_document(&raw, query_lc);
    if score == 0 {
        return Ok(());
    }
    let mut score = score;
    if boost_session {
        score = score.saturating_add(15);
    }
    if boost_curated {
        score = score.saturating_add(25); // curated knowledge wins ties
    }
    let snippet = best_snippet(&raw, query_lc, 220);
    hits.push(MemoryHit {
        path: display_path.replace('\\', "/"),
        score,
        snippet,
    });
    Ok(())
}

/// BM25-ish term frequency score for a document body (no path boosts).
///
/// Returns 0 when no query tokens of length ≥ 2 match.
pub(super) fn score_document(raw: &str, query_lc: &str) -> u32 {
    let lower = raw.to_ascii_lowercase();
    let doc_len = lower.split_whitespace().count().max(1) as u32;
    let mut score = 0u32;
    // BM25-ish: term frequency with length normalization + multi-token boost.
    let tokens: Vec<&str> = query_lc
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .collect();
    if tokens.is_empty() {
        return 0;
    }
    let mut matched = 0u32;
    for tok in &tokens {
        let c = lower.matches(tok).count() as u32;
        if c == 0 {
            continue;
        }
        matched += 1;
        // tf saturation
        let tf = (c * 100) / (c + 2);
        let idf = 3 + tok.len() as u32; // longer rare-ish terms score higher
        score = score.saturating_add(tf.saturating_mul(idf));
    }
    if matched == 0 {
        return 0;
    }
    // Prefer docs that match more query tokens.
    score = score.saturating_add(matched.saturating_mul(40));
    // Soft length penalty.
    score = score.saturating_mul(100) / (80 + doc_len.min(200));
    score
}

/// Window around the first matching query token.
pub(super) fn best_snippet(raw: &str, query_lc: &str, max_len: usize) -> String {
    let lower = raw.to_ascii_lowercase();
    let mut pos = None;
    for tok in query_lc.split_whitespace() {
        if tok.len() < 2 {
            continue;
        }
        if let Some(p) = lower.find(tok) {
            pos = Some(p);
            break;
        }
    }
    // Prefer ~40 chars of lead-in, but keep the match inside the window when
    // `max_len` is small (tests / tight budgets).
    let match_pos = pos.unwrap_or(0);
    let prefix = 40.min(max_len.saturating_sub(1));
    let start = match_pos.saturating_sub(prefix);
    let slice: String = raw
        .chars()
        .skip(start)
        .take(max_len)
        .collect::<String>()
        .replace('\n', " ");
    let mut s = slice.trim().to_string();
    if start > 0 {
        s = format!("…{s}");
    }
    if raw.chars().count() > start + max_len {
        s.push('…');
    }
    s
}

/// Reject path traversal; only `Normal` and `CurDir` components allowed.
pub(super) fn is_safe_rel_path(rel: &str) -> bool {
    use std::path::{Component, Path as StdPath};
    for c in StdPath::new(rel).components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_document_matches_and_misses() {
        let doc = "User prefers dark mode editors for coding.";
        assert!(score_document(doc, "dark mode") > 0);
        assert_eq!(score_document(doc, "zzzz"), 0);
        assert_eq!(score_document(doc, "a"), 0); // token too short
    }

    #[test]
    fn multi_token_scores_higher() {
        let doc = "dark mode coding dark";
        let one = score_document(doc, "dark");
        let two = score_document(doc, "dark mode");
        assert!(two >= one, "one={one} two={two}");
    }

    #[test]
    fn best_snippet_centers_on_match() {
        let raw = "aaaa ".repeat(20) + "dark mode rocks " + &"bbbb ".repeat(20);
        let snip = best_snippet(&raw, "dark", 80);
        assert!(snip.to_ascii_lowercase().contains("dark"), "snippet={snip:?}");
        assert!(snip.starts_with('…'));
    }

    #[test]
    fn safe_rel_path_rejects_dotdot() {
        assert!(is_safe_rel_path("MEMORY.md"));
        assert!(is_safe_rel_path("desktop/MEMORY.md"));
        assert!(!is_safe_rel_path("../secrets.txt"));
        assert!(!is_safe_rel_path("/abs"));
    }
}
