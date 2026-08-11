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

use std::path::{Path, PathBuf};

use crate::agent::Agent;
use crate::capability::{filter_tools_for_preset, CapabilityPreset};
use crate::runtime::SandboxConfig;
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

/// Plan / multi-step tracking — always registered (including VoiceSafe).
///
/// Kept separate from [`os_tools`] so capability presets that disable power
/// tools still get todos. The system prompt, skills, and finish-gate all
/// reference `todo_write`; missing registration used to hard-fail turns.
pub fn plan_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    plan_tools_at(&paths.sandbox_root.join("todos.json"))
}

/// Plan tools bound to an explicit todos file (session-local path).
pub fn plan_tools_at(todos_file: &Path) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(todo::TodoReadTool::with_path(todos_file)),
        Box::new(todo::TodoWriteTool::with_path(todos_file)),
    ]
}

/// OS surface: system info, open, clipboard (power-tool wave).
pub fn os_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(system::GetSystemInfoTool::new(
            paths.boris_home.to_string_lossy().into_owned(),
        )),
        Box::new(open_tool::OpenUrlTool),
        Box::new(open_tool::OpenPathTool::new(paths.read_roots_flat())),
        Box::new(clipboard::ClipboardGetTool),
        Box::new(clipboard::ClipboardSetTool),
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
/// `web_search` prefers Exa when `EXA_API_KEY` / `BORIS_EXA_API_KEY` or
/// `~/.boris/auth.json` `exa_api_key` is set; otherwise DuckDuckGo HTML scrape.
/// Registration failures are logged and skipped.
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
///
/// `sandbox` is mutated in place via [`CapabilityPreset::apply_to_sandbox`] —
/// pass the same [`SandboxConfig`] you then hand to [`Agent::configure_runtime`]
/// (call this function *before* `configure_runtime` so the runtime picks up the
/// preset-adjusted policy).
pub fn register_builtin_tools(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    sandbox: &mut SandboxConfig,
) {
    register_builtin_tools_with_preset(agent, paths, true, true, sandbox, CapabilityPreset::Full);
}

/// Same as [`register_builtin_tools`] with control over LLM extract and power tools.
pub fn register_builtin_tools_with_options(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
    sandbox: &mut SandboxConfig,
) {
    register_builtin_tools_with_preset(
        agent,
        paths,
        llm_extract,
        power_tools,
        sandbox,
        CapabilityPreset::Full,
    );
}

/// Full registration with a capability preset (toolset filtering).
///
/// 1. Core time/notes
/// 2. Plan tools (todo_read / todo_write) — **always**, even VoiceSafe
/// 3. Optional personal-context tools
/// 4. Optional power tools (OS / FS / web / bash)
/// 5. Filter by [`CapabilityPreset`]
/// 6. [`Agent::register_tools`]
///
/// Also applies `preset` to `sandbox` via [`CapabilityPreset::apply_to_sandbox`]
/// (network/shell lockdown for `VoiceSafe`/`LocalPower`) so tool registration
/// and sandbox policy can't structurally drift apart. Call this **before**
/// [`Agent::configure_runtime`] and pass the same (now preset-adjusted)
/// `sandbox` value into it — see `crates/boris-agent/README.md`.
pub fn register_builtin_tools_with_preset(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
    sandbox: &mut SandboxConfig,
    preset: CapabilityPreset,
) {
    preset.apply_to_sandbox(sandbox);
    let mut tools = builtin_tools(&paths);
    // Always register plan tools. VoiceSafe sets power_tools=false and used to
    // skip os_tools (which previously owned todos), while the prompt still
    // taught the model to call todo_write → "unknown tool" hard-fail loop.
    tools.extend(plan_tools(&paths));
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
        tools.extend(plan_tools(&paths));
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
    fn voice_safe_still_registers_todos() {
        let paths = test_paths();
        let mut tools = builtin_tools(&paths);
        tools.extend(plan_tools(&paths));
        // power_tools=false for VoiceSafe (no os/fs/web/bash).
        let tools = filter_tools_for_preset(tools, CapabilityPreset::VoiceSafe);
        let names: Vec<_> = tools.iter().map(|t| t.name().to_string()).collect();
        assert!(
            names.iter().any(|n| n == "todo_read"),
            "todo_read missing under VoiceSafe: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "todo_write"),
            "todo_write missing under VoiceSafe: {names:?}"
        );
        assert!(!names.iter().any(|n| n == "bash"), "bash must stay off");
        assert!(!names.iter().any(|n| n == "web_search"), "web must stay off");
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

    struct NoopClient;
    #[async_trait::async_trait]
    impl boris_ai::LlmClient for NoopClient {
        async fn complete(
            &self,
            _messages: serde_json::Value,
            _tools: serde_json::Value,
        ) -> Result<serde_json::Value, boris_ai::LlmError> {
            Err(boris_ai::LlmError::new("noop"))
        }
        fn model(&self) -> &str {
            "test"
        }
    }

    /// register_builtin_tools_with_preset must apply the preset to `sandbox`
    /// itself (not rely on the host calling `apply_to_sandbox` separately),
    /// so tool registration and sandbox policy can't structurally drift apart.
    #[test]
    fn register_builtin_tools_with_preset_applies_preset_to_sandbox() {
        let paths = test_paths();
        let mut agent = Agent::new(Box::new(NoopClient), "test");
        let mut sandbox = SandboxConfig::for_desktop_mvp(&paths.boris_home);
        assert_eq!(sandbox.network, crate::runtime::NetworkPolicy::Open);
        assert_eq!(sandbox.shell, crate::runtime::ShellPolicy::OpenConfirm);

        register_builtin_tools_with_preset(
            &mut agent,
            paths,
            true,
            true,
            &mut sandbox,
            CapabilityPreset::VoiceSafe,
        );

        // VoiceSafe locks network/shell closed; this must happen inside the
        // function itself, not only when a caller remembers to call it.
        assert_eq!(sandbox.network, crate::runtime::NetworkPolicy::Off);
        assert_eq!(sandbox.shell, crate::runtime::ShellPolicy::Denied);
    }
}
