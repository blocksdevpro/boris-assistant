//! User-facing model / provider preference parsing.
//!
//! OpenRouter distinguishes:
//! - **model id** — e.g. `google/gemini-2.5-flash-lite`
//! - **model-provider** — inference host slug (`coreweave`, `baseten`, …),
//!   not the model author (Google/OpenAI)

/// Split a free-form provider preference into OpenRouter slugs.
///
/// Accepts comma and/or whitespace separated lists:
/// `coreweave, baseten` → `["coreweave", "baseten"]`.
/// Empty / whitespace → empty vec (OpenRouter default routing).
pub fn parse_provider_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// Optional split of `model@provider` / `model|provider` into `(model, provider_pref)`.
///
/// Provider-only fields in settings take precedence when both are set; this is a
/// convenience so a single string can carry both.
pub fn split_model_and_provider(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    for sep in ['@', '|'] {
        if let Some((model, provider)) = raw.split_once(sep) {
            let model = model.trim();
            let provider = provider.trim();
            if !model.is_empty() && !provider.is_empty() {
                return (model.to_string(), Some(provider.to_string()));
            }
        }
    }
    (raw.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_list_comma_and_space() {
        assert_eq!(
            parse_provider_list("coreweave, Baseten  siliconflow"),
            vec![
                "coreweave".to_string(),
                "baseten".to_string(),
                "siliconflow".to_string()
            ]
        );
        assert!(parse_provider_list("  ").is_empty());
        assert!(parse_provider_list("").is_empty());
    }

    #[test]
    fn split_model_at_provider() {
        let (m, p) = split_model_and_provider("google/gemini-2.5-flash-lite@coreweave");
        assert_eq!(m, "google/gemini-2.5-flash-lite");
        assert_eq!(p.as_deref(), Some("coreweave"));

        let (m, p) = split_model_and_provider("openai/gpt-4o|deepinfra/turbo");
        assert_eq!(m, "openai/gpt-4o");
        assert_eq!(p.as_deref(), Some("deepinfra/turbo"));

        let (m, p) = split_model_and_provider("google/gemini-2.5-flash-lite");
        assert_eq!(m, "google/gemini-2.5-flash-lite");
        assert!(p.is_none());

        let (m, p) = split_model_and_provider("  ");
        assert!(m.is_empty());
        assert!(p.is_none());

        // Incomplete forms stay as model-only
        let (m, p) = split_model_and_provider("model@");
        assert_eq!(m, "model@");
        assert!(p.is_none());
    }
}
