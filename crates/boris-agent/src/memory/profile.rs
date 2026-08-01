//! Structured personal context about the human user.
//!
//! Designed for a voice assistant: compact, high-signal, durable facts — not
//! a dump of the full chat log. Updated actively by heuristics, tools, and
//! optional post-turn LLM extraction.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Categories keep retrieval / formatting tidy for a short spoken agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FactCategory {
    Identity,
    Preference,
    Project,
    Relationship,
    Habit,
    #[default]
    Other,
}

impl FactCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Preference => "preference",
            Self::Project => "project",
            Self::Relationship => "relationship",
            Self::Habit => "habit",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "identity" | "name" | "who" => Self::Identity,
            "preference" | "prefers" | "likes" | "dislikes" => Self::Preference,
            "project" | "work" | "job" => Self::Project,
            "relationship" | "people" | "family" => Self::Relationship,
            "habit" | "routine" => Self::Habit,
            _ => Self::Other,
        }
    }
}

/// One durable fact about the user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserFact {
    pub id: String,
    pub text: String,
    pub category: FactCategory,
    /// 0.0–1.0; higher = more trusted.
    pub confidence: f32,
    /// Where this came from (user utterance, tool, llm extract).
    pub source: String,
    pub created_at_ms: u64,
    pub last_seen_at_ms: u64,
    /// 1–10; used when trimming the prompt block.
    pub salience: u8,
}

impl UserFact {
    pub fn new(text: impl Into<String>, category: FactCategory, source: impl Into<String>) -> Self {
        let now = now_ms();
        let text = normalize_fact_text(text.into());
        Self {
            id: fact_id(&text),
            text,
            category,
            confidence: 0.75,
            source: source.into(),
            created_at_ms: now,
            last_seen_at_ms: now,
            salience: default_salience(category),
        }
    }
}

/// Stable personal profile Boris uses every turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserProfile {
    pub version: u32,
    /// What they go by (first name / nickname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_name: Option<String>,
    /// How Boris should address them if different (rarely used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address_as: Option<String>,
    /// Free-form preference lines ("prefers short answers", "hates being called bro").
    #[serde(default)]
    pub preferences: Vec<String>,
    /// Durable facts about them.
    #[serde(default)]
    pub facts: Vec<UserFact>,
    /// Current projects / topics they care about right now.
    #[serde(default)]
    pub ongoing: Vec<String>,
    pub updated_at_ms: u64,
    /// Turns processed with this profile attached (for extract cadence).
    #[serde(default)]
    pub turns_seen: u64,
}

impl Default for UserProfile {
    fn default() -> Self {
        Self {
            version: 1,
            preferred_name: None,
            address_as: None,
            preferences: Vec::new(),
            facts: Vec::new(),
            ongoing: Vec::new(),
            updated_at_ms: now_ms(),
            turns_seen: 0,
        }
    }
}

