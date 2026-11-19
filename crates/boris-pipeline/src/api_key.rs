//! OpenRouter key check for setup and the self-check UI.

use boris_agent::{check_openrouter_key, OpenRouterKeyCheck};
use serde::{Deserialize, Serialize};

/// Verdict on the configured OpenRouter key (wire form is snake_case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyState {
    /// Accepted, with credit (or no credit cap).
    Ok,
    /// Accepted, but its credit cap is used up.
    NoCredit,
    /// OpenRouter refused the key (bad, revoked, disabled).
    Rejected,
    /// No usable answer (offline, proxy, timeout, outage).
    Unreachable,
    /// No key entered.
    Missing,
}

/// What the UI shows for the key: a stable [`ApiKeyState`] plus a
/// human-readable `detail`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiKeyStatus {
    pub state: ApiKeyState,
    pub detail: String,
}

impl ApiKeyStatus {
    fn new(state: ApiKeyState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.state == ApiKeyState::Ok
    }
}

/// Ask OpenRouter whether `api_key` works and still has credit.
pub async fn check_api_key(api_key: &str) -> ApiKeyStatus {
    if api_key.trim().is_empty() {
        return ApiKeyStatus::new(ApiKeyState::Missing, "No OpenRouter key yet");
    }
    let result = check_openrouter_key(boris_agent::OPENROUTER_BASE_URL, api_key).await;
    let status = from_check(result);
    tracing::info!(state = ?status.state, detail = %status.detail, "api key check");
    status
}

fn from_check(check: OpenRouterKeyCheck) -> ApiKeyStatus {
    match check {
        OpenRouterKeyCheck::Valid {
            limit_remaining: Some(left),
            ..
        } if left <= 0.0 => ApiKeyStatus::new(
            ApiKeyState::NoCredit,
            "The key works but has no credit left. Add credit on openrouter.ai.",
        ),
        OpenRouterKeyCheck::Valid {
            limit_remaining, ..
        } => ApiKeyStatus::new(
            ApiKeyState::Ok,
            match limit_remaining {
                Some(left) => format!("Key works, ${left:.2} left on this key"),
                None => "Key works".to_string(),
            },
        ),
        OpenRouterKeyCheck::Rejected { status, message } => ApiKeyStatus::new(
            ApiKeyState::Rejected,
            format!("OpenRouter rejected this key ({status}): {message}"),
        ),
        OpenRouterKeyCheck::Unreachable { message } => ApiKeyStatus::new(
            ApiKeyState::Unreachable,
            format!("Could not reach OpenRouter: {message}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_credit_is_its_own_state() {
        let s = from_check(OpenRouterKeyCheck::Valid {
            label: None,
            limit_remaining: Some(0.0),
            is_free_tier: false,
        });
        assert_eq!(s.state, ApiKeyState::NoCredit);
    }

    #[test]
    fn uncapped_valid_key_is_ok() {
        let s = from_check(OpenRouterKeyCheck::Valid {
            label: None,
            limit_remaining: None,
            is_free_tier: false,
        });
        assert!(s.is_ok());
    }

    #[test]
    fn rejected_key_names_the_reason() {
        let s = from_check(OpenRouterKeyCheck::Rejected {
            status: 401,
            message: "User not found.".into(),
        });
        assert_eq!(s.state, ApiKeyState::Rejected);
        assert!(s.detail.contains("User not found."));
    }

    #[test]
    fn states_keep_their_wire_names() {
        let s = ApiKeyStatus::new(ApiKeyState::NoCredit, "x");
        let wire = serde_json::to_value(&s).unwrap();
        assert_eq!(wire["state"], "no_credit");
    }

    #[tokio::test]
    async fn blank_key_is_missing_without_a_network_call() {
        assert_eq!(check_api_key("  ").await.state, ApiKeyState::Missing);
    }
}
