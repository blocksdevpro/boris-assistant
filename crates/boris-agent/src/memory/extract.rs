//! Active personal-context extraction.
//!
//! Two layers:
//! 1. **Heuristics** — free, high-precision patterns from the user utterance
//!    ("my name is…", "I prefer…", "call me…").
//! 2. **LLM extract** — side-channel JSON call (does not touch chat context)
//!    when the turn looks personal or on a cadence.
//!
//! Both produce a [`ProfileDelta`] applied onto [`UserProfile`].

use serde::Deserialize;
use serde_json::{json, Value};

use super::profile::{FactCategory, UserFact, UserProfile};
use crate::client::LlmClient;
use crate::error::LlmError;

/// Proposed updates to merge into the durable profile.
#[derive(Debug, Clone, Default)]
pub struct ProfileDelta {
    pub preferred_name: Option<String>,
    pub address_as: Option<String>,
    pub preferences_add: Vec<String>,
    pub facts_add: Vec<UserFact>,
    pub facts_remove_query: Vec<String>,
    pub ongoing_add: Vec<String>,
    pub ongoing_replace: Option<Vec<String>>,
}

impl ProfileDelta {
    pub fn is_empty(&self) -> bool {
        self.preferred_name.is_none()
            && self.address_as.is_none()
            && self.preferences_add.is_empty()
            && self.facts_add.is_empty()
            && self.facts_remove_query.is_empty()
            && self.ongoing_add.is_empty()
            && self.ongoing_replace.is_none()
    }

    pub fn apply(self, profile: &mut UserProfile) {
        if let Some(n) = self.preferred_name {
            profile.set_preferred_name(n);
        }
        if let Some(a) = self.address_as {
            let a = a.trim().to_string();
            if !a.is_empty() {
                profile.address_as = Some(a);
                profile.touch();
            }
        }
        for p in self.preferences_add {
            profile.add_preference(p);
        }
        for q in self.facts_remove_query {
            profile.remove_facts_matching(&q);
        }
        for f in self.facts_add {
            profile.add_or_refresh_fact(f);
        }
        if let Some(on) = self.ongoing_replace {
            profile.set_ongoing(on);
        }
        for o in self.ongoing_add {
            profile.add_ongoing(o);
        }
    }
}

/// High-precision, zero-cost extraction from the user utterance.
pub fn extract_heuristic(user_text: &str) -> ProfileDelta {
    let mut delta = ProfileDelta::default();
    let raw = user_text.trim();
    if raw.is_empty() {
        return delta;
    }
    let lower = raw.to_ascii_lowercase();

    // Name patterns.
    if let Some(name) = capture_after(&lower, raw, &["my name is ", "i am ", "i'm ", "im "]) {
        // Avoid "I am tired" etc. — only accept short name-like captures.
        if looks_like_name(&name) {
            delta.preferred_name = Some(name);
        }
    }
    if let Some(name) = capture_after(&lower, raw, &["call me ", "please call me "]) {
        if looks_like_name(&name) {
            delta.preferred_name = Some(name.clone());
            delta.address_as = Some(name);
        }
    }

    // Preferences.
    for prefix in [
        "i prefer ",
        "i like ",
        "i love ",
        "i hate ",
        "i don't like ",
        "i do not like ",
        "please don't ",
        "please do not ",
        "never call me ",
        "don't call me ",
    ] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if rest.len() >= 3 && rest.len() <= 120 {
                delta.preferences_add.push(rest);
            }
        }
    }

    // Work / project.
    for prefix in [
        "i work on ",
        "i'm working on ",
        "i am working on ",
        "i build ",
        "i'm building ",
        "my project is ",
        "my project ",
    ] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if rest.len() >= 3 {
                delta.facts_add.push(UserFact::new(
                    format!("Works on / building: {rest}"),
                    FactCategory::Project,
                    "heuristic",
                ));
                delta.ongoing_add.push(rest);
            }
        }
    }

    // Role / identity.
    for prefix in ["i'm a ", "i am a ", "i'm an ", "i am an "] {
        if let Some(rest) = capture_after(&lower, raw, &[prefix]) {
            if looks_like_role(&rest) {
                delta.facts_add.push(UserFact::new(
                    format!("Is a {rest}"),
                    FactCategory::Identity,
                    "heuristic",
                ));
            }
        }
    }

    delta
}

