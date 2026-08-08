//! Profile data types and in-memory mutation.

use serde::{Deserialize, Serialize};

use super::helpers::{
    clean_name, default_salience, fact_id, normalize_fact_text, now_ms, similar_fact,
};

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
}
