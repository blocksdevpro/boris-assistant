//! Web search and fetch tools (async HTTP).
//!
//! # Tools
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `web_search` | [`WebSearchTool`] | Live web search (DuckDuckGo HTML lite) |
//! | `web_fetch`  | [`WebFetchTool`]  | Fetch a single URL as plain text |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`search`] | `web_search` tool + DDG result scrapers |
//! | [`fetch`]  | `web_fetch` tool |
//! | [`html`]   | HTML→text helpers (strip tags, HTML sniff) |
//! | [`url`]    | HTTP(S) URL validation + SSRF host checks |
//! | [`encode`] | Minimal query / percent encoding helpers |
//!
//! # Contributor notes
//!
//! - **Public surface**: re-export tool types only. Helpers stay crate-private so
//!   hosts and the model contract do not depend on scraping internals.
//! - **Semantics**: search backends, result formatting, fetch envelope tags, size
//!   caps, and timeouts must stay stable — the model and skills docs reference them.
//! - **Network**: both tools require [`Permission::Network`](crate::tool::Permission).
//!   Host policy must allow network via [`NetworkPolicy::Open`](crate::runtime::NetworkPolicy)
//!   (any host, still subject to SSRF host blocks in [`url`]) or
//!   [`NetworkPolicy::Allowlist`](crate::runtime::NetworkPolicy) (exact/suffix host
//!   match on tool URL args). `NetworkPolicy::Off` denies all network tools.
//! - Redirects are limited and each hop is re-validated against SSRF rules.
//! - Prefer pure helpers with unit tests over growing the tool `execute` bodies.

mod encode;
mod fetch;
mod html;
mod search;
mod url;

use std::time::Duration;

use reqwest::redirect::{Action, Attempt, Policy as RedirectPolicy};
use reqwest::Client;

use crate::tool::ToolError;

pub use fetch::WebFetchTool;
pub use search::WebSearchTool;

/// Shared SSRF-safe HTTP(S) URL parse for web tools and `open_url`.
pub(crate) use url::parse_safe_http_url;

/// Max plain-text characters returned by `web_fetch` before local truncation.
pub(crate) const MAX_FETCH_CHARS: usize = 12_000;

/// Hard cap on `web_search` result count.
pub(crate) const MAX_SEARCH: usize = 8;

/// Max redirect hops for web tools (each hop re-checked for SSRF).
const MAX_REDIRECTS: usize = 5;

/// Shared HTTP client for web tools (connect 10s, total 30s).
///
/// Redirect policy re-validates every hop with the same SSRF host rules as the
/// initial URL (see [`url::parse_safe_http_url`]).
pub(crate) fn http_client() -> Result<Client, ToolError> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(RedirectPolicy::custom(ssrf_redirect_policy))
        .user_agent(concat!("boris-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ToolError::failed(format!("http client: {e}")))
}

fn ssrf_redirect_policy(attempt: Attempt) -> Action {
    if attempt.previous().len() >= MAX_REDIRECTS {
        return attempt.error("too many redirects");
    }
    let next = attempt.url();
    match url::parse_safe_http_url(next.as_str()) {
        Ok(_) => attempt.follow(),
        Err(e) => attempt.error(format!("redirect blocked: {}", e.message)),
    }
}
