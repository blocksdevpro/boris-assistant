//! `web_search` tool — Exa API primary, DuckDuckGo HTML scrape as fallback.
//!
//! Prefer Exa when `EXA_API_KEY` / `BORIS_EXA_API_KEY` or `~/.boris/auth.json`
//! `exa_api_key` is set. DDG is best-effort only (CAPTCHA / markup changes).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use super::encode::{extract_uddg, urlencoding_encode};
use super::html::strip_tags;
use super::http_client;
use super::MAX_SEARCH;
use crate::tool::{
    require_object, require_string, truncate_tool_result, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

/// Max characters of Exa page text kept per hit (voice agents stay short).
const EXA_SNIPPET_CHARS: usize = 600;

/// Best-effort web search: Exa when keyed, else DuckDuckGo HTML scrape.
#[derive(Debug, Clone)]
pub struct WebSearchTool {
    client: Client,
    /// Optional explicit key (tests / hosts). Empty → resolve at execute time.
    exa_api_key: Option<String>,
}

impl WebSearchTool {
    pub fn new() -> Result<Self, ToolError> {
        Ok(Self {
            client: http_client()?,
            exa_api_key: None,
        })
    }

    /// Construct with an explicit Exa API key (non-empty). Prefer env/auth for hosts.
    pub fn with_exa_api_key(key: impl Into<String>) -> Result<Self, ToolError> {
        let key = key.into().trim().to_string();
        Ok(Self {
            client: http_client()?,
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
         Uses Exa when configured, otherwise a best-effort DuckDuckGo scrape. For hard lookups \
         (people, LinkedIn, profiles), call this multiple times in one message with different \
         query angles — do not rely on a single obvious phrase. Prefer this over guessing URLs. \
         Summarize for speech; do not read every result aloud."
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

        // 1) Exa (primary) when a key is available.
        if let Some(api_key) = self.resolve_exa_key() {
            match search_exa(&self.client, &api_key, query, limit).await {
                Ok(results) if !results.is_empty() => {
                    return Ok(truncate_tool_result(format_results(query, &results)));
                }
                Ok(_) => {
                    tracing::debug!(%query, "Exa returned zero hits; trying DDG fallback");
                }
                Err(e) => {
                    // Auth/config errors: fail loudly so the host can fix the key.
                    if e.looks_like_auth() {
                        return Ok(truncate_tool_result(format!(
                            "Exa search failed (auth/config): {}. Check EXA_API_KEY / ~/.boris/auth.json exa_api_key.",
                            e.message
                        )));
                    }
                    tracing::warn!(error = %e.message, %query, "Exa search failed; trying DDG fallback");
                }
            }
        } else {
            tracing::debug!("no Exa API key; using DuckDuckGo scrape only");
        }

        // 2) DuckDuckGo HTML lite / HTML (fallback).
        let results = search_ddg(&self.client, query, limit).await;
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

    let json: Value = serde_json::from_str(&text)
        .map_err(|e| ExaError::msg(format!("invalid JSON: {e}")))?;
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
            title: if title.is_empty() {
                url.clone()
            } else {
                title
            },
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

// ── DuckDuckGo fallback ──────────────────────────────────────────────────────

async fn search_ddg(client: &Client, query: &str, limit: usize) -> Vec<SearchHit> {
    let q = urlencoding_encode(query);
    let mut results = Vec::new();
    for endpoint in [
        format!("https://lite.duckduckgo.com/lite/?q={q}"),
        format!("https://html.duckduckgo.com/html/?q={q}"),
    ] {
        let resp = match client.get(&endpoint).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, %endpoint, "search request failed");
                continue;
            }
        };
        if !resp.status().is_success() {
            tracing::debug!(status = %resp.status(), %endpoint, "search HTTP non-success");
            continue;
        }
        let html = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(error = %e, "search body read failed");
                continue;
            }
        };
        results = parse_ddg_html(&html, limit);
        if results.is_empty() {
            results = parse_ddg_lite(&html, limit);
        }
        if !results.is_empty() {
            break;
        }
    }
    results
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

/// DuckDuckGo lite result table scraper.
pub(crate) fn parse_ddg_lite(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut rest = html;
    while hits.len() < limit {
        // lite uses class="result-link" or plain result__a sometimes
        let idx = rest
            .find("class=\"result-link\"")
            .or_else(|| rest.find("class='result-link'"))
            .or_else(|| rest.find("result-link"));
        let Some(idx) = idx else {
            break;
        };
        rest = &rest[idx..];
        let Some(href_i) = rest.find("href=\"") else {
            rest = &rest[1..];
            continue;
        };
        let after_href = &rest[href_i + 6..];
        let Some(end_h) = after_href.find('"') else {
            break;
        };
        let mut url = after_href[..end_h].to_string();
        if let Some(uddg) = extract_uddg(&url) {
            url = uddg;
        }
        let after_a = after_href.get(end_h..).unwrap_or("");
        let Some(gt) = after_a.find('>') else {
            rest = &rest[1..];
            continue;
        };
        let title_start = &after_a[gt + 1..];
        let Some(end_title) = title_start.find("</a>") else {
            rest = &rest[1..];
            continue;
        };
        let title = strip_tags(&title_start[..end_title]);
        if !title.is_empty() && (url.starts_with("http") || url.starts_with("//")) {
            if url.starts_with("//") {
                url = format!("https:{url}");
            }
            hits.push(SearchHit {
                title,
                url,
                snippet: String::new(),
            });
        }
        // Advance past this match to avoid infinite loop.
        rest = rest.get(10..).unwrap_or("");
    }
    hits
}

/// Very small HTML scraper for DDG result blocks.
pub(crate) fn parse_ddg_html(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    // Look for result links: class="result__a" href="..."
    let mut rest = html;
    while hits.len() < limit {
        let Some(idx) = rest.find("result__a") else {
            break;
        };
        rest = &rest[idx..];
        let Some(href_i) = rest.find("href=\"") else {
            break;
        };
        let after_href = &rest[href_i + 6..];
        let Some(end_h) = after_href.find('"') else {
            break;
        };
        let mut url = after_href[..end_h].to_string();
        // DDG sometimes wraps redirects.
        if let Some(uddg) = extract_uddg(&url) {
            url = uddg;
        }

        let after_a = after_href.get(end_h..).unwrap_or("");
        let Some(gt) = after_a.find('>') else {
            rest = &rest[1..];
            continue;
        };
        let title_start = &after_a[gt + 1..];
        let Some(end_title) = title_start.find("</a>") else {
            rest = &rest[1..];
            continue;
        };
        let title = strip_tags(&title_start[..end_title]);

        // Snippet: result__snippet
        let snippet = if let Some(s_i) = rest.find("result__snippet") {
            let s_rest = &rest[s_i..];
            if let Some(gt2) = s_rest.find('>') {
                let body = &s_rest[gt2 + 1..];
                if let Some(end_s) = body.find("</") {
                    strip_tags(&body[..end_s])
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        if !title.is_empty() && !url.is_empty() {
            hits.push(SearchHit {
                title,
                url,
                snippet: snippet.chars().take(200).collect(),
            });
        }
        // Advance past the current match (rest already starts at idx).
        rest = rest.get(10..).unwrap_or("");
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_ddg_sample() {
        // Parser looks for result__snippet after result__a in the same rest window.
        let html = r#"
        <div class="result">
          <a class="result__a" href="https://example.com/a">Example Title</a>
          <td class="result__snippet">A short snippet here.</td>
        </div>
        "#;
        let hits = parse_ddg_html(html, 5);
        assert!(!hits.is_empty(), "hits empty");
        assert_eq!(hits[0].title, "Example Title");
        assert!(hits[0].url.contains("example.com"));
        assert!(hits[0].snippet.contains("short snippet"));
    }

    #[test]
    fn parse_ddg_lite_sample() {
        let html = r#"
        <a class="result-link" href="https://example.com/lite">Lite Title</a>
        "#;
        let hits = parse_ddg_lite(html, 3);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Lite Title");
        assert_eq!(hits[0].url, "https://example.com/lite");
    }

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
