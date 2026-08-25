//! Side-channel LLM extraction (does not mutate conversation context).

use serde::Deserialize;
use serde_json::{json, Value};

use crate::memory::profile::{now_ms, FactCategory, UserFact};
use boris_ai::{LlmClient, LlmError};

use super::delta::ProfileDelta;
use super::heuristic::looks_like_name;

/// Side-channel LLM extraction. Does **not** mutate conversation context.
pub async fn extract_with_llm(
    client: &dyn LlmClient,
    user_text: &str,
    assistant_text: &str,
    profile_summary: &str,
) -> Result<ProfileDelta, LlmError> {
    let system = r#"You extract durable personal context about the HUMAN user for a voice assistant.
Return ONLY a JSON object (no markdown) with this shape:
{
  "preferred_name": string|null,
  "address_as": string|null,
  "preferences_add": string[],
  "facts_add": [{"text": string, "category": "identity|preference|project|relationship|habit|other", "memory_key": string|null, "expires_in_days": number|null}],
  "facts_remove_query": string[],
  "forget_preferred_name": boolean,
  "forget_all": boolean,
  "ongoing_add": string[],
  "ongoing_replace": string[]|null
}
Rules:
- Only durable facts (name, prefs, projects, people, habits). Not one-off chit-chat.
- Prefer short factual phrases.
- If nothing new, return empty arrays and nulls.
- For corrections/contradictions, use the same stable memory_key as the old value (examples: preferred_editor, home_city). Put the stale value in facts_remove_query only when the user explicitly asks to forget it.
- Set expires_in_days for temporary facts; omit/null means durable.
- Forgetting must reflect an explicit human request. Never infer it from silence.
- Do not invent. Do not quote the assistant persona as user facts.
- Max 5 facts_add, max 5 preferences_add."#;

    let user = format!(
        "Existing profile summary:\n{profile_summary}\n\nUser said:\n{user_text}\n\nAssistant replied:\n{assistant_text}\n\nJSON:"
    );

    let messages = json!([
        {"role": "system", "content": system},
        {"role": "user", "content": user},
    ]);

    let response = client.complete(messages, Value::Null).await?;
    let content = response
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim();
    parse_llm_delta(content)
}

pub(super) fn parse_llm_delta(content: &str) -> Result<ProfileDelta, LlmError> {
    let json_str = extract_json_object(content)
        .ok_or_else(|| LlmError::parse("personal extract: no JSON object in response"))?;
    let raw: LlmDeltaRaw = serde_json::from_str(json_str)
        .map_err(|e| LlmError::parse(format!("personal extract parse: {e}")))?;

    let mut delta = ProfileDelta {
        preferred_name: raw
            .preferred_name
            .filter(|s| !s.trim().is_empty() && looks_like_name(s)),
        address_as: raw.address_as.filter(|s| !s.trim().is_empty()),
        preferences_add: raw
            .preferences_add
            .into_iter()
            .filter(|s| s.len() >= 3)
            .take(5)
            .collect(),
        facts_remove_query: raw.facts_remove_query,
        forget_preferred_name: raw.forget_preferred_name,
        forget_all: raw.forget_all,
        ongoing_add: raw.ongoing_add.into_iter().take(5).collect(),
        ongoing_replace: raw
            .ongoing_replace
            .map(|v| v.into_iter().take(10).collect()),
        ..Default::default()
    };
    for f in raw.facts_add.into_iter().take(5) {
        let text = f.text.trim();
        if text.len() < 3 {
            continue;
        }
        let mut fact = UserFact::new(text, FactCategory::parse(&f.category), "llm_extract");
        fact.confidence = 0.65;
        if let Some(key) = f.memory_key.filter(|key| !key.trim().is_empty()) {
            fact = fact.with_memory_key(key);
        }
        if let Some(days) = f.expires_in_days.filter(|days| *days > 0) {
            const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
            fact.expires_at_ms =
                Some(now_ms().saturating_add(days.min(3_650).saturating_mul(DAY_MS)));
        }
        delta.facts_add.push(fact);
    }
    Ok(delta)
}

#[derive(Debug, Deserialize)]
struct LlmDeltaRaw {
    #[serde(default)]
    preferred_name: Option<String>,
    #[serde(default)]
    address_as: Option<String>,
    #[serde(default)]
    preferences_add: Vec<String>,
    #[serde(default)]
    facts_add: Vec<LlmFactRaw>,
    #[serde(default)]
    facts_remove_query: Vec<String>,
    #[serde(default)]
    forget_preferred_name: bool,
    #[serde(default)]
    forget_all: bool,
    #[serde(default)]
    ongoing_add: Vec<String>,
    #[serde(default)]
    ongoing_replace: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct LlmFactRaw {
    text: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    memory_key: Option<String>,
    #[serde(default)]
    expires_in_days: Option<u64>,
}

/// Slice the outermost `{ … }` span (tolerant of markdown fences).
pub(super) fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&s[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_llm_json() {
        let raw = r#"{"preferred_name":"Sam","preferences_add":["hates long lectures"],"facts_add":[{"text":"Builds robots","category":"project"}],"facts_remove_query":[],"ongoing_add":[],"ongoing_replace":null,"address_as":null}"#;
        let d = parse_llm_delta(raw).unwrap();
        assert_eq!(d.preferred_name.as_deref(), Some("Sam"));
        assert_eq!(d.facts_add.len(), 1);
    }

    #[test]
    fn extract_json_from_fenced() {
        let s = "Here you go:\n```json\n{\"a\":1}\n```\n";
        assert_eq!(extract_json_object(s), Some("{\"a\":1}"));
    }
}
