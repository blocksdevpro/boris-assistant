//! Personal context attach / extract after turns.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

    /// Synchronous, local-only learning fallback for hosts without a
    /// maintenance worker. This intentionally never performs an LLM call.
    pub(super) fn learn_personal_heuristic(&mut self, user_text: &str) {
        let Some(mem) = &self.personal else {
            return;
        };
        let delta = extract_heuristic(user_text);
        let changed = !delta.is_empty();
        let result = (|| -> Result<(), String> {
            let mut profile = mem
                .profile
                .lock()
                .map_err(|_| "personal profile lock poisoned".to_string())?;
            profile.turns_seen = profile.turns_seen.saturating_add(1);
            if changed {
                delta.apply(&mut profile);
            }
            mem.store.save(&profile)
        })();
        match result {
            Ok(()) if changed => info!("personal heuristic fallback updated profile"),
            Ok(()) => debug!("personal heuristic fallback recorded turn"),
            Err(e) => warn!(error = %e, "personal heuristic fallback save failed"),
        }
        self.refresh_system_prompt();
    }

    /// Heuristic + optional LLM personal-memory extract after a completed turn.
    #[allow(dead_code)]
    pub(super) async fn after_turn_learn(
        &mut self,
        user_text: &str,
        assistant_text: &str,
        tools_used: &[String],
    ) {
        let Some(mem) = &self.personal else {
            return;
        };
        let started = Instant::now();
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
                    debug!(
                        turns_seen,
                        ms = started.elapsed().as_millis() as u64,
                        "personal llm extract produced updates"
                    );
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
                    debug!(
                        turns_seen,
                        ms = started.elapsed().as_millis() as u64,
                        "personal llm extract empty"
                    );
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        ms = started.elapsed().as_millis() as u64,
                        "personal llm extract failed"
                    );
                }
            }
        }

        if delta.is_empty() && !heuristic_hit {
            if let Some(mem) = &self.personal {
                if let Ok(p) = mem.profile.lock() {
                    let _ = mem.store.save(&p);
                }
            }
            debug!(
                ms = started.elapsed().as_millis() as u64,
                "personal learn skipped"
            );
            return;
        }

        if let Some(mem) = &self.personal {
            if let Ok(mut p) = mem.profile.lock() {
                let before_empty = p.is_empty();
                delta.apply(&mut p);
                if let Err(e) = mem.store.save(&p) {
                    warn!(
                        error = %e,
                        ms = started.elapsed().as_millis() as u64,
                        "failed to save personal profile"
                    );
                } else {
                    info!(
                        was_empty = before_empty,
                        name = ?p.preferred_name,
                        facts = p.facts.len(),
                        prefs = p.preferences.len(),
                        ms = started.elapsed().as_millis() as u64,
                        "personal context updated"
                    );
                }
            }
        }

        self.refresh_system_prompt();
    }

    /// Refresh system prompt when profile tools mutated personal context mid-turn.
    pub(super) fn maybe_refresh_after_tools(&mut self, tools_used: &[String]) {
        if tools_used
            .iter()
            .any(|n| n == "save_user_fact" || n == "update_user_profile" || n == "get_user_context")
        {
            self.refresh_system_prompt();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use boris_ai::{LlmClient, LlmError};
    use serde_json::{json, Value};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct NoopClient;

    #[async_trait]
    impl LlmClient for NoopClient {
        async fn complete(&self, _messages: Value, _tools: Value) -> Result<Value, LlmError> {
            Ok(json!({"role":"assistant","content":"ok"}))
        }
    }

    #[test]
    fn heuristic_fallback_updates_and_persists_without_worker() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("boris-personal-fallback-{unique}"));
        let path = dir.join("profile.json");
        let mut agent = Agent::new(Box::new(NoopClient), "sys");
        let profile = agent.enable_personal_context(&path, true).unwrap();

        agent.learn_personal_heuristic("My name is Ada");

        let current = profile.lock().unwrap().clone();
        assert_eq!(current.preferred_name.as_deref(), Some("Ada"));
        assert_eq!(current.turns_seen, 1);
        let stored = ProfileStore::new(&path).load().unwrap();
        assert_eq!(stored.preferred_name.as_deref(), Some("Ada"));
        assert_eq!(stored.turns_seen, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
