//! `web_search` tool — DuckDuckGo HTML lite / HTML endpoint scraping.
//!
//! Best-effort: markup may change. Prefer pure parsers (`parse_ddg_*`) for tests.

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

/// Best-effort web search (DuckDuckGo HTML lite). May break if DDG changes markup.
#[derive(Debug, Clone)]
pub struct WebSearchTool {
    client: Client,
}

impl WebSearchTool {
    pub fn new() -> Result<Self, ToolError> {
        Ok(Self {
            client: http_client()?,
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
         Prefer this over guessing URLs. Summarize for speech; do not read every result aloud."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": {
                    "type": "number",
                    "description": "Max results (default 5, max 8)"
                }
            },
            "required": ["query"]
        })
    }

    fn meta(&self) -> ToolMeta {
        // Network read — safe to fan out with other lookups in the parallel read wave.
        ToolMeta::with_risk(ToolRisk::Moderate)
            .kind(ToolKind::Web)
            .permissions(&[Permission::Network])
            .timeout(Duration::from_secs(30))
            .read_only(true)
            .max_concurrency(6)
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

        // Prefer DDG lite (more stable markup), fall back to HTML endpoint.
        let q = urlencoding_encode(query.trim());
        let mut results = Vec::new();
        for endpoint in [
            format!("https://lite.duckduckgo.com/lite/?q={q}"),
            format!("https://html.duckduckgo.com/html/?q={q}"),
        ] {
            let resp = match self.client.get(&endpoint).send().await {
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

        if results.is_empty() {
            return Ok(truncate_tool_result(format!(
                "No search results for: {query} (search backends returned empty — try a simpler query)"
            )));
        }

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
        Ok(truncate_tool_result(out))
    }
}

/// Parse `limit` from tool args: default 5, clamped to `[1, MAX_SEARCH]`.
pub(crate) fn parse_search_limit(v: Option<&Value>) -> usize {
    v.and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(5)
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
        rest = &rest[idx + 10..];
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
        assert_eq!(parse_search_limit(None), 5);
        assert_eq!(parse_search_limit(Some(&json!(3))), 3);
        assert_eq!(parse_search_limit(Some(&json!(0))), 1);
        assert_eq!(parse_search_limit(Some(&json!(99))), MAX_SEARCH);
        assert_eq!(parse_search_limit(Some(&json!("nope"))), 5);
    }

    #[test]
    fn tool_name_stable() {
        let t = WebSearchTool::default();
        assert_eq!(t.name(), "web_search");
    }
}
