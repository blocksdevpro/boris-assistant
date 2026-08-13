//! Web search and fetch tools (async HTTP).
//!
//! # Tools
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `web_search` | [`WebSearchTool`] | Live web search (no-key public backends; optional Exa) |
//! | `web_fetch`  | [`WebFetchTool`]  | Fetch a single URL as plain text |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`search`] | `web_search` tool (public backends + optional Exa) |
//! | [`search_public`] | No-account backends (DDG HTML, DDG Instant Answer, Wikipedia) |
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
mod search_public;
mod url;

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE};
use reqwest::redirect::{Action, Attempt, Policy as RedirectPolicy};
use reqwest::Client;

use crate::tool::ToolError;

/// Browser-like UA for DuckDuckGo HTML. Their lite/html endpoints return HTTP 202
/// "anomaly" pages for the `boris-agent/…` product UA (verified 2026-08).
const BROWSER_SEARCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

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

fn client_builder(connect_secs: u64, timeout_secs: u64) -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(timeout_secs))
        .redirect(RedirectPolicy::custom(ssrf_redirect_policy))
}

/// Shared HTTP client for web tools (connect 10s, total 30s).
///
/// Redirect policy re-validates every hop with the same SSRF host rules as the
/// initial URL (see [`url::parse_safe_http_url`]).
pub(crate) fn http_client() -> Result<Client, ToolError> {
    client_builder(10, 30)
        .user_agent(concat!("boris-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ToolError::failed(format!("http client: {e}")))
}

/// Official search APIs (Wikipedia, DuckDuckGo Instant Answer, optional Exa).
///
/// Wikimedia requires a descriptive UA with a project URL. Shorter timeouts so
/// a hung official API does not eat the whole tool budget.
pub(crate) fn search_api_client() -> Result<Client, ToolError> {
    client_builder(8, 15)
        .user_agent(concat!(
            "BorisAssistant/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/blocksdevpro/boris-assistant)"
        ))
        .build()
        .map_err(|e| ToolError::failed(format!("search api client: {e}")))
}

/// DuckDuckGo HTML/lite scrape. DDG serves a block page to the product UA.
pub(crate) fn browser_search_client() -> Result<Client, ToolError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    client_builder(8, 12)
        .user_agent(BROWSER_SEARCH_UA)
        .default_headers(headers)
        .build()
        .map_err(|e| ToolError::failed(format!("browser search client: {e}")))
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
