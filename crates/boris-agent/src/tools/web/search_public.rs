//! No-account search backends: DuckDuckGo HTML, Instant Answer, Wikipedia.
//!
//! These are the default `web_search` path so a downloaded build works without
//! an Exa (or any other) API key. Official JSON APIs are preferred when the
//! HTML scrape is blocked.

use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

use super::encode::{extract_uddg, urlencoding_encode};
use super::html::plain_from_html;
use super::search::SearchHit;
use super::MAX_SEARCH;

const PER_REQUEST: Duration = Duration::from_secs(10);
const IA_SNIPPET_CHARS: usize = 400;
const WIKI_SNIPPET_CHARS: usize = 280;

/// Run the no-key backend chain until `limit` unique hits are collected.
///
/// Order: DDG Instant Answer lead (official) → DDG HTML/lite web results →
/// Instant Answer related topics → Wikipedia search (official).
pub(crate) async fn search_public(
    official: &Client,
    browser: &Client,
    query: &str,
    limit: usize,
) -> Vec<SearchHit> {
    let limit = limit.clamp(1, MAX_SEARCH);
    let mut hits = Vec::new();

    let ia = match search_ddg_instant(official, query).await {
        Ok(ia) => ia,
        Err(e) => {
            tracing::debug!(error = %e, %query, "DDG instant answer failed");
            InstantHits::default()
        }
    };
    merge_hits(&mut hits, ia.lead, limit);

    let web = search_ddg_html(browser, query, limit).await;
    if web.is_empty() {
        tracing::debug!(%query, "DDG HTML/lite returned no usable hits");
    }
    merge_hits(&mut hits, web, limit);

    if hits.len() < limit {
        merge_hits(&mut hits, ia.more, limit);
    }

    if hits.len() < limit {
        match search_wikipedia(official, query, limit - hits.len()).await {
            Ok(wiki) => merge_hits(&mut hits, wiki, limit),
            Err(e) => tracing::debug!(error = %e, %query, "Wikipedia search failed"),
        }
    }

    hits
}

#[derive(Debug, Default)]
pub(crate) struct InstantHits {
    /// Abstract / direct answer — put first when present.
    lead: Vec<SearchHit>,
    /// Related topics and "results" links.
    more: Vec<SearchHit>,
}

async fn search_ddg_instant(client: &Client, query: &str) -> Result<InstantHits, String> {
    let q = urlencoding_encode(query);
    let url = format!(
        "https://api.duckduckgo.com/?q={q}&format=json&no_html=1&skip_disambig=1&t=boris-assistant"
    );
    let resp = client
        .get(&url)
        .timeout(PER_REQUEST)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))?;
    Ok(parse_ddg_instant(&json, MAX_SEARCH))
}

async fn search_wikipedia(
    client: &Client,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, String> {
    let q = urlencoding_encode(query);
    let url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={q}&srlimit={limit}&srprop=snippet&format=json&utf8=1"
    );
    let resp = client
        .get(&url)
        .timeout(PER_REQUEST)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))?;
    Ok(parse_wikipedia_search(&json, limit))
}

async fn search_ddg_html(client: &Client, query: &str, limit: usize) -> Vec<SearchHit> {
    let q = urlencoding_encode(query);
    let form = format!("q={q}&b=&l=us-en");

    // POST first (what working DDG HTML clients use), then GET html, then lite.
    let attempts: [DdgAttempt<'_>; 3] = [
        DdgAttempt::Post {
            url: "https://html.duckduckgo.com/html/",
            body: &form,
        },
        DdgAttempt::Get(format!("https://html.duckduckgo.com/html/?q={q}")),
        DdgAttempt::Get(format!("https://lite.duckduckgo.com/lite/?q={q}")),
    ];

    for attempt in attempts {
        let resp = match attempt.send(client).await {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(error = %e, "DDG request failed");
                continue;
            }
        };
        let status = resp.status();
        if matches!(status.as_u16(), 202 | 403 | 429) {
            tracing::debug!(status = %status, "DDG blocked/rate-limited; not retrying other DDG endpoints");
            break;
        }
        if !status.is_success() {
            tracing::debug!(status = %status, "DDG HTTP non-success");
            continue;
        }
        let html = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(error = %e, "DDG body read failed");
                continue;
            }
        };
        if looks_like_block_page(&html) {
            tracing::debug!("DDG returned a block/anomaly page; not retrying other DDG endpoints");
            break;
        }
        let mut results = parse_ddg_html(&html, limit);
        if results.is_empty() {
            results = parse_ddg_lite(&html, limit);
        }
        results.retain(|h| !is_search_ad(&h.url));
        if !results.is_empty() {
            return results;
        }
    }
    Vec::new()
}

