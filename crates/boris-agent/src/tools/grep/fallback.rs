//! In-process content search: literals stay substring; regex uses the `regex` crate.

use regex::RegexBuilder;

use crate::tool::ToolError;
use crate::tools::path_pattern::{is_common_skip_dir, simple_name_glob};

use super::format::{count_files, GrepHits};
use super::query::{GrepQuery, OutputMode};
use super::{looks_like_regex, MAX_LINE_CHARS};

/// Walk `search_path` and collect hits (substring for literals, regex otherwise).
pub(super) fn rust_grep(
    query: &GrepQuery,
    search_path: &std::path::Path,
) -> Result<GrepHits, ToolError> {
    let matcher = Matcher::compile(&query.pattern, query.ignore_case, query.multiline)?;
    let mut acc = Acc::new(query);
    if search_path.is_file() {
        grep_file(search_path, &matcher, query, &mut acc);
        return Ok(acc.finish());
    }

    let mut stack = vec![search_path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if acc.full() {
            break;
        }
        let rd = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            if acc.full() {
                break;
            }
            let path = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
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
            if !file_allowed(&path, search_path, query) {
                continue;
            }
            grep_file(&path, &matcher, query, &mut acc);
        }
    }
    Ok(acc.finish())
}

fn file_allowed(path: &std::path::Path, root: &std::path::Path, query: &GrepQuery) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(g) = query.glob.as_deref() {
        let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
        if !simple_name_glob(g, name) && !simple_name_glob(g, &rel) {
            return false;
        }
    }
    if let Some(t) = query.file_type.as_deref() {
        if !type_matches(t, path) {
            return false;
        }
    }
    true
}

/// Map `rg --type` names (and short aliases) to extensions.
pub(super) fn type_matches(type_name: &str, path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let t = type_name.trim().to_ascii_lowercase();
    match t.as_str() {
        "rust" | "rs" => ext == "rs",
        "py" | "python" => ext == "py",
        "js" | "javascript" => matches!(ext.as_str(), "js" | "jsx" | "mjs" | "cjs"),
        "ts" | "typescript" => matches!(ext.as_str(), "ts" | "tsx"),
        "go" => ext == "go",
        "java" => ext == "java",
        "md" | "markdown" => matches!(ext.as_str(), "md" | "mdx"),
        "toml" => ext == "toml",
        "json" => ext == "json",
        "c" => matches!(ext.as_str(), "c" | "h"),
        "cpp" | "cc" | "cxx" => matches!(ext.as_str(), "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx"),
        "cs" | "csharp" => ext == "cs",
        "rb" | "ruby" => ext == "rb",
        "php" => ext == "php",
        "sh" | "bash" => matches!(ext.as_str(), "sh" | "bash"),
        "html" => matches!(ext.as_str(), "html" | "htm"),
        "css" => matches!(ext.as_str(), "css" | "scss"),
        "yaml" | "yml" => matches!(ext.as_str(), "yaml" | "yml"),
        other => ext == other,
    }
}

enum Matcher {
    Literal { needle: String, ignore_case: bool },
    Regex(regex::Regex),
}

impl Matcher {
    fn compile(pattern: &str, ignore_case: bool, multiline: bool) -> Result<Self, ToolError> {
        if multiline || looks_like_regex(pattern) {
            let re = RegexBuilder::new(pattern)
                .case_insensitive(ignore_case)
                .dot_matches_new_line(multiline)
                .multi_line(true)
                .build()
                .map_err(|e| {
                    ToolError::invalid_args(format!(
                        "Invalid regex: {e}. Escape literal special characters \
                         (`functionCall\\(`, `interface\\{{\\}}`). Pass the pattern as a raw \
                         string — no surrounding quotes."
                    ))
                })?;
            Ok(Self::Regex(re))
        } else {
            Ok(Self::Literal {
                needle: if ignore_case {
                    pattern.to_ascii_lowercase()
                } else {
                    pattern.to_string()
                },
                ignore_case,
            })
        }
    }

    fn is_match_line(&self, line: &str) -> bool {
        match self {
            Self::Literal {
                needle,
                ignore_case,
            } => {
                if *ignore_case {
                    line.to_ascii_lowercase().contains(needle)
                } else {
                    line.contains(needle)
                }
            }
            Self::Regex(re) => re.is_match(line),
        }
    }

    fn find_spans<'a>(&'a self, text: &'a str) -> Vec<(usize, usize)> {
        match self {
            Self::Literal { .. } => Vec::new(),
            Self::Regex(re) => re.find_iter(text).map(|m| (m.start(), m.end())).collect(),
        }
    }
}

struct Acc<'a> {
    query: &'a GrepQuery,
    lines: Vec<String>,
    match_count: usize,
    files: Vec<String>,
    truncated: bool,
}

