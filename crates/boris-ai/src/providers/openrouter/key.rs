//! API key check: `GET {base}/key` says whether OpenRouter accepts the key
//! and how much credit the key has left. Used by setup / self-check so a bad
//! key shows up before the first voice turn instead of after it.

use std::time::Duration;

use serde_json::Value;

/// Whole-request budget. A setup check should fail fast when offline.
const KEY_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of [`check_key`].
#[derive(Debug, Clone, PartialEq)]
pub enum KeyCheck {
    /// Key accepted. `limit_remaining` is `None` for keys without a credit cap.
    Valid {
        label: Option<String>,
        limit_remaining: Option<f64>,
        is_free_tier: bool,
    },
    /// OpenRouter answered and rejected the key (bad, revoked, disabled).
    Rejected { status: u16, message: String },
    /// No usable answer (offline, proxy, DNS, timeout, server error).
    Unreachable { message: String },
}

/// Ask OpenRouter about `api_key`. Never panics; every failure is a variant.
pub async fn check_key(base_url: &str, api_key: &str) -> KeyCheck {
    let client = match reqwest::Client::builder()
        .timeout(KEY_CHECK_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return KeyCheck::Unreachable {
                message: format!("HTTP client: {e}"),
            }
        }
    };
    let url = format!("{}/key", base_url.trim_end_matches('/'));
    let response = match client.get(&url).bearer_auth(api_key.trim()).send().await {
        Ok(response) => response,
        Err(e) => {
            return KeyCheck::Unreachable {
                message: e.to_string(),
            }
        }
    };
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    interpret(status, &body)
}

/// Map an HTTP status + body to a [`KeyCheck`]. Pure for tests.
pub(crate) fn interpret(status: u16, body: &str) -> KeyCheck {
    let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if status == 200 {
        let data = &json["data"];
        if data.is_object() {
            return KeyCheck::Valid {
                label: data["label"].as_str().map(str::to_string),
                limit_remaining: data["limit_remaining"].as_f64(),
                is_free_tier: data["is_free_tier"].as_bool().unwrap_or(false),
            };
        }
        return KeyCheck::Unreachable {
            message: "unexpected response from OpenRouter".into(),
        };
    }
    let message = json["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("HTTP {status}"));
    if (400..500).contains(&status) && status != 408 && status != 429 {
        KeyCheck::Rejected { status, message }
    } else {
        KeyCheck::Unreachable { message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_key_reads_remaining_credit() {
        let body = r#"{"data":{"label":"sk-or-v1-abc...","limit":10,"limit_remaining":2.5,"is_free_tier":false}}"#;
        assert_eq!(
            interpret(200, body),
            KeyCheck::Valid {
                label: Some("sk-or-v1-abc...".into()),
                limit_remaining: Some(2.5),
                is_free_tier: false,
            }
        );
    }

    #[test]
    fn uncapped_key_has_no_remaining_limit() {
        let body = r#"{"data":{"label":"x","limit":null,"limit_remaining":null}}"#;
        assert!(matches!(
            interpret(200, body),
            KeyCheck::Valid {
                limit_remaining: None,
                ..
            }
        ));
    }

    #[test]
    fn bad_key_is_rejected_with_the_server_message() {
        let body = r#"{"error":{"message":"No auth credentials found","code":401}}"#;
        assert_eq!(
            interpret(401, body),
            KeyCheck::Rejected {
                status: 401,
                message: "No auth credentials found".into()
            }
        );
    }

    #[test]
    fn rate_limits_and_outages_are_not_blamed_on_the_key() {
        assert!(matches!(interpret(429, "{}"), KeyCheck::Unreachable { .. }));
        assert!(matches!(interpret(503, ""), KeyCheck::Unreachable { .. }));
        assert!(matches!(
            interpret(200, "<html>"),
            KeyCheck::Unreachable { .. }
        ));
    }
}
