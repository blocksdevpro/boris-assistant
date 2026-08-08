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
