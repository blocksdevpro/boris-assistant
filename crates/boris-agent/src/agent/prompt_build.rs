//! System prompt assembly plus separate dynamic reference-data injection.

use crate::memory::PERSONAL_CONTEXT_MAX_CHARS;
use crate::prompt_profile::{PromptContext, UserInfo};
use crate::skills;

use super::Agent;

impl Agent {
    /// Toggle `<user_info>` injection (default: on).
    pub fn set_include_user_info(&mut self, include: bool) {
        self.include_user_info = include;
        self.refresh_system_prompt();
    }

    pub fn refresh_system_prompt(&mut self) {
        let composed = self.prompt_context().render();
        self.context.set_system(composed);
        self.context
            .set_personal_context(self.personal_context_data());
        let catalog = self.skills.as_ref().and_then(|shared| {
            shared
                .lock()
                .ok()
                .map(|loaded| skills::format_skills_catalog(&loaded.skills))
                .filter(|text| !text.is_empty())
        });
        self.context.set_skills_catalog(catalog);
    }

    /// Build the inspectable prompt profile (Grok-style `PromptContext`).
    pub fn prompt_context(&self) -> PromptContext {
        // The SQLite record plane is authoritative. The profile snapshot is
        // retained only to guide extraction cadence, never as a competing
        // prompt source.
        let skills_policy = self
            .skills
            .as_ref()
            .map(|_| skills::SKILLS_SYSTEM_POLICY.to_string());
        let memory_hint = self
            .memory_store
            .as_ref()
            .map(|m| m.prompt_hint())
            .or_else(|| self.long_term.as_ref().map(|m| m.prompt_hint()));
        let mut ctx = PromptContext::new(self.base_system_prompt.clone())
            .with_skills_policy(skills_policy)
            .with_memory_hint(memory_hint);
        if self.include_user_info {
            ctx = ctx.with_user_info(UserInfo::capture());
        }
        ctx
    }

    fn personal_context_data(&self) -> Option<String> {
        self.memory_store
            .as_ref()
            .and_then(|store| store.personal_context(PERSONAL_CONTEXT_MAX_CHARS).ok())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.personal.as_ref().and_then(|mem| {
                    mem.profile
                        .lock()
                        .ok()
                        .map(|p| p.render_block(PERSONAL_CONTEXT_MAX_CHARS))
                        .filter(|s| !s.is_empty())
                })
            })
    }

    pub(super) fn composed_system_prompt(&self) -> String {
        self.prompt_context().render()
    }

    pub fn set_base_system_prompt(&mut self, system_prompt: &str) {
        self.base_system_prompt = system_prompt.to_string();
        self.refresh_system_prompt();
    }

    /// Refresh system prompt so progressive catalog can mention discovery.
    pub(super) fn inject_progressive_prompt_hint(&mut self) {
        self.refresh_system_prompt();
    }
}