impl<'a> Acc<'a> {
    fn new(query: &'a GrepQuery) -> Self {
        Self {
            query,
            lines: Vec::new(),
            match_count: 0,
            files: Vec::new(),
            truncated: false,
        }
    }

    fn full(&self) -> bool {
        self.lines.len() >= self.query.limit
    }

    fn note_file(&mut self, path: &std::path::Path) {
        let s = path.display().to_string();
        if !self.files.iter().any(|p| p == &s) {
            self.files.push(s);
        }
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() >= self.query.limit {
            self.truncated = true;
            return;
        }
        self.lines.push(line);
        if self.lines.len() >= self.query.limit {
            self.truncated = true;
        }
    }

    fn finish(self) -> GrepHits {
        let file_count = if self.files.is_empty() {
            count_files(&self.lines)
        } else {
            self.files.len()
        };
        GrepHits {
            lines: self.lines,
            match_count: self.match_count,
            file_count,
            truncated: self.truncated && self.match_count > 0,
        }
    }
}

fn grep_file(path: &std::path::Path, matcher: &Matcher, query: &GrepQuery, acc: &mut Acc<'_>) {
    if acc.full() {
        return;
    }
    const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_FILE_BYTES {
            return;
        }
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

    if query.multiline {
        grep_multiline(path, matcher, query, &text, acc);
        return;
    }

    let lines: Vec<&str> = text.lines().collect();
    match query.output_mode {
        OutputMode::FilesWithMatches => {
            if lines.iter().any(|l| matcher.is_match_line(l)) {
                acc.match_count += 1;
                acc.note_file(path);
                acc.push_line(path.display().to_string());
            }
        }
        OutputMode::Count => {
            let n = lines.iter().filter(|l| matcher.is_match_line(l)).count();
            if n > 0 {
                acc.match_count += n;
                acc.note_file(path);
                acc.push_line(format!("{}:{n}", path.display()));
            }
        }
        OutputMode::Content => {
            emit_content_hits(path, matcher, query, &lines, acc);
        }
    }
}

fn emit_content_hits(
    path: &std::path::Path,
    matcher: &Matcher,
    query: &GrepQuery,
    lines: &[&str],
    acc: &mut Acc<'_>,
) {
    let mut pending_context: Vec<(usize, &str)> = Vec::new();
    let mut after_left = 0usize;
    let mut last_emitted: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if acc.full() {
            break;
        }
        let hit = matcher.is_match_line(line);
        if hit {
            acc.match_count += 1;
            acc.note_file(path);
            if query.before > 0 {
                let start = i.saturating_sub(query.before);
                for (j, ctx) in pending_context.iter() {
                    if *j >= start && last_emitted.is_none_or(|e| *j > e) {
                        acc.push_line(format_ctx(path, *j, ctx));
                        last_emitted = Some(*j);
                    }
                }
            }
            acc.push_line(format_match(path, i, line));
            last_emitted = Some(i);
            after_left = query.after;
        } else if after_left > 0 {
            if last_emitted.is_none_or(|e| i > e) {
                acc.push_line(format_ctx(path, i, line));
                last_emitted = Some(i);
            }
            after_left -= 1;
        }
        if query.before > 0 {
            pending_context.push((i, line));
            if pending_context.len() > query.before {
                pending_context.remove(0);
            }
        }
    }
}

fn grep_multiline(
    path: &std::path::Path,
    matcher: &Matcher,
    query: &GrepQuery,
    text: &str,
    acc: &mut Acc<'_>,
) {
    let spans = matcher.find_spans(text);
    if spans.is_empty() && !matches!(matcher, Matcher::Literal { .. }) {
        return;
    }
    // Literal multiline is still line-oriented; scan lines.
    if matches!(matcher, Matcher::Literal { .. }) {
        let lines: Vec<&str> = text.lines().collect();
        emit_content_hits(path, matcher, query, &lines, acc);
        return;
    }
    if spans.is_empty() {
        return;
    }
    match query.output_mode {
        OutputMode::FilesWithMatches => {
            acc.match_count += 1;
            acc.note_file(path);
            acc.push_line(path.display().to_string());
        }
        OutputMode::Count => {
            acc.match_count += spans.len();
            acc.note_file(path);
            acc.push_line(format!("{}:{}", path.display(), spans.len()));
        }
        OutputMode::Content => {
            let line_starts = line_start_indices(text);
            let lines: Vec<&str> = text.lines().collect();
            let mut emitted = std::collections::BTreeSet::new();
            for (start, _) in spans {
                if acc.full() {
                    break;
                }
                let line_idx = line_index_at(&line_starts, start);
                acc.match_count += 1;
                acc.note_file(path);
                let from = line_idx.saturating_sub(query.before);
                let to = (line_idx + 1 + query.after).min(lines.len());
                for (i, line) in lines.iter().enumerate().take(to).skip(from) {
                    if !emitted.insert(i) {
                        continue;
                    }
                    if i == line_idx {
                        acc.push_line(format_match(path, i, line));
                    } else {
                        acc.push_line(format_ctx(path, i, line));
                    }
                }
            }
        }
    }
}

