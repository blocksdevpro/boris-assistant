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
//! | [`url`]    | HTTP(S) URL validation |
//! | [`encode`] | Minimal query / percent encoding helpers |
//!
//! # Contributor notes
//!
//! - **Public surface**: re-export tool types only. Helpers stay crate-private so
//!   hosts and the model contract do not depend on scraping internals.
//! - **Semantics**: search backends, result formatting, fetch envelope tags, size
//!   caps, and timeouts must stay stable — the model and skills docs reference them.
//! - **Network**: both tools require [`Permission::Network`](crate::tool::Permission)
//!   and host `NetworkPolicy::Open`.
//! - Prefer pure helpers with unit tests over growing the tool `execute` bodies.

mod encode;
mod fetch;
mod html;
mod search;
mod url;

use std::time::Duration;

use reqwest::Client;

use crate::tool::ToolError;

pub use fetch::WebFetchTool;
pub use search::WebSearchTool;

/// Max plain-text characters returned by `web_fetch` before local truncation.
pub(crate) const MAX_FETCH_CHARS: usize = 12_000;

/// Hard cap on `web_search` result count.
pub(crate) const MAX_SEARCH: usize = 8;

/// Shared HTTP client for web tools (connect 10s, total 30s, 5 redirects).
pub(crate) fn http_client() -> Result<Client, ToolError> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("boris-agent/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ToolError::failed(format!("http client: {e}")))
}
