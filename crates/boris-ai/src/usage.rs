//! Token usage extracted from provider responses (cache-aware).

use serde_json::Value;

/// Token usage from an OpenAI-compatible (or OpenRouter) `usage` object.
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
    /// Reasoning / thinking tokens (`completion_tokens_details.reasoning_tokens`).
    ///
    /// Additive field (defaults to 0). NOTE for hosts: struct literals outside
    /// this crate must use `..Default::default()` so this field keeps working.
    pub reasoning: u64,
    // NOTE: `usage.cost` is deliberately not stored. `TokenUsage` stays
    // `Eq`-friendly (u64-only), and `log_complete` receives no raw `Value` to
    // parse a provider-reported cost from. If cost tracking is ever needed,
    // add a fixed-point integer field (e.g. micro-dollars) rather than `f64`.
}

impl TokenUsage {
    /// Parse an OpenAI-compatible `usage` JSON object.
    pub fn from_usage_value(usage: &Value) -> Self {
        let prompt_tokens = u64_field(usage, "prompt_tokens");
        let completion_tokens = u64_field(usage, "completion_tokens");
        let total_tokens = u64_field_opt(usage, "total_tokens")
            .unwrap_or_else(|| prompt_tokens.saturating_add(completion_tokens));

        let prompt_details = usage.get("prompt_tokens_details");
        let cached_tokens = prompt_details
            .map(|d| u64_field(d, "cached_tokens"))
            .unwrap_or(0);
        let cache_write_tokens = prompt_details
            .map(|d| u64_field(d, "cache_write_tokens"))
            .unwrap_or(0);
        let reasoning = usage
            .get("completion_tokens_details")
            .map(|d| u64_field(d, "reasoning_tokens"))
            .unwrap_or(0);

        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens,
            cache_write_tokens,
            reasoning,
        }
    }

    /// True when any prompt tokens came from cache.
    pub fn cache_hit(&self) -> bool {
        self.cached_tokens > 0
    }

    /// Whether this usage is worth logging (non-zero activity).
    pub fn is_worth_logging(&self) -> bool {
        self.total_tokens > 0 || self.cached_tokens > 0 || self.reasoning > 0
    }
}

/// Coerce one JSON field to `u64`.
///
/// Providers disagree on number encoding: accept integers, floats (`1000.0`),
/// and numeric strings (`"1000"`). Missing / null / unparseable → 0.
fn u64_field(obj: &Value, key: &str) -> u64 {
    obj.get(key).map(coerce_u64).unwrap_or(0)
}

fn u64_field_opt(obj: &Value, key: &str) -> Option<u64> {
    obj.get(key)
        .filter(|v| !v.is_null())
        .map(coerce_u64_defined)
        .unwrap_or(None)
}

fn coerce_u64(v: &Value) -> u64 {
    coerce_u64_defined(v).unwrap_or(0)
}

fn coerce_u64_defined(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        // `as u64` saturates negatives to 0 and clamps overflow; NaN → 0.
        return Some(f as u64);
    }
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if let Ok(n) = s.parse::<u64>() {
            return Some(n);
        }
        // Tolerate "1000.0"-style strings.
        if let Ok(f) = s.parse::<f64>() {
            return Some(f as u64);
        }
    }
    None
}

/// Log one finished LLM completion, always including wall time.
///
/// Token fields are included when the provider reported usage. Cache hits stay
/// labeled so hosts can spot prompt-cache savings without a second line.
pub fn log_complete(model: &str, path: &str, ms: u64, usage: Option<&TokenUsage>) {
    match usage {
        // `tracing` needs static field sets, so reasoning tokens get their own
        // variants instead of a conditional field.
        Some(usage) if usage.cache_hit() && usage.reasoning > 0 => {
            tracing::info!(
                model = %model,
                path,
                ms,
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                cached_tokens = usage.cached_tokens,
                cache_write_tokens = usage.cache_write_tokens,
                reasoning_tokens = usage.reasoning,
                "LLM complete (cache hit)"
            );
        }
        Some(usage) if usage.cache_hit() => {
            tracing::info!(
                model = %model,
                path,
                ms,
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                cached_tokens = usage.cached_tokens,
                cache_write_tokens = usage.cache_write_tokens,
                "LLM complete (cache hit)"
            );
        }
        Some(usage) if usage.is_worth_logging() && usage.reasoning > 0 => {
            tracing::info!(
                model = %model,
                path,
                ms,
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                cache_write_tokens = usage.cache_write_tokens,
                reasoning_tokens = usage.reasoning,
                "LLM complete"
            );
        }
        Some(usage) if usage.is_worth_logging() => {
            tracing::info!(
                model = %model,
                path,
                ms,
                prompt_tokens = usage.prompt_tokens,
                completion_tokens = usage.completion_tokens,
                cache_write_tokens = usage.cache_write_tokens,
                "LLM complete"
            );
        }
        _ => {
            tracing::info!(model = %model, path, ms, "LLM complete");
        }
    }
}

/// Log a failed LLM completion with the time spent before the error.
pub fn log_complete_failed(model: &str, path: &str, ms: u64, error: &dyn std::fmt::Display) {
    tracing::warn!(model = %model, path, ms, error = %error, "LLM complete failed");
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

    #[test]
    fn coerce_float_string_and_null_fields() {
        let u = TokenUsage::from_usage_value(&json!({
            "prompt_tokens": "1000",
            "completion_tokens": 50.0,
            "total_tokens": null,
            "prompt_tokens_details": {
                "cached_tokens": "900",
                "cache_write_tokens": 100.0
            }
        }));
        assert_eq!(u.prompt_tokens, 1000);
        assert_eq!(u.completion_tokens, 50);
        // Explicit null total still falls back to the sum.
        assert_eq!(u.total_tokens, 1050);
        assert_eq!(u.cached_tokens, 900);
        assert_eq!(u.cache_write_tokens, 100);
    }

    #[test]
    fn unparseable_fields_default_to_zero() {
        let u = TokenUsage::from_usage_value(&json!({
            "prompt_tokens": "n/a",
            "completion_tokens": [1],
            "prompt_tokens_details": { "cached_tokens": {} }
        }));
        assert_eq!(u.prompt_tokens, 0);
        assert_eq!(u.completion_tokens, 0);
        assert_eq!(u.total_tokens, 0);
        assert_eq!(u.cached_tokens, 0);
        assert!(!u.is_worth_logging());
    }

    #[test]
    fn captures_reasoning_tokens() {
        let u = TokenUsage::from_usage_value(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 60,
            "total_tokens": 160,
            "completion_tokens_details": { "reasoning_tokens": 40 }
        }));
        assert_eq!(u.reasoning, 40);
        assert!(u.is_worth_logging());
        // String-encoded reasoning also coerces.
        let u = TokenUsage::from_usage_value(&json!({
            "completion_tokens_details": { "reasoning_tokens": "25" }
        }));
        assert_eq!(u.reasoning, 25);
        assert!(u.is_worth_logging());
        // Absent details default to zero.
        let u = TokenUsage::from_usage_value(&json!({ "prompt_tokens": 1 }));
        assert_eq!(u.reasoning, 0);
    }
}