/// Whether this turn is worth an LLM extract (beyond heuristics).
pub fn should_llm_extract(
    user_text: &str,
    tools_used: &[String],
    turns_seen: u64,
    heuristic_nonempty: bool,
) -> bool {
    let t = user_text.trim();
    if t.chars().count() < 12 {
        return false;
    }
    // Pure time/date questions — skip.
    let lower = t.to_ascii_lowercase();
    if is_ephemeral_query(&lower) {
        return false;
    }
    // Explicit memory tools already ran — still extract structured profile.
    if tools_used
        .iter()
        .any(|n| n == "remember_note" || n == "save_user_fact" || n == "update_user_profile")
    {
        return true;
    }
    // Heuristics already got signal — optional LLM polish only every few turns.
    if heuristic_nonempty {
        return turns_seen % 2 == 0;
    }
    // Personal language markers.
    let personal = [
        " my ",
        " i ",
        " i'm ",
        " i am ",
        " me ",
        " mine ",
        " wife ",
        " husband ",
        " kids ",
        " job ",
        " work ",
        " project ",
        " prefer ",
        " always ",
        " never ",
    ];
    let padded = format!(" {lower} ");
    let score = personal.iter().filter(|p| padded.contains(*p)).count();
    if score >= 2 && t.chars().count() >= 20 {
        return true;
    }
    // Slow cadence for ambient learning.
    turns_seen > 0 && turns_seen % 4 == 0 && t.chars().count() >= 24
}

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
  "facts_add": [{"text": string, "category": "identity|preference|project|relationship|habit|other"}],
  "facts_remove_query": string[],
  "ongoing_add": string[],
  "ongoing_replace": string[]|null
}
Rules:
- Only durable facts (name, prefs, projects, people, habits). Not one-off chit-chat.
- Prefer short factual phrases.
- If nothing new, return empty arrays and nulls.
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

fn parse_llm_delta(content: &str) -> Result<ProfileDelta, LlmError> {
    let json_str = extract_json_object(content)
        .ok_or_else(|| LlmError::parse("personal extract: no JSON object in response"))?;
    let raw: LlmDeltaRaw = serde_json::from_str(json_str)
        .map_err(|e| LlmError::parse(format!("personal extract parse: {e}")))?;

    let mut delta = ProfileDelta::default();
    delta.preferred_name = raw
        .preferred_name
        .filter(|s| !s.trim().is_empty() && looks_like_name(s));
    delta.address_as = raw.address_as.filter(|s| !s.trim().is_empty());
    delta.preferences_add = raw
        .preferences_add
        .into_iter()
        .filter(|s| s.len() >= 3)
        .take(5)
        .collect();
    delta.facts_remove_query = raw.facts_remove_query;
    delta.ongoing_add = raw.ongoing_add.into_iter().take(5).collect();
    delta.ongoing_replace = raw
        .ongoing_replace
        .map(|v| v.into_iter().take(10).collect());
    for f in raw.facts_add.into_iter().take(5) {
        let text = f.text.trim();
        if text.len() < 3 {
            continue;
        }
        let mut fact = UserFact::new(text, FactCategory::parse(&f.category), "llm_extract");
        fact.confidence = 0.65;
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
    ongoing_add: Vec<String>,
    #[serde(default)]
    ongoing_replace: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct LlmFactRaw {
    text: String,
    #[serde(default)]
    category: String,
}

fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&s[start..=end])
}

fn capture_after(lower: &str, original: &str, prefixes: &[&str]) -> Option<String> {
    for p in prefixes {
        if let Some(idx) = lower.find(p) {
            let start = idx + p.len();
            // Map byte index carefully — prefixes are ascii.
            let rest = original.get(start..)?.trim();
            let rest = rest
                .split(['.', '!', '?', ',', ';'])
                .next()
                .unwrap_or(rest)
                .trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn looks_like_name(s: &str) -> bool {
    let s = s.trim();
    let words: Vec<_> = s.split_whitespace().collect();
    if words.is_empty() || words.len() > 3 {
        return false;
    }
    if s.len() < 2 || s.len() > 40 {
        return false;
    }
    // Reject common false positives for "i am …"
    let lower = s.to_ascii_lowercase();
    const BAD: &[&str] = &[
        "tired", "fine", "good", "ok", "okay", "here", "back", "ready", "done", "busy", "hungry",
        "sorry", "sure", "confused", "lost", "home", "going", "trying",
    ];
    if words.len() == 1 && BAD.contains(&lower.as_str()) {
        return false;
    }
    words.iter().all(|w| {
        w.chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\'')
    })
}

fn looks_like_role(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 3 && s.len() <= 60 && !s.contains("http")
}

fn is_ephemeral_query(lower: &str) -> bool {
    let ephemeral = [
        "what time",
        "what's the time",
        "whats the time",
        "what date",
        "what's the date",
        "whats the date",
        "what day is",
        "how are you",
        "hello",
        "hey boris",
        "hi boris",
        "good morning",
        "good night",
    ];
    ephemeral.iter().any(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_name() {
        let d = extract_heuristic("Hey, my name is Uttam");
        assert_eq!(d.preferred_name.as_deref(), Some("Uttam"));
    }

    #[test]
    fn heuristic_skips_i_am_tired() {
        let d = extract_heuristic("I am tired");
        assert!(d.preferred_name.is_none());
    }

    #[test]
    fn heuristic_prefer() {
        let d = extract_heuristic("I prefer short answers please");
        assert!(!d.preferences_add.is_empty());
    }

    #[test]
    fn parse_llm_json() {
        let raw = r#"{"preferred_name":"Sam","preferences_add":["hates long lectures"],"facts_add":[{"text":"Builds robots","category":"project"}],"facts_remove_query":[],"ongoing_add":[],"ongoing_replace":null,"address_as":null}"#;
        let d = parse_llm_delta(raw).unwrap();
        assert_eq!(d.preferred_name.as_deref(), Some("Sam"));
        assert_eq!(d.facts_add.len(), 1);
    }
}
