//! HTTP(S) URL validation and SSRF host checks for web tools.
//!
//! # Host policy
//!
//! `web_fetch` (and redirect hops) must not target loopback, link-local,
//! private RFC1918, CGNAT, metadata (e.g. `169.254.169.254`), or IPv6
//! unique-local / link-local addresses. Literal IPs and obvious local
//! hostnames are blocked before the request. Redirect targets are
//! re-validated the same way.
//!
//! **Residual risk**: when the host is a public DNS name, the OS resolver
//! may still map it to a private address at connect time (DNS rebinding).
//! Fully closing that requires a custom connector that pins resolved IPs;
//! this crate blocks the common static cases and documents the residual.
//!
//! For host network policy, see [`crate::runtime::NetworkPolicy::Open`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use reqwest::Url;

use crate::tool::ToolError;

/// Parse and validate a fetch URL; returns the canonical [`Url`].
///
/// Accepts only `http://` / `https://` URLs of reasonable length, with a
/// non-empty host that is not an obvious SSRF target (literal private IP,
/// localhost, link-local, metadata). Pure validation — does not resolve DNS
/// or fetch.
pub(crate) fn parse_safe_http_url(url: &str) -> Result<Url, ToolError> {
    let u = url.trim();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err(ToolError::invalid_args(
            "only http:// and https:// URLs are allowed",
        ));
    }
    if u.len() > 2048 {
        return Err(ToolError::invalid_args("url too long"));
    }
    let parsed = Url::parse(u).map_err(|e| ToolError::invalid_args(format!("invalid url: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ToolError::invalid_args(
            "only http:// and https:// URLs are allowed",
        ));
    }
    // Reject credentials in URL (userinfo) — common SSRF / phishing pattern.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ToolError::invalid_args(
            "urls with embedded credentials are not allowed",
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| ToolError::invalid_args("url missing host"))?;
    ensure_public_host(host)?;
    Ok(parsed)
}

fn ensure_public_host(host: &str) -> Result<(), ToolError> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() {
        return Err(ToolError::invalid_args("url missing host"));
    }

    // Obvious local / special hostnames (no DNS lookup).
    if h == "localhost"
        || h.ends_with(".localhost")
        || h == "localhost.localdomain"
        || h == "ip6-localhost"
        || h == "ip6-loopback"
        || h == "metadata"
        || h == "metadata.google.internal"
        || h.ends_with(".local")
        || h.ends_with(".internal")
        || h.ends_with(".intranet")
        || h.ends_with(".corp")
        || h.ends_with(".home")
        || h.ends_with(".lan")
    {
        return Err(ToolError::invalid_args(format!(
            "host `{h}` is blocked (local/internal)"
        )));
    }

    // IPv4 / IPv6 literals (url crate host_str omits brackets for IPv6).
    if let Ok(ip) = h.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            return Err(ToolError::invalid_args(format!(
                "host `{h}` is blocked (non-public address)"
            )));
        }
        return Ok(());
    }

    // Bracketed IPv6 that might slip through as raw string.
    if let Some(inner) = h.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Ok(ip) = inner.parse::<IpAddr>() {
            if is_blocked_ip(ip) {
                return Err(ToolError::invalid_args(format!(
                    "host `{h}` is blocked (non-public address)"
                )));
            }
            return Ok(());
        }
    }

    Ok(())
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_ipv4(v4),
        IpAddr::V6(v6) => is_blocked_ipv6(v6),
    }
}

fn is_blocked_ipv4(v4: Ipv4Addr) -> bool {
    // Loopback 127.0.0.0/8, unspecified, broadcast, multicast, private RFC1918,
    // link-local 169.254.0.0/16 (includes cloud metadata 169.254.169.254),
    // CGNAT 100.64.0.0/10, documentation/benchmark ranges where available.
    if v4.is_loopback()
        || v4.is_unspecified()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_private()
        || v4.is_link_local()
    {
        return true;
    }
    // CGNAT / shared address space (100.64.0.0/10).
    let o = v4.octets();
    if o[0] == 100 && (o[1] & 0xc0) == 64 {
        return true;
    }
    // 0.0.0.0/8 (current network).
    if o[0] == 0 {
        return true;
    }
    // IETF protocol assignments 192.0.0.0/24 (except some public anycast — still block for safety).
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }
    // TEST-NET / documentation.
    if v4.is_documentation() {
        return true;
    }
    // Reserved 240.0.0.0/4.
    if o[0] >= 240 {
        return true;
    }
    false
}

