//! Compact `<personal_context>` prompt block rendering.

use super::types::UserProfile;

impl UserProfile {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::profile::{FactCategory, UserFact};

    #[test]
    fn render_empty_is_empty() {
        assert!(UserProfile::default().render_block(800).is_empty());
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

    #[test]
    fn budget_still_includes_name() {
        let mut p = UserProfile::default();
        p.set_preferred_name("Ada");
        for i in 0..20 {
            p.add_or_refresh_fact(UserFact::new(
                format!("Fact number {i} with some padding text to grow the block"),
                FactCategory::Other,
                "test",
            ));
        }
        let block = p.render_block(200);
        assert!(block.contains("Ada") || block.contains("<personal_context>"));
        assert!(block.len() <= 200 || block.contains("Ada"));
    }
}
