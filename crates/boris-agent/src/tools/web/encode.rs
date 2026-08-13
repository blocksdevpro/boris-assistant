//! Minimal percent-encoding helpers for query strings and DDG redirect unwrap.

/// Encode a string for use as a query parameter value (space → `+`).
pub(crate) fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded string (`+` → space).
pub(crate) fn urlencoding_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &s[i + 1..i + 3];
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract the real destination from a DuckDuckGo redirect URL (`uddg=`).
pub(crate) fn extract_uddg(url: &str) -> Option<String> {
    // //duckduckgo.com/l/?uddg=https%3A%2F%2F...  (HTML may encode `&` as `&amp;`)
    let url = url.replace("&amp;", "&");
    let key = "uddg=";
    let i = url.find(key)?;
    let enc = &url[i + key.len()..];
    let enc = enc.split('&').next().unwrap_or(enc);
    Some(urlencoding_decode(enc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_query() {
        assert_eq!(urlencoding_encode("hello world"), "hello+world");
        assert_eq!(urlencoding_encode("a&b"), "a%26b");
        assert_eq!(urlencoding_encode("safe-._~"), "safe-._~");
    }

    #[test]
    fn decode_query() {
        assert_eq!(urlencoding_decode("hello+world"), "hello world");
        assert_eq!(urlencoding_decode("a%26b"), "a&b");
        assert_eq!(urlencoding_decode("100%25"), "100%");
    }

    #[test]
    fn extract_uddg_from_redirect() {
        let wrapped = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpath&rut=abc";
        let got = extract_uddg(wrapped).expect("uddg");
        assert_eq!(got, "https://example.com/path");
    }

    #[test]
    fn extract_uddg_from_html_entity_amp() {
        let wrapped = "//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&amp;rut=deadbeef";
        let got = extract_uddg(wrapped).expect("uddg");
        assert_eq!(got, "https://rust-lang.org/");
    }

    #[test]
    fn extract_uddg_absent() {
        assert!(extract_uddg("https://example.com").is_none());
    }
}
