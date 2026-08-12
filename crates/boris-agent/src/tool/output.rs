//! Observation truncation and soft-wrap helpers (voice-sized context).

/// Cap tool observation length so context doesn't explode.
///
/// Raised from 4k → 12k so web search / file reads keep usable signal for
/// multi-step agent work (compaction still trims older turns later).
pub const MAX_TOOL_RESULT_CHARS: usize = 12_000;

/// Higher cap for skill bodies (playbooks must stay intact for multi-step work).
pub const MAX_SKILL_RESULT_CHARS: usize = 24_000;

/// Soft-wrap width for long single lines (bash / dumps) — content preserved.
pub const DEFAULT_SOFT_WRAP_WIDTH: usize = 2_000;

const TRUNCATED_SUFFIX: &str = "\n…[truncated]";

/// Cap tool observation length so context doesn't explode (default 12k chars).
///
/// When cut, appends a short marker so the model knows the result was truncated.
pub fn truncate_tool_result(s: String) -> String {
    truncate_tool_result_to(s, MAX_TOOL_RESULT_CHARS)
}

/// Cap to an explicit character budget (UTF-8 safe by char count).
///
/// Output is always ≤ `max_chars` characters (including the truncation marker).
pub fn truncate_tool_result_to(s: String, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s;
    }
    let suffix_len = TRUNCATED_SUFFIX.chars().count();
    if max_chars <= suffix_len {
        return TRUNCATED_SUFFIX.chars().take(max_chars).collect();
    }
    let keep = max_chars - suffix_len;
    let head: String = s.chars().take(keep).collect();
    format!("{head}{TRUNCATED_SUFFIX}")
}

/// Soft-wrap a long line by inserting newlines every `wrap_width` characters.
/// **All content is preserved** (Grok bash strategy for long lines).
pub fn soft_wrap_line(line: &str, wrap_width: usize) -> String {
    if wrap_width == 0 || line.chars().count() <= wrap_width {
        return line.to_string();
    }
    let mut result = String::with_capacity(line.len() + line.len() / wrap_width);
    let mut on_line = 0;
    for ch in line.chars() {
        if on_line >= wrap_width {
            result.push('\n');
            on_line = 0;
        }
        result.push(ch);
        on_line += 1;
    }
    result
}

/// Soft-wrap every line of a multi-line string that exceeds `wrap_width`.
pub fn soft_wrap_text(text: &str, wrap_width: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&soft_wrap_line(line, wrap_width));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_leaves_short_strings_unchanged() {
        let s = "hello".to_string();
        assert_eq!(truncate_tool_result(s.clone()), s);
    }

    #[test]
    fn truncate_caps_long_strings() {
        let long: String = "a".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let out = truncate_tool_result(long);
        assert!(out.chars().count() <= MAX_TOOL_RESULT_CHARS);
        assert!(out.ends_with(TRUNCATED_SUFFIX));
        assert!(out.starts_with('a'));
    }

    #[test]
    fn truncate_at_exact_limit_is_unchanged() {
        let exact: String = "x".repeat(MAX_TOOL_RESULT_CHARS);
        let out = truncate_tool_result(exact.clone());
        assert_eq!(out, exact);
        assert!(!out.contains("[truncated]"));
    }

    #[test]
    fn truncate_to_custom_budget() {
        let s = "abcdefghij".to_string();
        let out = truncate_tool_result_to(s, 6);
        assert!(out.chars().count() <= 6);
        assert!(out.contains('…') || out.contains("truncated"));
    }

    #[test]
    fn soft_wrap_preserves_content() {
        let line = "a".repeat(5000);
        let wrapped = soft_wrap_line(&line, 2000);
        assert_eq!(wrapped.replace('\n', "").len(), 5000);
        assert!(wrapped.contains('\n'));
    }
}
