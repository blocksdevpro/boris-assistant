//! Output truncation and timeout-arg parsing for the bash tool.

use serde_json::Value;

use super::{DEFAULT_TIMEOUT_SECS, MAX_BYTES, MAX_LINES};

/// Truncate combined output: keep the beginning and the end (Grok-style).
///
/// Caps: last-resort 2000 lines / 30KB. The middle is dropped so the model
/// still sees command headers *and* the error tail.
pub(crate) fn truncate_output(combined: String) -> String {
    let total_lines = combined.lines().count();
    let over_bytes = combined.len() > MAX_BYTES;
    let over_lines = total_lines > MAX_LINES;
    if !over_bytes && !over_lines {
        let mut text = combined;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        return text;
    }

    const HEAD_LINES: usize = 200;
    let all: Vec<&str> = combined.lines().collect();
    let mut body = String::new();
    if over_lines {
        let tail_n = MAX_LINES.saturating_sub(HEAD_LINES);
        let tail_start = all.len().saturating_sub(tail_n).max(HEAD_LINES);
        let omitted = all
            .len()
            .saturating_sub(HEAD_LINES + (all.len() - tail_start));
        for line in all.iter().take(HEAD_LINES) {
            body.push_str(line);
            body.push('\n');
        }
        body.push_str(&format!("\n[... middle omitted: {omitted} lines ...]\n\n"));
        for line in all.iter().skip(tail_start) {
            body.push_str(line);
            body.push('\n');
        }
    } else {
        body.push_str(&combined);
        if !body.ends_with('\n') {
            body.push('\n');
        }
    }

    if body.len() > MAX_BYTES {
        let keep_head = MAX_BYTES / 3;
        let keep_tail = MAX_BYTES.saturating_sub(keep_head);
        let head_end = floor_nl(&body, keep_head);
        let tail_at = ceil_nl(&body, body.len().saturating_sub(keep_tail));
        if tail_at > head_end {
            body = format!(
                "{}\n[... middle truncated by bytes ...]\n{}",
                &body[..head_end],
                &body[tail_at..]
            );
        }
    }

    body.push_str(&format!(
        "\n[Output truncated: showing head+tail of {total_lines} lines]\n"
    ));
    body
}

fn floor_nl(s: &str, mut at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    at = s.floor_char_boundary(at);
    s[..at].rfind('\n').unwrap_or(at)
}

fn ceil_nl(s: &str, mut at: usize) -> usize {
    if at >= s.len() {
        return s.len();
    }
    at = s.floor_char_boundary(at);
    match s[at..].find('\n') {
        Some(i) => at + i + 1,
        None => at,
    }
}

/// Parse timeout from tool args (`timeout` or `timeout_secs`), default 120, clamp 1–300.
pub(crate) fn parse_timeout_secs(obj: &serde_json::Map<String, Value>) -> u64 {
    obj.get("timeout")
        .or_else(|| obj.get("timeout_secs"))
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, 300)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(v: Value) -> serde_json::Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn parse_timeout_defaults_and_clamps() {
        assert_eq!(parse_timeout_secs(&map(json!({}))), 120);
        assert_eq!(parse_timeout_secs(&map(json!({ "timeout": 30 }))), 30);
        assert_eq!(parse_timeout_secs(&map(json!({ "timeout_secs": 45 }))), 45);
        assert_eq!(parse_timeout_secs(&map(json!({ "timeout": 0 }))), 1);
        assert_eq!(parse_timeout_secs(&map(json!({ "timeout": 999 }))), 300);
        // `timeout` wins over `timeout_secs` when both present
        assert_eq!(
            parse_timeout_secs(&map(json!({ "timeout": 10, "timeout_secs": 50 }))),
            10
        );
    }

    #[test]
    fn truncate_output_short_unchanged() {
        let out = truncate_output("hello\nworld\n".into());
        assert_eq!(out, "hello\nworld\n");
    }

    #[test]
    fn truncate_output_by_lines_keeps_head_and_tail() {
        let many: String = (0..MAX_LINES + 50).map(|i| format!("line-{i}\n")).collect();
        let out = truncate_output(many);
        assert!(out.contains("[Output truncated:"));
        assert!(out.contains("middle omitted") || out.contains("head+tail"));
        assert!(out.contains("line-0"));
        assert!(out.contains(&format!("line-{}", MAX_LINES + 49)));
        let body_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("line-")).collect();
        assert_eq!(body_lines.len(), MAX_LINES);
    }

    #[test]
    fn truncate_output_by_bytes() {
        // Build a string larger than MAX_BYTES with many short lines.
        let mut s = String::new();
        while s.len() <= MAX_BYTES {
            s.push_str("abcdefghij\n");
        }
        let out = truncate_output(s);
        assert!(out.contains("[Output truncated:"));
        // Truncated body should be under or near the byte cap (notice adds a bit).
        let notice_start = out.find("\n[Output truncated:").unwrap();
        assert!(notice_start <= MAX_BYTES);
    }
}
