//! Token usage extracted from provider responses (cache-aware).

use serde_json::Value;

/// Token usage from an OpenRouter (or compatible) `usage` object.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Input tokens billed for the prompt.
    pub prompt_tokens: u64,
    /// Output tokens billed for the completion.
    pub completion_tokens: u64,
    /// Total tokens when reported; else prompt + completion.
    pub total_tokens: u64,
    /// Prompt tokens served from provider cache (`prompt_tokens_details.cached_tokens`).
    pub cached_tokens: u64,
    /// Tokens written into cache this request (when reported).
    pub cache_write_tokens: u64,
}

impl TokenUsage {
    /// Parse an OpenRouter-style `usage` JSON object.
    pub fn from_usage_value(usage: &Value) -> Self {
        let prompt_tokens = u64_field(usage, "prompt_tokens");
        let completion_tokens = u64_field(usage, "completion_tokens");
        let total_tokens = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(prompt_tokens.saturating_add(completion_tokens));

        let details = usage.get("prompt_tokens_details");
        let cached_tokens = details
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_write_tokens = details
            .and_then(|d| d.get("cache_write_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            cache_write_tokens,
        }
    }

    /// True when any prompt tokens came from cache.
    pub fn cache_hit(&self) -> bool {
        self.cached_tokens > 0
    }

    /// Whether this usage is worth logging (non-zero activity).
    pub fn is_worth_logging(&self) -> bool {
        self.total_tokens > 0 || self.cached_tokens > 0
    }
}

fn u64_field(obj: &Value, key: &str) -> u64 {
    obj.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

/// Log usage at info (cache hit) or debug (miss / cold).
pub fn log_usage(model: &str, usage: &TokenUsage, path: &str) {
    if !usage.is_worth_logging() {
        return;
    }
    if usage.cache_hit() {
        tracing::info!(
            model = %model,
            path,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            cached_tokens = usage.cached_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            "OpenRouter usage (cache hit)"
        );
    } else {
        tracing::debug!(
            model = %model,
            path,
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            cache_write_tokens = usage.cache_write_tokens,
            "OpenRouter usage"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn token_usage_parses_cached_tokens() {
        let usage = json!({
            "prompt_tokens": 1000,
            "completion_tokens": 50,
            "total_tokens": 1050,
            "prompt_tokens_details": {
                "cached_tokens": 900,
                "cache_write_tokens": 100
            }
        });
        let u = TokenUsage::from_usage_value(&usage);
        assert_eq!(u.prompt_tokens, 1000);
        assert_eq!(u.completion_tokens, 50);
        assert_eq!(u.total_tokens, 1050);
        assert_eq!(u.cached_tokens, 900);
        assert_eq!(u.cache_write_tokens, 100);
        assert!(u.cache_hit());
        assert!(u.is_worth_logging());
    }

    #[test]
    fn total_defaults_to_sum() {
        let u = TokenUsage::from_usage_value(&json!({
            "prompt_tokens": 10,
            "completion_tokens": 5
        }));
        assert_eq!(u.total_tokens, 15);
        assert!(!u.cache_hit());
    }
}
