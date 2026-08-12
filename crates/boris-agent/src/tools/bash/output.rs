//! Output truncation and timeout-arg parsing for the bash tool.

use serde_json::Value;

use super::{DEFAULT_TIMEOUT_SECS, MAX_BYTES, MAX_LINES};

/// Truncate combined output like tau (2000 lines / 30KB, keep the tail).
pub(crate) fn truncate_output(mut combined: String) -> String {
    let total_lines = combined.lines().count();
    let truncated_by_bytes = combined.len() > MAX_BYTES;
    if truncated_by_bytes {
        let slice = &combined[..MAX_BYTES.min(combined.len())];
        let cut = slice.rfind('\n').unwrap_or(slice.len());
        combined.truncate(cut);
    }
    let shown: Vec<&str> = combined.lines().collect();
    let truncated_by_lines = shown.len() > MAX_LINES;
    let display: Vec<&str> = if truncated_by_lines {
        shown[shown.len() - MAX_LINES..].to_vec()
    } else {
        shown
    };
    let mut text = display.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    if truncated_by_lines || truncated_by_bytes {
        text.push_str(&format!(
            "\n[Output truncated: showing last {} lines of {}]",
            display.len(),
            total_lines
        ));
    }
    text
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
    fn truncate_output_by_lines_keeps_tail() {
        let many: String = (0..MAX_LINES + 50).map(|i| format!("line-{i}\n")).collect();
        let out = truncate_output(many);
        assert!(out.contains("[Output truncated:"));
        assert!(out.contains(&format!("line-{}", MAX_LINES + 49)));
        assert!(!out.contains("line-0\n"));
        // Last MAX_LINES lines of original, plus trailing newline + notice.
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
