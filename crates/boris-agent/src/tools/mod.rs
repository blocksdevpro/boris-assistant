//! Built-in agent tools and host registration helpers.
//!
//! # Layout
//!
//! | Module | Tools |
//! |--------|--------|
//! | [`time`] | get_time, get_date |
//! | [`notes`] | remember / recall notes |
//! | [`profile`] | user profile / facts |
//! | [`system`] / [`open_tool`] / [`clipboard`] / [`todo`] | OS surface |
//! | [`files`] / [`glob`] / [`grep`] | filesystem |
//! | [`web`] | web_search, web_fetch |
//! | [`bash`] | shell |
//! | [`skills_tools`] / [`memory_tools`] / [`subagent`] / [`tool_search`] | advanced |
//!
//! Hosts (pipeline / desktop) should call [`register_builtin_tools`] once after
//! constructing [`crate::Agent`] with a [`BuiltinToolPaths`] pointing at `~/.boris`.

pub mod bash;
pub mod clipboard;
pub mod files;
pub mod fs_common;
pub mod glob;
pub mod grep;
pub mod memory_tools;
pub mod notes;
pub mod open_tool;
pub mod path_pattern;
pub mod profile;
pub mod skills_tools;
pub mod subagent;
pub mod system;
pub mod time;
pub mod todo;
pub mod tool_search;
pub mod web;

use std::path::PathBuf;

use crate::agent::Agent;
use crate::capability::{filter_tools_for_preset, CapabilityPreset};
use crate::tool::Tool;
use crate::tools::files::FsRoots;

/// Paths and sandbox roots for file-backed / OS tools.
///
/// Built by the host from `boris-pipeline` home layout (`~/.boris`).
#[derive(Debug, Clone)]
pub struct BuiltinToolPaths {
    /// Notes file path.
    pub notes_path: PathBuf,
    /// Durable personal profile JSON (`~/.boris/memory/profile.json`).
    pub profile_path: PathBuf,
    /// Default write sandbox (`~/.boris/sandbox`).
    pub sandbox_root: PathBuf,
    /// Boris data roots (memory, sessions) — readable/writable for memory tools.
    pub data_roots: Vec<PathBuf>,
    /// Extra user-granted read roots (Desktop, Documents, …).
    pub allow_read: Vec<PathBuf>,
    /// Extra user-granted write roots (usually empty).
    pub allow_write: Vec<PathBuf>,
    /// Boris home (for system_info display).
    pub boris_home: PathBuf,
}

impl BuiltinToolPaths {
    fn fs_roots(&self) -> FsRoots {
        FsRoots {
            sandbox: self.sandbox_root.clone(),
            data: self.data_roots.clone(),
            allow_read: self.allow_read.clone(),
            allow_write: self.allow_write.clone(),
        }
    }

    fn read_roots_flat(&self) -> Vec<PathBuf> {
        fs_common::read_roots(
            &self.sandbox_root,
            &self.data_roots,
            &self.allow_read,
            &self.allow_write,
        )
    }
}

// ── Tool set factories ───────────────────────────────────────────────────────

/// Core v1 tools: time + notes (no profile).
pub fn builtin_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(time::GetTimeTool),
        Box::new(time::GetDateTool),
        Box::new(notes::RememberNoteTool::new(paths.notes_path.clone())),
        Box::new(notes::RecallNotesTool::new(paths.notes_path.clone())),
    ]
}

/// OS surface: system info, open, clipboard, todos.
pub fn os_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(system::GetSystemInfoTool::new(
            paths.boris_home.to_string_lossy().into_owned(),
        )),
        Box::new(open_tool::OpenUrlTool),
        Box::new(open_tool::OpenPathTool::new(paths.read_roots_flat())),
        Box::new(clipboard::ClipboardGetTool),
        Box::new(clipboard::ClipboardSetTool),
        Box::new(todo::TodoReadTool::new(&paths.sandbox_root)),
        Box::new(todo::TodoWriteTool::new(&paths.sandbox_root)),
    ]
}

/// Filesystem tools (list / read / write / edit / glob / grep).
pub fn fs_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    let roots = paths.fs_roots();
    vec![
        Box::new(files::ListDirTool::new(roots.clone())),
        Box::new(files::ReadFileTool::new(roots.clone())),
        Box::new(files::WriteFileTool::new(roots.clone())),
        Box::new(files::EditFileTool::new(roots.clone())),
        Box::new(glob::GlobTool::new(roots.clone())),
        Box::new(grep::GrepTool::new(roots)),
    ]
}

