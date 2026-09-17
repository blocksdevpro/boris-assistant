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

/// Result of a head+tail truncation, including a resumable cursor.
///
/// `cursor` (`byte:N`) is an informational char-offset hint embedded in the
/// marker text only — it is **not** a tool argument. Re-call with narrower
/// args (`offset`/`limit`/`head_limit`) instead of passing the cursor back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncateOutcome {
    pub text: String,
    pub truncated: bool,
    /// Char offset where the omitted middle / tail resumes (informational only).
    pub cursor: Option<String>,
}

/// Cap tool observation length so context doesn't explode (default 12k chars).
///
/// When cut, keep a useful head **and** tail plus a resumable cursor.
pub fn truncate_tool_result(s: String) -> String {
    truncate_tool_result_to(s, MAX_TOOL_RESULT_CHARS)
}

/// Cap to an explicit character budget (UTF-8 safe by char count).
///
/// Output is always ≤ `max_chars` characters (including the truncation marker).
pub fn truncate_tool_result_to(s: String, max_chars: usize) -> String {
    truncate_tool_result_detailed(s, max_chars).text
}

/// Head + tail truncation with an informational char-offset cursor (`byte:N`).
///
/// The marker tells the model to re-call with narrower args; the cursor is
/// explicitly **not** a tool argument. Offsets are char counts (multibyte
/// safe), kept under the legacy `byte:N` label for wire compatibility.
pub fn truncate_tool_result_detailed(s: String, max_chars: usize) -> TruncateOutcome {
    if max_chars == 0 {
        return TruncateOutcome {
            text: String::new(),
            truncated: !s.is_empty(),
            cursor: if s.is_empty() {
                None
            } else {
                Some("byte:0".into())
            },
        };
    }
    let count = s.chars().count();
    if count <= max_chars {
        return TruncateOutcome {
            text: s,
            truncated: false,
            cursor: None,
        };
    }
    // Reserve room for marker + head/tail. Marker now carries the re-call
    // hint plus the informational-only note, so reserve generously.
    let reserve = 160usize;
    if max_chars <= reserve {
        let suffix_len = TRUNCATED_SUFFIX.chars().count();
        if max_chars <= suffix_len {
            let text: String = TRUNCATED_SUFFIX.chars().take(max_chars).collect();
            return TruncateOutcome {
                text,
                truncated: true,
                cursor: Some("byte:0".into()),
            };
        }
        let keep = max_chars.saturating_sub(suffix_len);
        let head: String = s.chars().take(keep).collect();
        let off = head.chars().count();
        return TruncateOutcome {
            text: format!("{head}{TRUNCATED_SUFFIX}"),
            truncated: true,
            cursor: Some(format!("byte:{off}")),
        };
    }
    let mut keep = max_chars - reserve;
    loop {
        let head_chars = keep / 2;
        let tail_chars = keep - head_chars;
        let head: String = s.chars().take(head_chars).collect();
        let tail: String = s
            .chars()
            .rev()
            .take(tail_chars)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        let off = head.chars().count();
        let (ls, le) = line_span(&s, off);
        let marker = format!(
            "\n…[truncated; re-call with narrower args (offset/limit/head_limit), \
             cursor=byte:{off} lines={ls}-{le} — cursor is informational only, not a tool arg]…\n"
        );
        let marker_chars = marker.chars().count();
        if keep.saturating_add(marker_chars) <= max_chars {
            return TruncateOutcome {
                text: format!("{head}{marker}{tail}"),
                truncated: true,
                cursor: Some(format!("byte:{off}")),
            };
        }
        // Extremely large line/cursor numbers can make the marker exceed the
        // conservative reserve. Shrink retained content until the public hard
        // cap is true for every input.
        let excess = keep
            .saturating_add(marker_chars)
            .saturating_sub(max_chars)
            .max(1);
        keep = keep.saturating_sub(excess);
    }
}

/// Line span for a char-offset cursor (multibyte safe).
///
/// `char_off` is a char count, converted to the nearest char boundary before
/// slicing so split UTF-8 can never panic (`get` + fallback).
fn line_span(s: &str, char_off: usize) -> (usize, usize) {
    let byte_idx = s
        .char_indices()
        .nth(char_off)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let start_lines = s
        .get(..byte_idx)
        .map(|h| h.bytes().filter(|b| *b == b'\n').count() + 1)
        .unwrap_or(1);
    let total = s.bytes().filter(|b| *b == b'\n').count() + 1;
    (start_lines, total)
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
        assert!(out.contains("[truncated"));
        assert!(out.starts_with('a'));
        assert!(out.contains("cursor=byte:"));
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
    fn truncate_many_lines_still_honors_exact_budget() {
        let long = "x\n".repeat(600_000);
        let out = truncate_tool_result_to(long, MAX_TOOL_RESULT_CHARS);
        assert!(out.chars().count() <= MAX_TOOL_RESULT_CHARS);
        assert!(out.contains("cursor=byte:"));
    }

    #[test]
    fn soft_wrap_preserves_content() {
        let line = "a".repeat(5000);
        let wrapped = soft_wrap_line(&line, 2000);
        assert_eq!(wrapped.replace('\n', "").len(), 5000);
        assert!(wrapped.contains('\n'));
    }

    #[test]
    fn truncated_marker_is_actionable_and_cursor_not_a_tool_arg() {
        let long: String = "a".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let out = truncate_tool_result(long);
        assert!(out.contains("re-call with narrower args"));
        assert!(out.contains("offset/limit/head_limit"));
        assert!(out.contains("informational only, not a tool arg"));
        assert!(out.contains("cursor=byte:"));
    }

    #[test]
    fn truncate_multibyte_is_char_safe_and_never_panics() {
        // Emoji are 4 bytes / 1 char: byte-len vs char-count mix would
        // mislabel the cursor and risk splitting UTF-8.
        let s = format!("{}\n{}\n{}", "🎉".repeat(2_000), "line-🎉-mid", "🚀".repeat(2_000));
        let out = truncate_tool_result_to(s.clone(), 1_000);
        assert!(out.chars().count() <= 1_000);
        assert!(out.contains("[truncated"));
        // Cursor value must be a char count, not a byte length.
        let cursor_off: usize = out
            .find("cursor=byte:")
            .map(|i| {
                out[i + "cursor=byte:".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(usize::MAX)
            })
            .unwrap();
        assert!(cursor_off <= 1_000, "cursor {cursor_off} must be char-based");
        // line_span on a split-UTF-8 boundary must not panic.
        let (ls, le) = super::line_span(&s, 1);
        assert!(ls >= 1 && le >= ls);
        let emoji_boundary = "a🎉b";
        // char offset 2 lands mid-emoji in byte terms; must fallback safely.
        let (ls2, _) = super::line_span(emoji_boundary, 2);
        assert_eq!(ls2, 1);
    }
}