fn line_start_indices(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            out.push(i + 1);
        }
    }
    out
}

fn line_index_at(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

fn format_match(path: &std::path::Path, idx: usize, line: &str) -> String {
    let clipped = if line.len() > MAX_LINE_CHARS {
        super::format::truncate_line(line, MAX_LINE_CHARS)
    } else {
        line.to_string()
    };
    format!("{}:{}:{clipped}", path.display(), idx + 1)
}

fn format_ctx(path: &std::path::Path, idx: usize, line: &str) -> String {
    let clipped = if line.len() > MAX_LINE_CHARS {
        super::format::truncate_line(line, MAX_LINE_CHARS)
    } else {
        line.to_string()
    };
    format!("{}:{}-{clipped}", path.display(), idx + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::grep::query::GrepQuery;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn q(pattern: &str) -> GrepQuery {
        GrepQuery {
            pattern: pattern.into(),
            path: None,
            glob: None,
            file_type: None,
            ignore_case: false,
            multiline: false,
            before: 0,
            after: 0,
            output_mode: OutputMode::Content,
            limit: 50,
        }
    }

    fn temp(tag: &str) -> std::path::PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-grep-fb-{tag}-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_line_in_file() {
        let dir = temp("find");
        std::fs::write(dir.join("a.txt"), "hello\nFINDME please\nbye\n").unwrap();
        let mut query = q("findme");
        query.ignore_case = true;
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.match_count, 1);
        assert!(hits.lines[0].contains("FINDME"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_is_not_substring() {
        let dir = temp("re");
        std::fs::write(dir.join("a.rs"), "fn main() {}\nfn helper() {}\nTODO\n").unwrap();
        let hits = rust_grep(&q(r"fn\s+\w+"), &dir).unwrap();
        assert_eq!(hits.match_count, 2, "got {:?}", hits.lines);
        let todo = rust_grep(&q("TODO"), &dir).unwrap();
        assert_eq!(todo.match_count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn context_lines_use_dash_marker() {
        let dir = temp("ctx");
        std::fs::write(dir.join("a.txt"), "one\ntwo\nHIT\nfour\nfive\n").unwrap();
        let mut query = q("HIT");
        query.before = 1;
        query.after = 1;
        let hits = rust_grep(&query, &dir).unwrap();
        let joined = hits.lines.join("\n");
        assert!(joined.contains("two"));
        assert!(joined.contains("HIT"));
        assert!(joined.contains("four"));
        assert!(
            joined.contains('-'),
            "context should use '-' marker: {joined}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_and_type_filter() {
        let dir = temp("glob");
        std::fs::write(dir.join("a.rs"), "needle here\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle here\n").unwrap();
        let mut query = q("needle");
        query.glob = Some("*.rs".into());
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.match_count, 1);
        assert!(hits.lines[0].contains("a.rs"));

        query.glob = None;
        query.file_type = Some("rust".into());
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.match_count, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_with_matches_and_count() {
        let dir = temp("modes");
        std::fs::write(dir.join("a.txt"), "x\nx\n").unwrap();
        std::fs::write(dir.join("b.txt"), "y\n").unwrap();
        let mut query = q("x");
        query.output_mode = OutputMode::FilesWithMatches;
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.lines.len(), 1);
        assert!(hits.lines[0].ends_with("a.txt") || hits.lines[0].contains("a.txt"));

        query.output_mode = OutputMode::Count;
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.match_count, 2);
        assert!(hits.lines[0].ends_with(":2") || hits.lines.iter().any(|l| l.contains(":2")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_regex_errors() {
        let dir = temp("badre");
        std::fs::write(dir.join("a.txt"), "x\n").unwrap();
        let err = rust_grep(&q("(unclosed"), &dir).unwrap_err();
        assert!(err.message.to_ascii_lowercase().contains("regex"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skips_binary_files() {
        let dir = temp("bin");
        let mut bytes = b"needle".to_vec();
        bytes.push(0);
        bytes.extend_from_slice(b"more");
        std::fs::write(dir.join("x.bin"), bytes).unwrap();
        let hits = rust_grep(&q("needle"), &dir).unwrap();
        assert_eq!(hits.match_count, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn respects_limit() {
        let dir = temp("lim");
        std::fs::write(dir.join("a.txt"), "x\nx\nx\nx\n").unwrap();
        let mut query = q("x");
        query.limit = 2;
        let hits = rust_grep(&query, &dir).unwrap();
        assert_eq!(hits.lines.len(), 2);
        assert!(hits.truncated);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
