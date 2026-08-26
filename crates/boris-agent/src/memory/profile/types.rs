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

/// Durable lifecycle state. Records are tombstoned instead of deleted so a
/// correction or forgetting request remains auditable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FactStatus {
    #[default]
    Active,
    Superseded,
    Forgotten,
    Expired,
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
    /// Optional semantic slot (`preferred_name`, `home_city`, `favorite_editor`).
    /// A new active value for the same key supersedes the old one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_key: Option<String>,
    #[serde(default)]
    pub status: FactStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_note: Option<String>,
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
            memory_key: None,
            status: FactStatus::Active,
            superseded_by: None,
            expires_at_ms: None,
            lifecycle_note: None,
        }
    }

    pub fn with_memory_key(mut self, key: impl Into<String>) -> Self {
        let key = normalize_key(&key.into());
        self.memory_key = (!key.is_empty()).then_some(key);
        self
    }

    pub fn with_expiry_ms(mut self, expires_at_ms: Option<u64>) -> Self {
        self.expires_at_ms = expires_at_ms;
        self
    }

    pub fn is_active_at(&self, timestamp_ms: u64) -> bool {
        self.status == FactStatus::Active
            && self
                .expires_at_ms
                .is_none_or(|expires| expires > timestamp_ms)
    }

    pub fn is_active(&self) -> bool {
        self.is_active_at(now_ms())
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
            version: 2,
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
            && !self.facts.iter().any(UserFact::is_active)
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
        let changed = self.preferred_name.as_deref() != Some(name.as_str());
        self.preferred_name = Some(name.clone());
        if changed {
            self.add_or_refresh_fact(
                UserFact::new(
                    format!("Preferred name: {name}"),
                    FactCategory::Identity,
                    "profile_update",
                )
                .with_memory_key("preferred_name"),
            );
        } else {
            self.touch();
        }
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
        self.expire_due_facts();
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
            existing.status = FactStatus::Active;
            existing.superseded_by = None;
            existing.lifecycle_note = None;
            existing.expires_at_ms = fact.expires_at_ms;
            existing.source = fact.source;
            if fact.memory_key.is_some() {
                existing.memory_key = fact.memory_key;
            }
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
        if fact.memory_key.is_none() {
            fact.memory_key = inferred_memory_key(&fact.text, fact.category);
        }
        if let Some(key) = fact.memory_key.as_deref() {
            for existing in self.facts.iter_mut().filter(|existing| {
                existing.status == FactStatus::Active
                    && existing.memory_key.as_deref() == Some(key)
                    && existing.id != fact.id
            }) {
                existing.status = FactStatus::Superseded;
                existing.superseded_by = Some(fact.id.clone());
                existing.lifecycle_note = Some(format!("replaced by newer value for `{key}`"));
            }
        }
        self.facts.push(fact);
        self.trim_facts();
        self.touch();
    }

    /// Tombstone matching records and remove matching legacy scalar/list values.
    pub fn forget_matching(&mut self, query: &str) -> usize {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return 0;
        }
        let mut changed = 0;
        for fact in &mut self.facts {
            if fact.status == FactStatus::Active && semantic_match(&fact.text, &q) {
                fact.status = FactStatus::Forgotten;
                fact.lifecycle_note = Some(format!("forgotten by user request: {query}"));
                changed += 1;
            }
        }
        let before_prefs = self.preferences.len();
        self.preferences.retain(|value| !semantic_match(value, &q));
        changed += before_prefs - self.preferences.len();
        let before_ongoing = self.ongoing.len();
        self.ongoing.retain(|value| !semantic_match(value, &q));
        changed += before_ongoing - self.ongoing.len();
        if q.contains("name") {
            if self.preferred_name.take().is_some() {
                changed += 1;
            }
            self.address_as = None;
            self.forget_key("preferred_name", "forgotten by user request");
        }
        if changed > 0 {
            self.touch();
        }
        changed
    }

    /// Back-compatible name; removal now means a durable tombstone.
    pub fn remove_facts_matching(&mut self, query: &str) {
        self.forget_matching(query);
    }

    pub fn forget_all(&mut self) {
        for fact in &mut self.facts {
            if fact.status == FactStatus::Active {
                fact.status = FactStatus::Forgotten;
                fact.lifecycle_note = Some("forgotten by user request: all personal memory".into());
            }
        }
        self.preferred_name = None;
        self.address_as = None;
        self.preferences.clear();
        self.ongoing.clear();
        self.touch();
    }

    pub fn forget_key(&mut self, key: &str, note: &str) {
        let key = normalize_key(key);
        for fact in &mut self.facts {
            if fact.status == FactStatus::Active && fact.memory_key.as_deref() == Some(key.as_str())
            {
                fact.status = FactStatus::Forgotten;
                fact.lifecycle_note = Some(note.to_string());
            }
        }
    }

    /// Materialize time-based expiry as a lifecycle state.
    pub fn expire_due_facts(&mut self) -> usize {
        let now = now_ms();
        let mut expired = 0;
        for fact in &mut self.facts {
            if fact.status == FactStatus::Active
                && fact.expires_at_ms.is_some_and(|expires| expires <= now)
            {
                fact.status = FactStatus::Expired;
                fact.lifecycle_note = Some("expired automatically".into());
                expired += 1;
            }
        }
        if expired > 0 {
            self.touch();
        }
        expired
    }

    /// Whether a retrieved snippet repeats a value that was superseded,
    /// forgotten, or expired in the structured profile.
    pub fn suppresses_retrieval_text(&self, text: &str) -> bool {
        self.facts
            .iter()
            .filter(|fact| fact.status != FactStatus::Active || !fact.is_active())
            .any(|fact| semantic_match(text, &fact.text))
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
        let active = self
            .facts
            .iter()
            .filter(|fact| fact.status == FactStatus::Active)
            .count();
        if active <= MAX_FACTS {
            return;
        }
        let mut active_ids = self
            .facts
            .iter()
            .filter(|fact| fact.status == FactStatus::Active)
            .map(|fact| (fact.id.clone(), fact.salience, fact.last_seen_at_ms))
            .collect::<Vec<_>>();
        active_ids.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
        let keep = active_ids
            .into_iter()
            .take(MAX_FACTS)
            .map(|item| item.0)
            .collect::<std::collections::HashSet<_>>();
        for fact in &mut self.facts {
            if fact.status == FactStatus::Active && !keep.contains(&fact.id) {
                fact.status = FactStatus::Expired;
                fact.lifecycle_note = Some("expired from active prompt capacity".into());
            }
        }
    }
}

fn normalize_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn inferred_memory_key(text: &str, category: FactCategory) -> Option<String> {
    let lower = text.trim().to_ascii_lowercase();
    if !matches!(category, FactCategory::Project | FactCategory::Relationship) {
        if let Some((label, _)) = lower.split_once(':') {
            let key = normalize_key(label);
            if !key.is_empty() && key.len() <= 48 {
                return Some(key);
            }
        }
    }
    for prefix in [
        "lives in ",
        "preferred name ",
        "favorite editor ",
        "timezone ",
    ] {
        if lower.starts_with(prefix) {
            return Some(normalize_key(prefix));
        }
    }
    None
}

fn semantic_match(value: &str, query: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    if value.contains(&query) || query.contains(&value) {
        return true;
    }
    let words = |text: &str| {
        text.split(|ch: char| !ch.is_ascii_alphanumeric())
            .filter(|word| word.len() >= 3)
            .filter(|word| !["that", "this", "about", "from", "with", "remember"].contains(word))
            .map(ToOwned::to_owned)
            .collect::<std::collections::HashSet<String>>()
    };
    let value_words = words(&value);
    let query_words = words(&query);
    !query_words.is_empty()
        && query_words.intersection(&value_words).count() * 2 >= query_words.len()
}
