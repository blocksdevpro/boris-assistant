//! Model-facing grep cards (Grok-style wrapper + empty-result hints).

use super::MAX_LINE_CHARS;

/// Collected matches ready to render.
#[derive(Debug, Clone, Default)]
pub(super) struct GrepHits {
    pub lines: Vec<String>,
    pub match_count: usize,
    pub file_count: usize,
    pub truncated: bool,
}

impl GrepHits {
    pub(super) fn render(&self, pattern: &str, search_path: &str, glob: Option<&str>) -> String {
        if self.lines.is_empty() {
            return no_matches(pattern, search_path, glob);
        }
        let body = self
            .lines
            .iter()
            .map(|l| truncate_line(l, MAX_LINE_CHARS))
            .collect::<Vec<_>>()
            .join("\n");
        let mut out =
            format!("<workspace_result path=\"{search_path}\">\n{body}\n</workspace_result>\n");
        let noun = if self.match_count == 1 {
            "match"
        } else {
            "matches"
        };
        let files = if self.file_count == 1 {
            "file"
        } else {
            "files"
        };
        if self.truncated {
            out.push_str(&format!(
                "Found at least {} {noun} in {} {files} (truncated). Narrow path/glob/type, or raise head_limit.",
                self.match_count, self.file_count
            ));
        } else {
            out.push_str(&format!(
                "Found {} {noun} in {} {files}.",
                self.match_count, self.file_count
            ));
        }
        out
    }
}

pub(super) fn no_matches(pattern: &str, search_path: &str, glob: Option<&str>) -> String {
    let mut out = format!(
        "<workspace_result path=\"{search_path}\">\nNo matches found\n</workspace_result>\n\
         No matches for pattern '{pattern}' under {search_path}."
    );
    if let Some(g) = glob {
        out.push_str(&format!(" glob='{g}'."));
    }
    out.push_str(
        " Next: drop glob/type, set -i true, escape regex metacharacters, or search a parent directory. \
         Prefer this grep tool over bash grep/rg.",
    );
    out
}

pub(super) fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let mut s: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// Count unique file prefixes in `path:line:` / `path:line-` / `path:count` lines.
pub(super) fn count_files(lines: &[String]) -> usize {
    let mut seen = Vec::new();
    for line in lines {
        if let Some(path) = file_prefix(line) {
            if !seen.iter().any(|p| p == &path) {
                seen.push(path);
            }
        }
    }
    seen.len()
}

fn file_prefix(line: &str) -> Option<String> {
    // Windows paths contain `:`, so split on the last `:` that looks like a line/count marker.
    let bytes = line.as_bytes();
    let mut colon = None;
    for (i, b) in bytes.iter().enumerate().rev() {
        if (*b == b':' || *b == b'-')
            && i > 0
            && bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit())
        {
            colon = Some(i);
            break;
        }
    }
    colon
        .map(|i| line[..i].to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_hits_include_next_step_hints() {
        let out = GrepHits::default().render("TODO", "C:\\proj", Some("*.rs"));
        assert!(out.contains("No matches found"));
        assert!(out.contains("glob='*.rs'"));
        assert!(out.contains("drop glob") || out.contains("-i"));
    }

    #[test]
    fn render_wraps_and_counts() {
        let hits = GrepHits {
            lines: vec!["a.rs:1:hello".into(), "b.rs:2:hello".into()],
            match_count: 2,
            file_count: 2,
            truncated: false,
        };
        let out = hits.render("hello", "/tmp", None);
        assert!(out.contains("<workspace_result"));
        assert!(out.contains("Found 2 matches in 2 files."));
        assert!(out.contains("a.rs:1:hello"));
    }

    #[test]
    fn long_lines_are_cut() {
        let line = "x".repeat(80);
        let out = truncate_line(&line, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }
}
