//! HTTP(S) URL validation for web tools.

use crate::tool::ToolError;

/// Accept only `http://` / `https://` URLs of reasonable length.
///
/// Pure validation — does not resolve DNS or fetch.
pub(crate) fn ensure_http_url(url: &str) -> Result<(), ToolError> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(ToolError::invalid_args(
            "only http:// and https:// URLs are allowed",
        ));
    }
    if u.len() > 2048 {
        return Err(ToolError::invalid_args("url too long"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https() {
        assert!(ensure_http_url("https://example.com").is_ok());
        assert!(ensure_http_url("http://example.com/path").is_ok());
        assert!(ensure_http_url("  https://x.com  ").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(ensure_http_url("ftp://example.com").is_err());
        assert!(ensure_http_url("file:///etc/passwd").is_err());
        assert!(ensure_http_url("javascript:alert(1)").is_err());
        assert!(ensure_http_url("").is_err());
        assert!(ensure_http_url("example.com").is_err());
    }

    #[test]
    fn rejects_overlong_url() {
        let long = format!("https://example.com/{}", "a".repeat(2100));
        assert!(ensure_http_url(&long).is_err());
    }
}
