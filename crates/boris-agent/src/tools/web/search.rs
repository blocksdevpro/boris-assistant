//! `web_search` tool — no-key public backends by default, optional Exa upgrade.
//!
//! Default path (no account, no config): DuckDuckGo Instant Answer + DDG HTML
//! (browser-compatible headers) + Wikipedia. Exa is used first when
//! `EXA_API_KEY` / `BORIS_EXA_API_KEY` or `~/.boris/auth.json` `exa_api_key`
//! is set; Exa failures fall through to the public backends.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::search_api_client;
use super::search_public::search_public;
use super::{browser_search_client, MAX_SEARCH};
use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

/// Max characters of Exa page text kept per hit (voice agents stay short).
const EXA_SNIPPET_CHARS: usize = 600;

/// Live web search: public backends always; Exa first when a key is present.
#[derive(Debug, Clone)]
pub struct WebSearchTool {
    /// Wikipedia, DDG Instant Answer, optional Exa.
    official: Client,
    /// DDG HTML/lite (blocked unless the UA looks like a browser).
    browser: Client,
    /// Optional explicit key (tests / hosts). Empty → resolve at execute time.
    exa_api_key: Option<String>,
}

impl WebSearchTool {
    pub fn new() -> Result<Self, ToolError> {
        Ok(Self {
            official: search_api_client()?,
            browser: browser_search_client()?,
            exa_api_key: None,
        })
    }

    /// Construct with an explicit Exa API key (non-empty). Prefer env/auth for hosts.
    pub fn with_exa_api_key(key: impl Into<String>) -> Result<Self, ToolError> {
        let key = key.into().trim().to_string();
        Ok(Self {
            official: search_api_client()?,
            browser: browser_search_client()?,
            exa_api_key: if key.is_empty() { None } else { Some(key) },
        })
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new().expect("http client")
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the live web for a query. Returns numbered titles, URLs, and short snippets. \
         Works with no API key. For hard lookups (people, LinkedIn, profiles), call this \
         multiple times in one message with different query angles — do not rely on a single \
         obvious phrase. Prefer this over guessing URLs. Summarize for speech; do not read \
         every result aloud."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": {
                    "type": "number",
                    "description": "Max results (default 8, max 8)"
                }
            },
            "required": ["query"]
        })
    }

    fn meta(&self) -> ToolMeta {
        // Network read — safe to fan out with other lookups in the parallel read wave.
        // Cap lower than before: paid search APIs + anti-bot friendliness.
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Web)
            .permissions(&[Permission::Network])
            .timeout(Duration::from_secs(30))
            .read_only(true)
            .max_concurrency(4)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let query = require_string(obj, "query")?;
        if query.trim().is_empty() {
            return Err(ToolError::invalid_args("query is empty"));
        }
        let limit = parse_search_limit(obj.get("limit"));
        let query = query.trim();

        // Optional Exa upgrade. Failures (including bad keys) fall through so a
        // downloaded build never depends on an Exa account.
        if let Some(api_key) = self.resolve_exa_key() {
            match search_exa(&self.official, &api_key, query, limit).await {
                Ok(results) if !results.is_empty() => {
                    return Ok(truncate_tool_result(format_results(query, &results)));
                }
                Ok(_) => {
                    tracing::debug!(%query, "Exa returned zero hits; trying public search");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e.message,
                        auth = e.looks_like_auth(),
                        %query,
                        "Exa search failed; trying public search"
                    );
                }
            }
        }

        let results = search_public(&self.official, &self.browser, query, limit).await;
        if results.is_empty() {
            return Ok(truncate_tool_result(format!(
                "No search results for: {query} (search backends returned empty — try a simpler query)"
            )));
        }
        Ok(truncate_tool_result(format_results(query, &results)))
    }
}

