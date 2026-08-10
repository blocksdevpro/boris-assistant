//! Explicit system-prompt composition (Grok `PromptContext`, Boris-sized).
//!
//! Keeps the base persona, OS/user_info, personal memory, skills catalog, and
//! optional memory hints as named sections instead of opaque string concat.
//!
//! # Surface
//!
//! | Type | Role |
//! |------|------|
//! | [`UserInfo`] | Host facts for the `<user_info>` block |
//! | [`PromptContext`] | Inspectable sections + stable `render()` order |
//!
//! Render order is fixed: base → user_info → personal → skills → memory_hint.
//! Empty / whitespace-only optional sections are omitted.

use serde::{Deserialize, Serialize};

/// Snapshot of environment facts for the `<user_info>` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,
    /// Local calendar date `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_date: Option<String>,
}

impl UserInfo {
    /// Capture best-effort host facts for the current process.
    pub fn capture() -> Self {
        let os_name = Some(std::env::consts::OS.to_string());
        let shell = std::env::var("SHELL")
            .ok()
            .or_else(|| std::env::var("COMSPEC").ok());
        let working_directory = std::env::current_dir()
            .ok()
            .map(|p| p.display().to_string());
        let current_date = chrono::Local::now().format("%Y-%m-%d").to_string();
        Self {
            os_name,
            shell,
            working_directory,
            current_date: Some(current_date),
        }
    }

    /// Render the `<user_info>` block, or empty string when all fields are absent.
    pub fn render_block(&self) -> String {
        if self.os_name.is_none()
            && self.shell.is_none()
            && self.working_directory.is_none()
            && self.current_date.is_none()
        {
            return String::new();
        }
        let mut lines = vec!["<user_info>".to_string()];
        if let Some(os) = &self.os_name {
            lines.push(format!("OS: {os}"));
        }
        if let Some(shell) = &self.shell {
            lines.push(format!("Shell: {shell}"));
        }
        if let Some(cwd) = &self.working_directory {
            lines.push(format!("Working directory: {cwd}"));
        }
        if let Some(date) = &self.current_date {
            lines.push(format!("Today's date: {date}"));
        }
        lines.push("</user_info>".to_string());
        lines.join("\n")
    }
}

/// First-class system prompt profile — inspectable sections, one `render()`.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    /// Core persona / behavior instructions.
    pub base: String,
    /// OS / cwd / date block.
    pub user_info: Option<UserInfo>,
    /// Pre-rendered `<personal_context>` (or empty).
    pub personal_context: Option<String>,
    /// Pre-rendered `<skills>` catalog (or empty).
    pub skills_catalog: Option<String>,
    /// Optional memory / RAG hint when long-term memory is enabled.
    pub memory_hint: Option<String>,
}

impl PromptContext {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            ..Default::default()
        }
    }

    pub fn with_user_info(mut self, info: UserInfo) -> Self {
        self.user_info = Some(info);
        self
    }

    pub fn with_personal(mut self, block: Option<String>) -> Self {
        self.personal_context = block.filter(|s| !s.trim().is_empty());
        self
    }

    pub fn with_skills(mut self, catalog: Option<String>) -> Self {
        self.skills_catalog = catalog.filter(|s| !s.trim().is_empty());
        self
    }

    pub fn with_memory_hint(mut self, hint: Option<String>) -> Self {
        self.memory_hint = hint.filter(|s| !s.trim().is_empty());
        self
    }

    /// Join non-empty sections with blank lines (stable order).
    pub fn render(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if !self.base.trim().is_empty() {
            parts.push(self.base.trim_end());
        }

        let user_block;
        if let Some(info) = &self.user_info {
            user_block = info.render_block();
            if !user_block.is_empty() {
                parts.push(&user_block);
            }
        }

        if let Some(p) = &self.personal_context {
            let t = p.trim();
            if !t.is_empty() {
                parts.push(t);
            }
        }
        if let Some(s) = &self.skills_catalog {
            let t = s.trim();
            if !t.is_empty() {
                parts.push(t);
            }
        }
        if let Some(m) = &self.memory_hint {
            let t = m.trim();
            if !t.is_empty() {
                parts.push(t);
            }
        }

        parts.join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_joins_sections() {
        let ctx = PromptContext::new("You are Boris.")
            .with_user_info(UserInfo {
                os_name: Some("windows".into()),
                shell: None,
                working_directory: Some("C:\\proj".into()),
                current_date: Some("2026-08-04".into()),
            })
            .with_personal(Some("<personal_context>\nName: Ada\n</personal_context>".into()))
            .with_skills(Some("<skills>\n- research\n</skills>".into()));
        let out = ctx.render();
        assert!(out.starts_with("You are Boris."));
        assert!(out.contains("<user_info>"));
        assert!(out.contains("Working directory: C:\\proj"));
        assert!(out.contains("Name: Ada"));
        assert!(out.contains("<skills>"));
    }

    #[test]
    fn empty_optional_sections_omitted() {
        let ctx = PromptContext::new("Base only.")
            .with_personal(Some("  ".into()))
            .with_skills(None);
        assert_eq!(ctx.render(), "Base only.");
    }

    #[test]
    fn with_filters_drop_whitespace_blocks() {
        let ctx = PromptContext::new("Base")
            .with_personal(Some("\n  \t".into()))
            .with_skills(Some("   ".into()))
            .with_memory_hint(Some("\n".into()));
        assert!(ctx.personal_context.is_none());
        assert!(ctx.skills_catalog.is_none());
        assert!(ctx.memory_hint.is_none());
        assert_eq!(ctx.render(), "Base");
    }

    #[test]
    fn render_includes_memory_hint_last() {
        let ctx = PromptContext::new("Base")
            .with_skills(Some("<skills/>".into()))
            .with_memory_hint(Some("<memory_hint>x</memory_hint>".into()));
        let out = ctx.render();
        let skills_pos = out.find("<skills/>").unwrap();
        let mem_pos = out.find("<memory_hint>").unwrap();
        assert!(skills_pos < mem_pos);
    }

    #[test]
    fn user_info_render_block_empty_when_all_none() {
        assert_eq!(UserInfo::default().render_block(), "");
    }

    #[test]
    fn user_info_render_block_partial_fields() {
        let info = UserInfo {
            os_name: Some("windows".into()),
            shell: None,
            working_directory: None,
            current_date: Some("2026-01-01".into()),
        };
        let block = info.render_block();
        assert!(block.starts_with("<user_info>"));
        assert!(block.ends_with("</user_info>"));
        assert!(block.contains("OS: windows"));
        assert!(block.contains("Today's date: 2026-01-01"));
        assert!(!block.contains("Shell:"));
        assert!(!block.contains("Working directory:"));
    }

    #[test]
    fn user_info_capture_sets_os_and_date() {
        let info = UserInfo::capture();
        assert_eq!(info.os_name.as_deref(), Some(std::env::consts::OS));
        assert!(info.current_date.is_some());
        let date = info.current_date.as_deref().unwrap();
        assert_eq!(date.len(), 10, "YYYY-MM-DD");
        assert!(date.chars().nth(4) == Some('-'));
    }

    #[test]
    fn empty_base_omitted_from_render() {
        let ctx = PromptContext::new("   ").with_memory_hint(Some("hint".into()));
        assert_eq!(ctx.render(), "hint");
    }

    #[test]
    fn blank_line_separators_between_sections() {
        let ctx = PromptContext::new("A").with_skills(Some("B".into()));
        assert_eq!(ctx.render(), "A\n\nB");
    }
}
