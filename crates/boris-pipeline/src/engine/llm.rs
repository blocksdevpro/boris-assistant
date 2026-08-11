//! OpenRouter client wiring for the voice agent plane.

use boris_agent::{OpenRouterClient, ReasoningConfig};

/// Default strong/primary model when nothing is configured.
///
/// Prefer a current, tool-capable OpenRouter chat model for research / tool turns.
pub(super) const DEFAULT_STRONG_MODEL: &str = "google/gemini-3.6-flash";

/// Heuristic: specialized apply/merge models (Morph, etc.) usually cannot tool-call.
pub(super) fn looks_like_non_agent_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.starts_with("morph/")
        || m.contains("morph-v")
        || m.contains("apply-model")
        || m.contains("/apply")
}

/// Resolve model id + optional OpenRouter model-provider preference.
///
/// Accepts `model@provider` in the model field; a separate provider arg wins when both set.
pub(super) fn resolve_model_and_provider(
    model: Option<&str>,
    provider: Option<&str>,
    default_model: &str,
) -> (String, Option<String>) {
    let raw = model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default_model);
    let (model_id, inline_provider) = boris_agent::split_model_and_provider(raw);
    let model_id = if model_id.is_empty() {
        default_model.to_string()
    } else {
        model_id
    };
    let provider = provider
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or(inline_provider);
    (model_id, provider)
}

/// Build a client with reasoning enabled.
///
/// - **strong** path → `high` effort (tools, research, plan)
/// - **fast** path → `medium` (still thinks; cheaper than high)
pub(super) fn build_openrouter_client(
    api_key: &str,
    model: &str,
    provider_pref: Option<&str>,
    pin: bool,
    session_id: &str,
    strong: bool,
) -> OpenRouterClient {
    let reasoning = if strong {
        ReasoningConfig::high()
    } else {
        ReasoningConfig::medium()
    };
    let mut client = OpenRouterClient::new(api_key.to_string(), Some(model.to_string()))
        .with_session_id(session_id)
        .with_reasoning(reasoning);
    if let Some(pref) = provider_pref {
        if !pref.trim().is_empty() {
            client = client
                .with_provider_pref(pref)
                .with_allow_fallbacks(!pin);
            tracing::info!(
                model = %model,
                provider = %pref,
                allow_fallbacks = !pin,
                "OpenRouter model-provider preference set"
            );
        }
    }
    tracing::info!(
        model = %model,
        effort = client.reasoning().effort.as_str(),
        max_tokens = client.max_tokens(),
        "OpenRouter reasoning configured"
    );
    client
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_agent_model_heuristic() {
        assert!(looks_like_non_agent_model("morph/morph-v3-large"));
        assert!(looks_like_non_agent_model("vendor/apply-model"));
        assert!(!looks_like_non_agent_model("google/gemini-3.6-flash"));
        assert!(!looks_like_non_agent_model("google/gemini-2.5-flash-lite"));
    }

    #[test]
    fn resolve_defaults_and_inline_provider() {
        let (m, p) = resolve_model_and_provider(None, None, DEFAULT_STRONG_MODEL);
        assert_eq!(m, DEFAULT_STRONG_MODEL);
        assert!(p.is_none());

        let (m, p) = resolve_model_and_provider(Some("foo@bar"), None, DEFAULT_STRONG_MODEL);
        assert_eq!(m, "foo");
        assert_eq!(p.as_deref(), Some("bar"));

        let (m, p) =
            resolve_model_and_provider(Some("foo@bar"), Some("explicit"), DEFAULT_STRONG_MODEL);
        assert_eq!(m, "foo");
        assert_eq!(p.as_deref(), Some("explicit"));
    }
}
