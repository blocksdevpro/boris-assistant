//! Strip invalid tool markup from model speech.
//!
//! Tools must only run via structured API `tool_calls`. Models sometimes
//! invent text/`<invoke>` tool syntax in `content` — never execute that, and
//! never speak or store it.

/// Reminder when the model dumps tool XML/JSON into speech instead of API tools.
pub const TOOL_PROTOCOL_REMINDER: &str = "\
<system-reminder>\n\
Tools are ONLY available through the host function-calling API (structured tool_calls). \
Never write tool XML, invoke tags, parameter tags, tool JSON, or fake tool syntax in your spoken text. \
If you need tools, call them as real functions now. After tools finish, speak 1–2 short plain sentences only.\n\
</system-reminder>";

/// True when text looks like tool markup or pseudo-tool syntax (not real API tools).
pub fn contains_tool_markup(s: &str) -> bool {
    if s.trim().is_empty() {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    lower.contains("<invoke")
        || lower.contains("</invoke>")
        || lower.contains("<parameter")
        || lower.contains("</parameter>")
        || lower.contains("<tool_call")
        || lower.contains("</tool_call>")
        || lower.contains("<function")
        || lower.contains("function_call")
        || lower.contains("tool_use")
        || (lower.contains("\"name\"")
            && lower.contains("\"arguments\"")
            && (lower.contains("web_search")
                || lower.contains("web_fetch")
                || lower.contains("load_skill")
                || lower.contains("spawn_subagent")))
}

/// Remove tool-markup spans so only speakable prose remains.
///
/// Removes both XML-style `<invoke>` blocks and JSON pseudo-tool blocks
/// (`{"name": …, "arguments": …}` mentioning known tools, including inside
/// ```json fences). Does **not** parse markup into real tool calls —
/// recovery is re-prompt only.
pub fn strip_tool_markup(s: &str) -> String {
    if !contains_tool_markup(s) {
        return s.trim().to_string();
    }
    // First remove JSON/code-fence pseudo-tool blocks, then XML tags below.
    let without_json = strip_json_pseudo_tool_blocks(&strip_fenced_pseudo_tool_blocks(s));
    let mut out = String::with_capacity(without_json.len());
    let mut rest = without_json.as_str();
    while !rest.is_empty() {
        // Drop common open→close blocks.
        if let Some(start) = find_markup_start(rest) {
            out.push_str(&rest[..start]);
            let after = &rest[start..];
            if let Some(end) = find_markup_end(after) {
                rest = &after[end..];
            } else {
                // No closer — drop the rest of the line / remainder.
                if let Some(nl) = after.find('\n') {
                    rest = &after[nl + 1..];
                } else {
                    rest = "";
                }
            }
            continue;
        }
        out.push_str(rest);
        break;
    }
    // Collapse whitespace left by removals.
    let cleaned: String = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !line_is_tool_noise(l))
        .collect::<Vec<_>>()
        .join(" ");
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// True when original had markup and nothing speakable remains after strip.
pub fn is_markup_only_speech(s: &str) -> bool {
    contains_tool_markup(s) && strip_tool_markup(s).is_empty()
}

/// True when a JSON-ish span looks like a pseudo-tool call for a known tool.
fn looks_like_pseudo_tool_json(block: &str) -> bool {
    let lower = block.to_ascii_lowercase();
    lower.contains("\"name\"")
        && lower.contains("\"arguments\"")
        && (lower.contains("web_search")
            || lower.contains("web_fetch")
            || lower.contains("load_skill")
            || lower.contains("spawn_subagent"))
}

/// Remove ``` fenced blocks that contain pseudo-tool JSON.
///
/// Non-tool fences (e.g. a legit code sample) are preserved.
fn strip_fenced_pseudo_tool_blocks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("```") {
        let after_open = &rest[start + 3..];
        if let Some(close_rel) = after_open.find("```") {
            let close_abs = start + 3 + close_rel + 3;
            let fence = &rest[start..close_abs];
            if looks_like_pseudo_tool_json(fence) {
                out.push_str(&rest[..start]);
                out.push(' ');
                rest = &rest[close_abs..];
            } else {
                out.push_str(&rest[..close_abs]);
                rest = &rest[close_abs..];
            }
        } else {
            // Unclosed fence: drop it when it looks like pseudo-tool JSON,
            // otherwise keep the remainder as-is.
            let fence = &rest[start..];
            if looks_like_pseudo_tool_json(fence) {
                out.push_str(&rest[..start]);
                rest = "";
            } else {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Remove bare `{... "name" ... "arguments" ...}` objects mentioning known tools.
///
/// Uses brace matching that respects `"..."` strings and `\` escapes so
/// prose braces and nested objects don't confuse span detection. Non-tool
/// JSON (no known tool name) is preserved.
fn strip_json_pseudo_tool_blocks(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        if bytes[i] != b'{' {
            // Copy through to the next candidate brace (ASCII, so safe).
            if let Some(rel) = s[i..].find('{') {
                out.push_str(&s[i..i + rel]);
                i += rel;
            } else {
                out.push_str(&s[i..]);
                break;
            }
            continue;
        }
        match find_json_object_end(s, i) {
            Some(end) => {
                let span = &s[i..end];
                if looks_like_pseudo_tool_json(span) {
                    out.push(' ');
                    i = end;
                } else {
                    out.push_str(span);
                    i = end;
                }
            }
            None => {
                // Unbalanced `{`: if the tail looks like a truncated tool
                // call, drop the rest of the line/remainder; otherwise keep it.
                let tail = &s[i..];
                let line_end = tail.find('\n').map(|n| i + n).unwrap_or(s.len());
                let line = &s[i..line_end];
                if looks_like_pseudo_tool_json(line) {
                    out.push(' ');
                    // Drop to end of line, keep following lines for prose.
                    i = if line_end < s.len() { line_end + 1 } else { s.len() };
                    // If the remainder after the dropped line has no prose
                    // (only more JSON noise), the whitespace-collapse + noise
                    // filter below will finish the job.
                } else {
                    out.push_str(tail);
                    break;
                }
            }
        }
    }
    out
}

/// Byte index just past the `}` matching the `{` at `start`, respecting
/// double-quoted strings and backslash escapes. `None` when unbalanced.
fn find_json_object_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut idx = start;
    while idx < bytes.len() {
        let b = bytes[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                // `idx` is ASCII `}`, so +1 is always a char boundary.
                return Some(idx + 1);
            }
        }
        idx += 1;
    }
    None
}

fn find_markup_start(s: &str) -> Option<usize> {
    let lower = s.to_ascii_lowercase();
    let needles = [
        "<invoke",
        "<parameter",
        "<tool_call",
        "<function",
        "</invoke",
        "</parameter",
        "</tool_call",
        "</function",
    ];
    needles.iter().filter_map(|n| lower.find(n)).min()
}

fn find_markup_end(from_start: &str) -> Option<usize> {
    let lower = from_start.to_ascii_lowercase();
    // Prefer full closing tags, then end of line.
    for closer in [
        "</invoke>",
        "</parameter>",
        "</tool_call>",
        "</function>",
        "/>",
    ] {
        if let Some(i) = lower.find(closer) {
            return Some(i + closer.len());
        }
    }
    // Self-contained single tag ending with >
    if let Some(i) = from_start.find('>') {
        return Some(i + 1);
    }
    None
}

fn line_is_tool_noise(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return true;
    }
    let lower = t.to_ascii_lowercase();
    lower.starts_with("<invoke")
        || lower.starts_with("<parameter")
        || lower.starts_with("</")
        || lower.starts_with("tool_call")
        || (lower.starts_with('{') && lower.contains("arguments"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_invoke_xml() {
        assert!(contains_tool_markup(
            r#"Okay.\n<invoke name="web_search">\n<parameter name="query">hi</parameter>\n</invoke>"#
        ));
        assert!(!contains_tool_markup("I found your LinkedIn, bro."));
    }

    #[test]
    fn strips_invoke_keeps_prose() {
        let raw = "Okay now we are talking!\n\n<invoke name=\"web_fetch\">\n\
                   <parameter name=\"url\">https://example.com</parameter>\n\
                   </invoke>";
        let cleaned = strip_tool_markup(raw);
        assert!(cleaned.contains("Okay now we are talking"));
        assert!(!cleaned.contains("invoke"));
        assert!(!cleaned.contains("example.com"));
    }

    #[test]
    fn markup_only_is_empty_after_strip() {
        let raw = r#"<invoke name="web_search"><parameter name="query">x</parameter></invoke>"#;
        assert!(is_markup_only_speech(raw));
        assert!(strip_tool_markup(raw).is_empty());
    }

    #[test]
    fn plain_speech_unchanged() {
        let s = "The chores are done, bro. Trust me.";
        assert!(!contains_tool_markup(s));
        assert_eq!(strip_tool_markup(s), s);
    }

    #[test]
    fn detects_json_pseudo_tool() {
        let raw = r#"thinking… {"name": "web_search", "arguments": "{\"query\": \"alice\"}"}"#;
        assert!(contains_tool_markup(raw));
    }

    #[test]
    fn strips_bare_json_pseudo_tool_block() {
        let raw = r#"{"name": "web_search", "arguments": "{\"query\": \"alice\"}"}"#;
        assert!(is_markup_only_speech(raw));
        assert!(strip_tool_markup(raw).is_empty());
    }

    #[test]
    fn strips_fenced_json_pseudo_tool_block() {
        let raw = "Done digging.\n```json\n{\"name\": \"web_fetch\", \"arguments\": \"{\\\"url\\\": \\\"https://example.com\\\"}\"}\n```";
        let cleaned = strip_tool_markup(raw);
        assert!(cleaned.contains("Done digging"));
        assert!(!cleaned.contains("web_fetch"));
        assert!(!cleaned.contains("example.com"));
        assert!(!cleaned.contains("```"));
    }

    #[test]
    fn mixed_prose_plus_json_keeps_prose() {
        let raw = "Here is what I found, bro.\n{\"name\": \"load_skill\", \"arguments\": \"{\\\"skill\\\": \\\"research\\\"}\"}";
        let cleaned = strip_tool_markup(raw);
        assert!(cleaned.contains("Here is what I found"));
        assert!(!cleaned.contains("load_skill"));
        assert!(!cleaned.contains("arguments"));
    }

    #[test]
    fn non_tool_json_preserved() {
        let s = r#"The result is {"value": 42} for today."#;
        assert!(!contains_tool_markup(s));
        assert_eq!(strip_tool_markup(s), s);
    }

    #[test]
    fn json_with_braces_in_strings_stripped_safely() {
        let raw = "{\"name\": \"web_search\", \"arguments\": \"{\\\"query\\\": \\\"a } b { c\\\"}\"}";
        assert!(is_markup_only_speech(raw));
        assert!(strip_tool_markup(raw).is_empty());
    }
}
