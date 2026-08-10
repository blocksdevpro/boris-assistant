//! Proposed profile updates and merge onto [`UserProfile`].

use crate::memory::profile::{UserFact, UserProfile};

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