impl WebSearchTool {
    fn resolve_exa_key(&self) -> Option<String> {
        if let Some(k) = &self.exa_api_key {
            let t = k.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        resolve_exa_api_key_from_env_or_auth()
    }
}

/// Resolve Exa key: env first, then `~/.boris/auth.json` field `exa_api_key`.
pub fn resolve_exa_api_key_from_env_or_auth() -> Option<String> {
    for var in ["EXA_API_KEY", "BORIS_EXA_API_KEY"] {
        if let Ok(v) = std::env::var(var) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    read_exa_key_from_auth_json()
}

fn boris_home_guess() -> PathBuf {
    if let Ok(h) = std::env::var("BORIS_HOME") {
        let p = PathBuf::from(h);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    if let Ok(h) = std::env::var("USERPROFILE") {
        return PathBuf::from(h).join(".boris");
    }
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".boris");
    }
    PathBuf::from(".boris")
}

fn read_exa_key_from_auth_json() -> Option<String> {
    let path = boris_home_guess().join("auth.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let key = v
        .get("exa_api_key")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(key.to_string())
}

// ── Exa ──────────────────────────────────────────────────────────────────────

struct ExaError {
    message: String,
    auth: bool,
}

impl ExaError {
    fn msg(s: impl Into<String>) -> Self {
        Self {
            message: s.into(),
            auth: false,
        }
    }
    fn auth(s: impl Into<String>) -> Self {
        Self {
            message: s.into(),
            auth: true,
        }
    }
    fn looks_like_auth(&self) -> bool {
        self.auth
    }
}

async fn search_exa(
    client: &Client,
    api_key: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, ExaError> {
    let body = json!({
        "query": query,
        "type": "auto",
        "numResults": limit,
        "contents": {
            "text": { "maxCharacters": 2000 }
        }
    });

    let resp = client
        .post("https://api.exa.ai/search")
        .header("x-api-key", api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ExaError::msg(format!("request failed: {e}")))?;

    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ExaError::msg(format!("read body: {e}")))?;

    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ExaError::auth(format!(
            "HTTP {status}: {}",
            truncate_err_body(&text)
        )));
    }
    if !status.is_success() {
        return Err(ExaError::msg(format!(
            "HTTP {status}: {}",
            truncate_err_body(&text)
        )));
    }

    let json: Value =
        serde_json::from_str(&text).map_err(|e| ExaError::msg(format!("invalid JSON: {e}")))?;
    Ok(parse_exa_results(&json, limit))
}

/// Parse Exa `/search` JSON into [`SearchHit`]s (pure; unit-tested).
pub(crate) fn parse_exa_results(json: &Value, limit: usize) -> Vec<SearchHit> {
    let Some(arr) = json.get("results").and_then(|r| r.as_array()) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for r in arr.iter().take(limit) {
        let title = r
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let url = r
            .get("url")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() && url.is_empty() {
            continue;
        }
        let text = r.get("text").and_then(|t| t.as_str()).unwrap_or("");
        let snippet: String = text.chars().take(EXA_SNIPPET_CHARS).collect();
        hits.push(SearchHit {
            title: if title.is_empty() { url.clone() } else { title },
            url,
            snippet,
        });
    }
    hits
}

fn truncate_err_body(s: &str) -> String {
    let t = s.trim();
    t.chars().take(240).collect()
}

fn format_results(query: &str, results: &[SearchHit]) -> String {
    let mut out = format!("Search results for: {query}\n");
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} — {}\n   {}\n",
            i + 1,
            r.title,
            r.url,
            r.snippet
        ));
    }
    out
}

/// Parse `limit` from tool args: default [`MAX_SEARCH`], clamped to `[1, MAX_SEARCH]`.
pub(crate) fn parse_search_limit(v: Option<&Value>) -> usize {
    v.and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(MAX_SEARCH)
        .clamp(1, MAX_SEARCH)
}

/// One search result hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_search_limit_defaults_and_clamps() {
        assert_eq!(parse_search_limit(None), MAX_SEARCH);
        assert_eq!(parse_search_limit(Some(&json!(3))), 3);
        assert_eq!(parse_search_limit(Some(&json!(0))), 1);
        assert_eq!(parse_search_limit(Some(&json!(99))), MAX_SEARCH);
        assert_eq!(parse_search_limit(Some(&json!("nope"))), MAX_SEARCH);
    }

    #[test]
    fn tool_name_stable() {
        let t = WebSearchTool::default();
        assert_eq!(t.name(), "web_search");
    }

    #[test]
    fn parse_exa_sample() {
        let json = json!({
            "results": [
                {
                    "title": "Rust Lang",
                    "url": "https://www.rust-lang.org/",
                    "text": "A language empowering everyone to build reliable and efficient software."
                },
                {
                    "title": "",
                    "url": "https://example.com/only-url",
                    "text": "x"
                }
            ]
        });
        let hits = parse_exa_results(&json, 8);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust Lang");
        assert!(hits[0].url.contains("rust-lang"));
        assert!(hits[0].snippet.contains("empowering"));
        assert_eq!(hits[1].title, "https://example.com/only-url");
    }

    #[test]
    fn parse_exa_empty_or_missing() {
        assert!(parse_exa_results(&json!({}), 5).is_empty());
        assert!(parse_exa_results(&json!({"results": []}), 5).is_empty());
    }
}