fn is_blocked_ipv6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || v6.is_unique_local()
        || v6.is_unicast_link_local()
    {
        return true;
    }
    // IPv4-mapped / compatible → re-check embedded v4.
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_blocked_ipv4(v4);
    }
    if let Some(v4) = v6.to_ipv4() {
        // to_ipv4 also covers ::ffff:x.x.x.x on some versions; check again.
        return is_blocked_ipv4(v4);
    }
    // Documentation 2001:db8::/32
    let seg = v6.segments();
    if seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0x0db8 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_blocked(host: &str) -> bool {
        ensure_public_host(host).is_err()
    }

    #[test]
    fn accepts_http_and_https() {
        assert!(parse_safe_http_url("https://example.com").is_ok());
        assert!(parse_safe_http_url("http://example.com/path").is_ok());
        assert!(parse_safe_http_url("  https://x.com  ").is_ok());
        assert!(parse_safe_http_url("https://docs.rs/reqwest").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(parse_safe_http_url("ftp://example.com").is_err());
        assert!(parse_safe_http_url("file:///etc/passwd").is_err());
        assert!(parse_safe_http_url("javascript:alert(1)").is_err());
        assert!(parse_safe_http_url("").is_err());
        assert!(parse_safe_http_url("example.com").is_err());
    }

    #[test]
    fn rejects_overlong_url() {
        let long = format!("https://example.com/{}", "a".repeat(2100));
        assert!(parse_safe_http_url(&long).is_err());
    }

    #[test]
    fn blocks_loopback_and_localhost() {
        assert!(host_blocked("localhost"));
        assert!(host_blocked("LOCALHOST"));
        assert!(host_blocked("foo.localhost"));
        assert!(host_blocked("127.0.0.1"));
        assert!(host_blocked("127.1.2.3"));
        assert!(host_blocked("::1"));
        assert!(parse_safe_http_url("http://127.0.0.1/").is_err());
        assert!(parse_safe_http_url("http://localhost:8080/admin").is_err());
        assert!(parse_safe_http_url("http://[::1]/").is_err());
    }

    #[test]
    fn blocks_private_rfc1918() {
        assert!(host_blocked("10.0.0.1"));
        assert!(host_blocked("172.16.0.1"));
        assert!(host_blocked("172.31.255.255"));
        assert!(host_blocked("192.168.1.1"));
        assert!(parse_safe_http_url("https://192.168.0.1/secret").is_err());
        // 172.15.x and 172.32.x are public-ish (not RFC1918); 172.16–31 only.
        assert!(!host_blocked("172.32.0.1"));
        assert!(!host_blocked("8.8.8.8"));
    }

    #[test]
    fn blocks_link_local_and_metadata() {
        assert!(host_blocked("169.254.169.254"));
        assert!(host_blocked("169.254.1.1"));
        assert!(host_blocked("metadata.google.internal"));
        assert!(parse_safe_http_url("http://169.254.169.254/latest/meta-data/").is_err());
    }

    #[test]
    fn blocks_ipv6_ula_and_link_local() {
        assert!(host_blocked("fc00::1"));
        assert!(host_blocked("fd12:3456:789a::1"));
        assert!(host_blocked("fe80::1"));
        assert!(parse_safe_http_url("http://[fe80::1]/").is_err());
        assert!(parse_safe_http_url("http://[fd00::1]/").is_err());
    }

    #[test]
    fn blocks_cgnat_and_unspecified() {
        assert!(host_blocked("100.64.0.1"));
        assert!(host_blocked("0.0.0.0"));
        assert!(host_blocked("::"));
    }

    #[test]
    fn blocks_internal_suffix_hostnames() {
        assert!(host_blocked("db.internal"));
        assert!(host_blocked("printer.local"));
        assert!(host_blocked("fileserver.corp"));
    }

    #[test]
    fn blocks_userinfo() {
        assert!(parse_safe_http_url("https://user:pass@example.com/").is_err());
    }

    #[test]
    fn allows_public_hostnames() {
        assert!(!host_blocked("example.com"));
        assert!(!host_blocked("api.github.com"));
        assert!(!host_blocked("1.1.1.1"));
    }

    #[test]
    fn parse_safe_returns_url() {
        let u = parse_safe_http_url("https://example.com/a?b=1").unwrap();
        assert_eq!(u.host_str(), Some("example.com"));
    }
}