impl UserProfile {
    pub fn is_empty(&self) -> bool {
        self.preferred_name.is_none()
            && self.address_as.is_none()
            && self.preferences.is_empty()
            && self.facts.is_empty()
            && self.ongoing.is_empty()
    }

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }

    pub fn set_preferred_name(&mut self, name: impl Into<String>) {
        let name = clean_name(name.into());
        if name.is_empty() {
            return;
        }
        self.preferred_name = Some(name);
        self.touch();
    }

    pub fn add_preference(&mut self, line: impl Into<String>) {
        let line = normalize_fact_text(line.into());
        if line.is_empty() {
            return;
        }
        if self
            .preferences
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&line))
        {
            return;
        }
        self.preferences.push(line);
        // Cap preferences.
        const MAX_PREFS: usize = 24;
        if self.preferences.len() > MAX_PREFS {
            let drop_n = self.preferences.len() - MAX_PREFS;
            self.preferences.drain(0..drop_n);
        }
        self.touch();
    }

    pub fn add_or_refresh_fact(&mut self, mut fact: UserFact) {
        fact.text = normalize_fact_text(fact.text);
        if fact.text.is_empty() {
            return;
        }
        // Merge near-duplicates (same id or highly similar text).
        if let Some(existing) = self
            .facts
            .iter_mut()
            .find(|f| f.id == fact.id || similar_fact(&f.text, &fact.text))
        {
            existing.last_seen_at_ms = now_ms();
            existing.confidence = ((existing.confidence + fact.confidence) * 0.5).clamp(0.0, 1.0);
            if fact.salience > existing.salience {
                existing.salience = fact.salience;
            }
            // Prefer clearer longer text if similar.
            if fact.text.len() > existing.text.len() {
                existing.text = fact.text;
            }
            self.touch();
            return;
        }
        self.facts.push(fact);
        self.trim_facts();
        self.touch();
    }

    pub fn remove_facts_matching(&mut self, query: &str) {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return;
        }
        let before = self.facts.len();
        self.facts
            .retain(|f| !f.text.to_ascii_lowercase().contains(&q));
        if self.facts.len() != before {
            self.touch();
        }
    }

    pub fn set_ongoing(&mut self, items: Vec<String>) {
        self.ongoing = items
            .into_iter()
            .map(normalize_fact_text)
            .filter(|s| !s.is_empty())
            .take(10)
            .collect();
        self.touch();
    }

    pub fn add_ongoing(&mut self, item: impl Into<String>) {
        let item = normalize_fact_text(item.into());
        if item.is_empty() {
            return;
        }
        if self.ongoing.iter().any(|o| o.eq_ignore_ascii_case(&item)) {
            return;
        }
        self.ongoing.push(item);
        if self.ongoing.len() > 10 {
            let drop_n = self.ongoing.len() - 10;
            self.ongoing.drain(0..drop_n);
        }
        self.touch();
    }

    fn trim_facts(&mut self) {
        const MAX_FACTS: usize = 48;
        if self.facts.len() <= MAX_FACTS {
            return;
        }
        // Drop lowest salience, then oldest last_seen.
        self.facts.sort_by(|a, b| {
            b.salience
                .cmp(&a.salience)
                .then_with(|| b.last_seen_at_ms.cmp(&a.last_seen_at_ms))
        });
        self.facts.truncate(MAX_FACTS);
    }

    /// Compact block injected into the system prompt every turn.
    ///
    /// Hard character budget keeps voice turns cheap and avoids drowning the
    /// persona prompt.
    pub fn render_block(&self, max_chars: usize) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("<personal_context>".into());
        lines.push(
            "Durable facts about the human. Use them to personalize. Do not recite this block."
                .into(),
        );

        if let Some(name) = &self.preferred_name {
            lines.push(format!("Name: {name}"));
        }
        if let Some(addr) = &self.address_as {
            if self.preferred_name.as_ref() != Some(addr) {
                lines.push(format!("Address as: {addr}"));
            }
        }
        if !self.preferences.is_empty() {
            lines.push(format!("Preferences: {}", self.preferences.join("; ")));
        }
        if !self.ongoing.is_empty() {
            lines.push(format!("Ongoing: {}", self.ongoing.join("; ")));
        }
        if !self.facts.is_empty() {
            lines.push("Facts:".into());
            let mut facts = self.facts.clone();
            facts.sort_by(|a, b| {
                b.salience
                    .cmp(&a.salience)
                    .then_with(|| b.last_seen_at_ms.cmp(&a.last_seen_at_ms))
            });
            for f in facts.iter().take(16) {
                lines.push(format!("- ({}) {}", f.category.as_str(), f.text));
            }
        }
        lines.push("</personal_context>".into());

        let mut out = lines.join("\n");
        if out.len() > max_chars {
            // Truncate from the end of facts first by rebuilding with fewer facts.
            out = self.render_block_budget(max_chars);
        }
        out
    }

    fn render_block_budget(&self, max_chars: usize) -> String {
        let mut facts = self.facts.clone();
        facts.sort_by(|a, b| {
            b.salience
                .cmp(&a.salience)
                .then_with(|| b.last_seen_at_ms.cmp(&a.last_seen_at_ms))
        });
        for take in (0..=facts.len().min(16)).rev() {
            let mut lines: Vec<String> = Vec::new();
            lines.push("<personal_context>".into());
            lines.push(
                "Durable facts about the human. Use them to personalize. Do not recite this block."
                    .into(),
            );
            if let Some(name) = &self.preferred_name {
                lines.push(format!("Name: {name}"));
            }
            if !self.preferences.is_empty() {
                let prefs: Vec<_> = self.preferences.iter().take(8).cloned().collect();
                lines.push(format!("Preferences: {}", prefs.join("; ")));
            }
            if !self.ongoing.is_empty() {
                let on: Vec<_> = self.ongoing.iter().take(5).cloned().collect();
                lines.push(format!("Ongoing: {}", on.join("; ")));
            }
            if take > 0 {
                lines.push("Facts:".into());
                for f in facts.iter().take(take) {
                    lines.push(format!("- ({}) {}", f.category.as_str(), f.text));
                }
            }
            lines.push("</personal_context>".into());
            let out = lines.join("\n");
            if out.len() <= max_chars {
                return out;
            }
        }
        // Absolute fallback.
        let name = self.preferred_name.as_deref().unwrap_or("unknown");
        format!("<personal_context>\nName: {name}\n</personal_context>")
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn default_salience(cat: FactCategory) -> u8 {
    match cat {
        FactCategory::Identity => 9,
        FactCategory::Preference => 7,
        FactCategory::Project => 6,
        FactCategory::Relationship => 6,
        FactCategory::Habit => 5,
        FactCategory::Other => 4,
    }
}

fn normalize_fact_text(s: String) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim().trim_matches(|c: char| c == '"' || c == '\'');
    // Cap individual fact length for voice budget.
    if s.chars().count() > 160 {
        s.chars().take(157).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}

fn clean_name(s: String) -> String {
    let s = s
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '.' || c == '!');
    // First token / short name only.
    let s = s.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
    if s.chars().count() > 40 {
        s.chars().take(40).collect()
    } else {
        s
    }
}

fn fact_id(text: &str) -> String {
    // Stable-ish id from normalized lowercase text (no extra deps).
    let key = text.to_ascii_lowercase();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("f{h:016x}")
}

fn similar_fact(a: &str, b: &str) -> bool {
    let a = a.to_ascii_lowercase();
    let b = b.to_ascii_lowercase();
    if a == b {
        return true;
    }
    // Containment for short updates ("likes rust" vs "likes rust and go").
    if a.len() >= 12 && b.len() >= 12 && (a.contains(&b) || b.contains(&a)) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_empty_is_empty() {
        assert!(UserProfile::default().render_block(800).is_empty());
    }

    #[test]
    fn add_fact_dedupes() {
        let mut p = UserProfile::default();
        p.add_or_refresh_fact(UserFact::new(
            "Works on Boris in Rust",
            FactCategory::Project,
            "test",
        ));
        p.add_or_refresh_fact(UserFact::new(
            "Works on Boris in Rust",
            FactCategory::Project,
            "test",
        ));
        assert_eq!(p.facts.len(), 1);
    }

    #[test]
    fn name_and_render() {
        let mut p = UserProfile::default();
        p.set_preferred_name("Uttam");
        p.add_preference("likes short answers");
        let block = p.render_block(800);
        assert!(block.contains("Uttam"));
        assert!(block.contains("short answers"));
        assert!(block.contains("<personal_context>"));
    }
}