/// Web search + fetch (requires host network policy Open).
///
/// Registration failures are logged and skipped (missing API keys, etc.).
pub fn web_tools() -> Vec<Box<dyn Tool>> {
    let mut out: Vec<Box<dyn Tool>> = Vec::new();
    match web::WebSearchTool::new() {
        Ok(t) => out.push(Box::new(t)),
        Err(e) => tracing::warn!(error = %e, "web_search not registered"),
    }
    match web::WebFetchTool::new() {
        Ok(t) => out.push(Box::new(t)),
        Err(e) => tracing::warn!(error = %e, "web_fetch not registered"),
    }
    out
}

/// Bash tool (requires host shell policy + HITL for dangerous runs).
pub fn bash_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    let cwd_roots = paths.read_roots_flat();
    vec![Box::new(bash::BashTool::new(
        cwd_roots,
        paths.sandbox_root.clone(),
    ))]
}

/// Alias for [`bash_tools`].
#[deprecated(note = "use bash_tools")]
pub fn shell_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    bash_tools(paths)
}

// ── Registration on Agent ────────────────────────────────────────────────────

/// Register time + notes tools. Prefer [`register_builtin_tools`].
pub fn register_time_and_notes(agent: &mut Agent, paths: &BuiltinToolPaths) {
    agent.register_tools(builtin_tools(paths));
}

/// Full host setup: personal context + all MVP tool waves (Full preset).
pub fn register_builtin_tools(agent: &mut Agent, paths: BuiltinToolPaths) {
    register_builtin_tools_with_preset(agent, paths, true, true, CapabilityPreset::Full);
}

/// Same as [`register_builtin_tools`] with control over LLM extract and power tools.
pub fn register_builtin_tools_with_options(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
) {
    register_builtin_tools_with_preset(
        agent,
        paths,
        llm_extract,
        power_tools,
        CapabilityPreset::Full,
    );
}

/// Full registration with a capability preset (toolset filtering).
///
/// 1. Core time/notes  
/// 2. Optional personal-context tools  
/// 3. Optional power tools (OS / FS / web / bash)  
/// 4. Filter by [`CapabilityPreset`]  
/// 5. [`Agent::register_tools`]
pub fn register_builtin_tools_with_preset(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
    preset: CapabilityPreset,
) {
    let mut tools = builtin_tools(&paths);
    tools.extend(try_profile_tools(agent, &paths, llm_extract));

    if power_tools {
        tools.extend(os_tools(&paths));
        tools.extend(fs_tools(&paths));
        tools.extend(web_tools());
        tools.extend(bash_tools(&paths));
    }

    let tools = filter_tools_for_preset(tools, preset);
    agent.register_tools(tools);
}

fn try_profile_tools(
    agent: &mut Agent,
    paths: &BuiltinToolPaths,
    llm_extract: bool,
) -> Vec<Box<dyn Tool>> {
    match agent.enable_personal_context(&paths.profile_path, llm_extract) {
        Ok(profile) => vec![
            Box::new(profile::SaveUserFactTool::with_path(
                profile.clone(),
                paths.profile_path.clone(),
            )),
            Box::new(profile::UpdateUserProfileTool::with_path(
                profile.clone(),
                paths.profile_path.clone(),
            )),
            Box::new(profile::GetUserContextTool::new(profile)),
        ],
        Err(e) => {
            tracing::warn!(
                error = %e,
                "personal context enable failed; profile tools not registered"
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths() -> BuiltinToolPaths {
        let root = std::env::temp_dir().join(format!("boris-tool-meta-{}", std::process::id()));
        BuiltinToolPaths {
            notes_path: root.join("notes.jsonl"),
            profile_path: root.join("profile.json"),
            sandbox_root: root.join("sandbox"),
            data_roots: vec![root.join("memory")],
            allow_read: vec![],
            allow_write: vec![],
            boris_home: root,
        }
    }

    /// Lint-style: every registered builtin should set explicit `read_only`.
    #[test]
    fn builtins_set_explicit_read_only() {
        let paths = test_paths();
        let mut tools: Vec<Box<dyn Tool>> = Vec::new();
        tools.extend(builtin_tools(&paths));
        tools.extend(os_tools(&paths));
        tools.extend(fs_tools(&paths));
        tools.extend(web_tools());
        tools.extend(bash_tools(&paths));

        let mut missing = Vec::new();
        for t in &tools {
            if t.meta().read_only.is_none() {
                missing.push(t.name().to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "tools missing explicit ToolMeta::read_only: {missing:?}"
        );
    }

    #[test]
    fn shell_allowlist_style_integration_via_bash_meta() {
        let paths = test_paths();
        let tools = bash_tools(&paths);
        assert_eq!(tools.len(), 1);
        let m = tools[0].meta();
        assert!(m.permissions.contains(&crate::tool::Permission::Shell));
        assert_eq!(m.read_only, Some(false));
        assert!(m.requires_confirmation);
    }
}