enum DdgAttempt<'a> {
    Post { url: &'a str, body: &'a str },
    Get(String),
}

impl DdgAttempt<'_> {
    async fn send(&self, client: &Client) -> Result<reqwest::Response, reqwest::Error> {
        match self {
            DdgAttempt::Post { url, body } => {
                client
                    .post(*url)
                    .timeout(PER_REQUEST)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .header("referer", "https://html.duckduckgo.com/")
                    .body((*body).to_string())
                    .send()
                    .await
            }
            DdgAttempt::Get(url) => client.get(url).timeout(PER_REQUEST).send().await,
        }
    }
}

/// Parse DuckDuckGo Instant Answer JSON into lead + related hits (pure).
pub(crate) fn parse_ddg_instant(json: &Value, limit: usize) -> InstantHits {
    let mut lead = Vec::new();
    let mut more = Vec::new();

    let heading = json
        .get("Heading")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let abstract_text = json
        .get("AbstractText")
        .or_else(|| json.get("Abstract"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let abstract_url = json
        .get("AbstractURL")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !abstract_text.is_empty() {
        let title = if heading.is_empty() {
            json.get("AbstractSource")
                .and_then(|v| v.as_str())
                .unwrap_or("DuckDuckGo")
                .to_string()
        } else {
            heading.to_string()
        };
        let url = if abstract_url.is_empty() {
            String::new()
        } else {
            abstract_url.to_string()
        };
        lead.push(SearchHit {
            title,
            url,
            snippet: abstract_text.chars().take(IA_SNIPPET_CHARS).collect(),
        });
    }

    let answer = json
        .get("Answer")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !answer.is_empty() {
        let answer_url = json
            .get("AnswerURL")
            .or_else(|| json.get("AbstractURL"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        lead.push(SearchHit {
            title: if heading.is_empty() {
                "Instant answer".into()
            } else {
                heading.to_string()
            },
            url: answer_url,
            snippet: answer.chars().take(IA_SNIPPET_CHARS).collect(),
        });
    }

    collect_ddg_topic_hits(json.get("Results"), &mut more, limit);
    collect_ddg_topic_hits(json.get("RelatedTopics"), &mut more, limit);

    InstantHits { lead, more }
}

fn collect_ddg_topic_hits(node: Option<&Value>, out: &mut Vec<SearchHit>, limit: usize) {
    let Some(arr) = node.and_then(|v| v.as_array()) else {
        return;
    };
    for item in arr {
        if out.len() >= limit {
            return;
        }
        if let Some(nested) = item.get("Topics").and_then(|t| t.as_array()) {
            for sub in nested {
                if out.len() >= limit {
                    return;
                }
                if let Some(hit) = topic_hit(sub) {
                    out.push(hit);
                }
            }
            continue;
        }
        if let Some(hit) = topic_hit(item) {
            out.push(hit);
        }
    }
}

fn topic_hit(item: &Value) -> Option<SearchHit> {
    let url = item.get("FirstURL")?.as_str()?.trim();
    if url.is_empty() {
        return None;
    }
    let text = item
        .get("Text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim();
    let title = text.split(" - ").next().unwrap_or(text).trim();
    if title.is_empty() {
        return None;
    }
    Some(SearchHit {
        title: title.to_string(),
        url: url.to_string(),
        snippet: text.chars().take(200).collect(),
    })
}

/// Parse Wikipedia `action=query&list=search` JSON (pure).
pub(crate) fn parse_wikipedia_search(json: &Value, limit: usize) -> Vec<SearchHit> {
    let Some(arr) = json.pointer("/query/search").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for item in arr.iter().take(limit) {
        let title = item
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            continue;
        }
        let snippet = item.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
        hits.push(SearchHit {
            title: title.to_string(),
            url: wikipedia_title_url(title),
            snippet: plain_from_html(snippet)
                .chars()
                .take(WIKI_SNIPPET_CHARS)
                .collect(),
        });
    }
    hits
}

pub(crate) fn wikipedia_title_url(title: &str) -> String {
    let mut out = String::from("https://en.wikipedia.org/wiki/");
    for c in title.chars() {
        match c {
            ' ' => out.push('_'),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '(' | ')' | ',' | ':' | '/' => {
                out.push(c)
            }
            _ => {
                let mut buf = [0u8; 4];
                for b in c.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}

fn merge_hits(into: &mut Vec<SearchHit>, extra: Vec<SearchHit>, limit: usize) {
    for hit in extra {
        if into.len() >= limit {
            return;
        }
        if hit.title.trim().is_empty() && hit.url.trim().is_empty() {
            continue;
        }
        if is_search_ad(&hit.url) {
            continue;
        }
        let key = normalize_url(&hit.url);
        let dup = if key.is_empty() {
            into.iter()
                .any(|h| h.title.eq_ignore_ascii_case(&hit.title))
        } else {
            into.iter().any(|h| normalize_url(&h.url) == key)
        };
        if !dup {
            into.push(hit);
        }
    }
}

fn normalize_url(url: &str) -> String {
    let mut u = url.trim().to_string();
    if u.starts_with("//") {
        u = format!("https:{u}");
    }
    if let Some(rest) = u.strip_prefix("https://") {
        u = format!("https://{}", rest.trim_start_matches("www."));
    } else if let Some(rest) = u.strip_prefix("http://") {
        u = format!("http://{}", rest.trim_start_matches("www."));
    }
    if u.ends_with('/') && u.matches('/').count() > 2 {
        u.pop();
    }
    u
}

fn is_search_ad(url: &str) -> bool {
    url.contains("duckduckgo.com/y.js")
}

fn looks_like_block_page(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let has_results = lower.contains("result__a") || lower.contains("result-link");
    if has_results {
        return false;
    }
    lower.contains("anomaly")
        || lower.contains("unusual traffic")
        || lower.contains("captcha")
        || lower.contains("enable javascript")
        || lower.contains("error-lite+")
        || lower.contains("error getting results")
}

/// DuckDuckGo lite result table scraper.
pub(crate) fn parse_ddg_lite(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut rest = html;
    while hits.len() < limit {
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
        let title = plain_from_html(&title_start[..end_title]);
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
        rest = rest.get(10..).unwrap_or("");
    }
    hits
}

/// Very small HTML scraper for DDG result blocks.
pub(crate) fn parse_ddg_html(html: &str, limit: usize) -> Vec<SearchHit> {
    let mut hits = Vec::new();
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
        let title = plain_from_html(&title_start[..end_title]);

        let snippet = if let Some(s_i) = rest.find("result__snippet") {
            let s_rest = &rest[s_i..];
            if let Some(gt2) = s_rest.find('>') {
                let body = &s_rest[gt2 + 1..];
                if let Some(end_s) = body.find("</") {
                    plain_from_html(&body[..end_s])
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
            if url.starts_with("//") {
                url = format!("https:{url}");
            }
            hits.push(SearchHit {
                title,
                url,
                snippet: snippet.chars().take(200).collect(),
            });
        }
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
    fn parse_ddg_html_unwraps_uddg_and_amp() {
        let html = r#"
        <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust%2Dlang.org%2F&amp;rut=abc">Rust</a>
        <a class="result__snippet">The language.</a>
        "#;
        let hits = parse_ddg_html(html, 2);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://rust-lang.org/");
    }

    #[test]
    fn parse_wikipedia_sample() {
        let json = json!({
            "query": {
                "search": [
                    {
                        "title": "Rust (programming language)",
                        "snippet": "<span class=\"searchmatch\">Rust</span> is a general-purpose language"
                    }
                ]
            }
        });
        let hits = parse_wikipedia_search(&json, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Rust (programming language)");
        assert_eq!(
            hits[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert!(hits[0].snippet.contains("general-purpose"));
        assert!(!hits[0].snippet.contains("<span"));
    }

    #[test]
    fn parse_ddg_instant_sample() {
        let json = json!({
            "Heading": "Rust (programming language)",
            "AbstractText": "Rust is a general-purpose programming language.",
            "AbstractURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "RelatedTopics": [
                {
                    "Text": "Cargo - Rust package manager",
                    "FirstURL": "https://en.wikipedia.org/wiki/Cargo_(software)"
                },
                {
                    "Name": "See also",
                    "Topics": [
                        {
                            "Text": "Go - Another language",
                            "FirstURL": "https://en.wikipedia.org/wiki/Go_(programming_language)"
                        }
                    ]
                }
            ]
        });
        let ia = parse_ddg_instant(&json, 8);
        assert_eq!(ia.lead.len(), 1);
        assert!(ia.lead[0].snippet.contains("general-purpose"));
        assert_eq!(ia.more.len(), 2);
        assert_eq!(ia.more[0].title, "Cargo");
        assert_eq!(ia.more[1].title, "Go");
    }

    #[test]
    fn merge_dedupes_www_and_slash() {
        let mut hits = vec![SearchHit {
            title: "A".into(),
            url: "https://www.example.com/path/".into(),
            snippet: String::new(),
        }];
        merge_hits(
            &mut hits,
            vec![SearchHit {
                title: "B".into(),
                url: "https://example.com/path".into(),
                snippet: String::new(),
            }],
            8,
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn block_page_detection() {
        assert!(looks_like_block_page(
            "<html>anomaly detected please enable javascript</html>"
        ));
        assert!(looks_like_block_page(
            r#"If this persists, please <a href="mailto:error-lite+9318@duckduckgo.com?subject=Error getting results">email us</a>."#
        ));
        assert!(!looks_like_block_page(
            r#"<a class="result__a" href="https://x">X</a>"#
        ));
    }

    #[tokio::test]
    #[ignore = "hits the live web; run with --ignored when changing search backends"]
    async fn live_public_search_returns_hits() {
        let official = crate::tools::web::search_api_client().expect("official client");
        let browser = crate::tools::web::browser_search_client().expect("browser client");
        let hits = search_public(&official, &browser, "rust programming language", 5).await;
        assert!(
            !hits.is_empty(),
            "public search returned no hits (DDG + Wikipedia)"
        );
        assert!(hits.iter().any(|h| !h.url.is_empty()));
    }

    /// Public backends only (DDG IA / DDG HTML / Wikipedia). Never Exa.
    #[tokio::test]
    #[ignore = "live battery; do not run in CI"]
    async fn live_public_search_battery() {
        let official = crate::tools::web::search_api_client().expect("official client");
        let browser = crate::tools::web::browser_search_client().expect("browser client");

        struct Case {
            kind: &'static str,
            query: &'static str,
            must: bool,
        }
        let cases = [
            Case {
                kind: "encyclopedia",
                query: "rust programming language",
                must: true,
            },
            Case {
                kind: "fact",
                query: "who invented the telephone",
                must: true,
            },
            Case {
                kind: "definition",
                query: "define serendipity",
                must: true,
            },
            Case {
                kind: "calc",
                query: "what is 15% of 240",
                must: false,
            },
            Case {
                kind: "weather",
                query: "weather in Bengaluru",
                must: false,
            },
            Case {
                kind: "fx",
                query: "USD to INR",
                must: false,
            },
            Case {
                kind: "date",
                query: "when is Diwali 2026",
                must: true,
            },
            Case {
                kind: "news",
                query: "latest news India",
                must: true,
            },
            Case {
                kind: "sports",
                query: "Premier League table",
                must: true,
            },
            Case {
                kind: "person",
                query: "Sundar Pichai",
                must: true,
            },
            Case {
                kind: "person-hard",
                query: "Sundar Pichai LinkedIn",
                must: false,
            },
            Case {
                kind: "tech",
                query: "tokio spawn rust",
                must: true,
            },
            Case {
                kind: "error",
                query: "error E0308 mismatched types rust",
                must: false,
            },
            Case {
                kind: "site",
                query: "site:github.com rust async tutorial",
                must: false,
            },
            Case {
                kind: "product",
                query: "best wireless earbuds 2026",
                must: true,
            },
            Case {
                kind: "local",
                query: "best pizza in Austin Texas",
                must: true,
            },
            Case {
                kind: "howto",
                query: "how to reset windows 11 password",
                must: true,
            },
            Case {
                kind: "science",
                query: "James Webb telescope latest discoveries",
                must: true,
            },
            Case {
                kind: "entertainment",
                query: "Oppenheimer cast",
                must: true,
            },
            Case {
                kind: "spanish",
                query: "qué es la fotosíntesis",
                must: true,
            },
            Case {
                kind: "typo",
                query: "pythn list comprehnsion",
                must: false,
            },
            Case {
                kind: "short",
                query: "NASA",
                must: true,
            },
            Case {
                kind: "ambiguous",
                query: "apple",
                must: true,
            },
            Case {
                kind: "code",
                query: "fn main rust hello world",
                must: true,
            },
            Case {
                kind: "url-query",
                query: "https://rust-lang.org",
                must: false,
            },
            Case {
                kind: "current-fact",
                query: "prime minister of India",
                must: true,
            },
            Case {
                kind: "market",
                query: "bitcoin price",
                must: false,
            },
            Case {
                kind: "garbage",
                query: "????",
                must: false,
            },
            Case {
                kind: "compare",
                query: "C++ vs Rust performance",
                must: true,
            },
            Case {
                kind: "quoted",
                query: "\"OpenRouter API\" rate limits",
                must: false,
            },
            Case {
                kind: "stat",
                query: "how many people live in Tokyo",
                must: true,
            },
            Case {
                kind: "history",
                query: "when did the Berlin Wall fall",
                must: true,
            },
        ];

        println!(
            "\n{:<16} {:>5} {:>5} {:>5} {:>5} {:>6}  query",
            "kind", "ia", "html", "wiki", "tot", "ms"
        );
        println!("{}", "-".repeat(96));

        let mut empty_must = Vec::new();
        let mut empty_any = 0usize;
        let mut html_empty = 0usize;
        let mut ia_lead = 0usize;
        let mut total_ms = 0u128;
        let mut broken_urls = 0usize;
        let mut ddg_web_hits = 0usize;

        for (i, case) in cases.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
            let t0 = std::time::Instant::now();
            let ia = search_ddg_instant(&official, case.query)
                .await
                .unwrap_or_default();
            let html = search_ddg_html(&browser, case.query, 5).await;
            let wiki = search_wikipedia(&official, case.query, 5)
                .await
                .unwrap_or_default();
            let ms = t0.elapsed().as_millis();
            total_ms += ms;

            let mut merged = Vec::new();
            merge_hits(&mut merged, ia.lead.clone(), 5);
            merge_hits(&mut merged, html.clone(), 5);
            merge_hits(&mut merged, ia.more.clone(), 5);
            merge_hits(&mut merged, wiki.clone(), 5);

            if !ia.lead.is_empty() {
                ia_lead += 1;
            }
            if html.is_empty() {
                html_empty += 1;
            } else {
                ddg_web_hits += html.len();
            }
            if merged.is_empty() {
                empty_any += 1;
                if case.must {
                    empty_must.push(case.query);
                }
            }
            for h in &merged {
                if !h.url.is_empty()
                    && !h.url.starts_with("http://")
                    && !h.url.starts_with("https://")
                {
                    broken_urls += 1;
                }
            }

            let top = merged.first().map(|h| {
                let snip: String = h.snippet.chars().take(80).collect();
                format!("{} | {} | {}", h.title, h.url, snip)
            });
            println!(
                "{:<16} {:>5} {:>5} {:>5} {:>5} {:>6}  {}",
                case.kind,
                ia.lead.len() + ia.more.len(),
                html.len(),
                wiki.len(),
                merged.len(),
                ms,
                case.query
            );
            if let Some(top) = top {
                println!("                 top: {top}");
            } else {
                println!("                 top: (none)");
            }
        }

        let n = cases.len();
        let must_n = cases.iter().filter(|c| c.must).count();
        println!("{}", "-".repeat(96));
        println!(
            "queries={n} must={must_n} empty_must={} empty_any={empty_any} html_empty={html_empty} ia_lead={ia_lead} ddg_html_hits={ddg_web_hits} broken_urls={broken_urls} avg_ms={}",
            empty_must.len(),
            total_ms / n as u128
        );
        if !empty_must.is_empty() {
            println!("MUST-HAVE empty: {empty_must:?}");
        }
        assert!(
            empty_must.is_empty(),
            "expected hits for straightforward queries, empty: {empty_must:?}"
        );
        assert!(
            html_empty < n / 2,
            "DuckDuckGo HTML empty for {html_empty}/{n} queries — likely blocked"
        );
    }
}
