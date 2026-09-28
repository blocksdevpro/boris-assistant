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
///
/// - Splits on the **earliest** of `@` / `|` (so `"a@b|c"` → model `a`,
///   provider `b|c`).
/// - A trailing separator with no provider (`"model@"`, `"model|"`) returns
///   the stripped model with no provider.
/// - The provider part is trimmed and lowercased for consistency with
///   [`parse_provider_list`]; the model id keeps its original case (model ids
///   are case-sensitive paths like `google/gemini-2.5-flash-lite`).
pub fn split_model_and_provider(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    // '@' and '|' are single-byte ASCII, so byte indices always sit on char
    // boundaries and slicing here is safe.
    let sep = match (raw.find('@'), raw.find('|')) {
        (Some(a), Some(p)) => Some(a.min(p)),
        (Some(a), None) => Some(a),
        (None, Some(p)) => Some(p),
        (None, None) => None,
    };
    if let Some(idx) = sep {
        let model = raw[..idx].trim();
        let provider = raw[idx + 1..].trim();
        if !model.is_empty() {
            if provider.is_empty() {
                return (model.to_string(), None);
            }
            return (model.to_string(), Some(provider.to_ascii_lowercase()));
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

        // Incomplete forms keep the stripped model with no provider
        let (m, p) = split_model_and_provider("model@");
        assert_eq!(m, "model");
        assert!(p.is_none());

        let (m, p) = split_model_and_provider("model|");
        assert_eq!(m, "model");
        assert!(p.is_none());
    }

    #[test]
    fn split_model_lowercases_provider_and_prefers_earliest_separator() {
        let (m, p) = split_model_and_provider("google/gemini-2.5-flash-lite@CoreWeave");
        assert_eq!(m, "google/gemini-2.5-flash-lite");
        assert_eq!(p.as_deref(), Some("coreweave"));

        let (m, p) = split_model_and_provider("openai/gpt-4o| DeepInfra/Turbo ");
        assert_eq!(m, "openai/gpt-4o");
        assert_eq!(p.as_deref(), Some("deepinfra/turbo"));

        // Earliest of '@' / '|' wins, regardless of loop order.
        let (m, p) = split_model_and_provider("a@b|c");
        assert_eq!(m, "a");
        assert_eq!(p.as_deref(), Some("b|c"));

        let (m, p) = split_model_and_provider("a|b@c");
        assert_eq!(m, "a");
        assert_eq!(p.as_deref(), Some("b@c"));
    }
}
