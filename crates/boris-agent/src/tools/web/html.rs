//! HTML → plain text helpers used by search scrapers and fetch.

/// Strip HTML tags and collapse whitespace to single spaces.
pub(crate) fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // collapse whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Strip tags, decode common entities, collapse whitespace — search snippets.
pub(crate) fn plain_from_html(s: &str) -> String {
    decode_html_entities(&strip_tags(s))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decode a small set of HTML entities used in search snippets / hrefs.
pub(crate) fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(end) = after.find(';') {
            let ent = &after[..=end];
            if let Some(ch) = entity_to_char(ent) {
                out.push(ch);
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

fn entity_to_char(ent: &str) -> Option<char> {
    match ent {
        "&amp;" => Some('&'),
        "&lt;" => Some('<'),
        "&gt;" => Some('>'),
        "&quot;" => Some('"'),
        "&apos;" | "&#39;" | "&#039;" => Some('\''),
        "&nbsp;" => Some(' '),
        _ if ent.len() > 4 && (ent.starts_with("&#x") || ent.starts_with("&#X")) => {
            let hex = &ent[3..ent.len() - 1];
            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        }
        _ if ent.len() > 3 && ent.starts_with("&#") => {
            let n = ent[2..ent.len() - 1].parse::<u32>().ok()?;
            char::from_u32(n)
        }
        _ => None,
    }
}

/// Sniff the first bytes of a body for HTML markup when Content-Type is missing.
pub(crate) fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html")
}

/// Convert HTML to readable text: prefer `htmd` markdown, fall back to tag strip.
pub(crate) fn html_to_text(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| strip_tags(html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_tags_removes_markup() {
        assert_eq!(strip_tags("<b>Hello</b> <i>world</i>"), "Hello world");
        assert_eq!(strip_tags("plain"), "plain");
        assert_eq!(strip_tags("  a   \n  b  "), "a b");
    }

    #[test]
    fn strip_tags_nested() {
        assert_eq!(
            strip_tags("<div><span>nested</span> text</div>"),
            "nested text"
        );
    }

    #[test]
    fn decode_common_entities() {
        assert_eq!(decode_html_entities("a &amp; b"), "a & b");
        assert_eq!(decode_html_entities("it&#039;s"), "it's");
        assert_eq!(decode_html_entities("x &unknown; y"), "x &unknown; y");
    }

    #[test]
    fn plain_from_html_strips_and_decodes() {
        assert_eq!(
            plain_from_html("<span class=\"searchmatch\">Rust</span> &amp; friends"),
            "Rust & friends"
        );
    }

    #[test]
    fn looks_like_html_detects_doctype_and_html() {
        assert!(looks_like_html(b"<!DOCTYPE html><html>"));
        assert!(looks_like_html(b"<html lang=\"en\">"));
        assert!(!looks_like_html(b"{\"json\": true}"));
        assert!(!looks_like_html(b"plain text body"));
    }

    #[test]
    fn html_to_text_produces_nonempty() {
        let text = html_to_text("<h1>Title</h1><p>Body para</p>");
        assert!(text.to_ascii_lowercase().contains("title") || text.contains("Body"));
        assert!(!text.is_empty());
    }
}
