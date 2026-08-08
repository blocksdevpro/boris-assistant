//! System prompt assembly (base + personal + skills + memory + user_info).

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
    }

    /// Build the inspectable prompt profile (Grok-style `PromptContext`).
    pub fn prompt_context(&self) -> PromptContext {
        let personal = self.personal.as_ref().and_then(|mem| {
            mem.profile
                .lock()
                .ok()
                .map(|p| p.render_block(PERSONAL_CONTEXT_MAX_CHARS))
                .filter(|s| !s.is_empty())
        });
        let skills_catalog = self.skills.as_ref().and_then(|shared| {
            shared
                .lock()
                .ok()
                .map(|g| skills::format_skills_catalog(&g.skills))
                .filter(|s| !s.is_empty())
        });
        let memory_hint = self.long_term.as_ref().map(|m| m.prompt_hint());
        let mut ctx = PromptContext::new(self.base_system_prompt.clone())
            .with_personal(personal)
            .with_skills(skills_catalog)
            .with_memory_hint(memory_hint);
        if self.include_user_info {
            ctx = ctx.with_user_info(UserInfo::capture());
        }
        ctx
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
