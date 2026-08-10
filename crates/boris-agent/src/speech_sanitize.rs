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
/// Does **not** parse markup into real tool calls — recovery is re-prompt only.
pub fn strip_tool_markup(s: &str) -> String {
    if !contains_tool_markup(s) {
        return s.trim().to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
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
    needles
        .iter()
        .filter_map(|n| lower.find(n))
        .min()
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
}
