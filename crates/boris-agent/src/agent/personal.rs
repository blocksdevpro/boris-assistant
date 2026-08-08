//! Personal context attach / extract after turns.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tracing::{debug, info, warn};

use crate::memory::{
    extract_heuristic, extract_with_llm, should_llm_extract, ProfileStore, UserProfile,
};

use super::Agent;

/// Optional durable personal context attached to the agent.
pub(super) struct PersonalMemory {
    pub(super) store: ProfileStore,
    pub(super) profile: Arc<Mutex<UserProfile>>,
    pub(super) llm_extract: bool,
}

impl Agent {
    /// Enable durable personal context stored at `profile_path`.
    pub fn enable_personal_context(
        &mut self,
        profile_path: impl Into<PathBuf>,
        llm_extract: bool,
    ) -> Result<Arc<Mutex<UserProfile>>, String> {
        let store = ProfileStore::new(profile_path);
        let profile = store.load()?;
        let shared = Arc::new(Mutex::new(profile));
        self.personal = Some(PersonalMemory {
            store,
            profile: shared.clone(),
            llm_extract,
        });
        self.refresh_system_prompt();
        info!(
            path = %self.personal.as_ref().unwrap().store.path().display(),
            "personal context enabled"
        );
        Ok(shared)
    }

    pub fn personal_profile(&self) -> Option<Arc<Mutex<UserProfile>>> {
        self.personal.as_ref().map(|p| p.profile.clone())
    }

    pub fn profile_store_path(&self) -> Option<PathBuf> {
        self.personal.as_ref().map(|p| p.store.path().to_path_buf())
    }

    /// Heuristic + optional LLM personal-memory extract after a completed turn.
    pub(super) async fn after_turn_learn(
        &mut self,
        user_text: &str,
        assistant_text: &str,
        tools_used: &[String],
    ) {
        let Some(mem) = &self.personal else {
            return;
        };
        let llm_extract_enabled = mem.llm_extract;

        let mut delta = extract_heuristic(user_text);
        let heuristic_hit = !delta.is_empty();

        let (turns_seen, profile_summary, do_llm) = {
            let Ok(mut p) = mem.profile.lock() else {
                return;
            };
            p.turns_seen = p.turns_seen.saturating_add(1);
            let turns_seen = p.turns_seen;
            let summary = if p.is_empty() {
                "(empty)".to_string()
            } else {
                p.render_block(400)
            };
            let do_llm = llm_extract_enabled
                && should_llm_extract(user_text, tools_used, turns_seen, heuristic_hit);
            (turns_seen, summary, do_llm)
        };

        if do_llm {
            match extract_with_llm(
                self.client.as_ref(),
                user_text,
                assistant_text,
                &profile_summary,
            )
            .await
            {
                Ok(llm_delta) if !llm_delta.is_empty() => {
                    debug!(turns_seen, "personal llm extract produced updates");
                    if let Some(n) = llm_delta.preferred_name.clone() {
                        delta.preferred_name = Some(n);
                    }
                    if let Some(a) = llm_delta.address_as.clone() {
                        delta.address_as = Some(a);
                    }
                    delta.preferences_add.extend(llm_delta.preferences_add);
                    delta.facts_add.extend(llm_delta.facts_add);
                    delta
                        .facts_remove_query
                        .extend(llm_delta.facts_remove_query);
                    delta.ongoing_add.extend(llm_delta.ongoing_add);
                    if llm_delta.ongoing_replace.is_some() {
                        delta.ongoing_replace = llm_delta.ongoing_replace;
                    }
                }
                Ok(_) => {
                    debug!(turns_seen, "personal llm extract empty");
                }
                Err(e) => {
                    warn!(error = %e, "personal llm extract failed");
                }
            }
        }

        if delta.is_empty() && !heuristic_hit {
            if let Some(mem) = &self.personal {
                if let Ok(p) = mem.profile.lock() {
                    let _ = mem.store.save(&p);
                }
            }
            return;
        }

        if let Some(mem) = &self.personal {
            if let Ok(mut p) = mem.profile.lock() {
                let before_empty = p.is_empty();
                delta.apply(&mut p);
                if let Err(e) = mem.store.save(&p) {
                    warn!(error = %e, "failed to save personal profile");
                } else {
                    info!(
                        was_empty = before_empty,
                        name = ?p.preferred_name,
                        facts = p.facts.len(),
                        prefs = p.preferences.len(),
                        "personal context updated"
                    );
                }
            }
        }

        self.refresh_system_prompt();
    }

    /// Refresh system prompt when profile tools mutated personal context mid-turn.
    pub(super) fn maybe_refresh_after_tools(&mut self, tools_used: &[String]) {
        if tools_used.iter().any(|n| {
            n == "save_user_fact" || n == "update_user_profile" || n == "get_user_context"
        }) {
            self.refresh_system_prompt();
        }
    }
}
