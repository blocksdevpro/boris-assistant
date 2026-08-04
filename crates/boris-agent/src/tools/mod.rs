//! Built-in agent tools (tau-inspired suite + Boris voice tools).
//!
//! Individual tools live in submodules; this module assembles the default set
//! and exposes registration helpers for the host (`boris-pipeline` / desktop).

pub mod bash;
pub mod clipboard;
pub mod files;
pub mod fs_common;
pub mod glob;
pub mod grep;
pub mod notes;
pub mod open_tool;
pub mod profile;
pub mod system;
pub mod time;
pub mod todo;
pub mod web;

use std::path::PathBuf;

use crate::agent::Agent;
use crate::tool::Tool;
use crate::tools::files::FsRoots;

/// Paths and roots for file-backed / OS tools.
pub struct BuiltinToolPaths {
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

/// Core v1 tools: time, notes (no profile).
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

/// Filesystem tools (tau-style names + list_dir / glob / grep).
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

/// Bash tool (requires host shell policy OpenConfirm + HITL).
pub fn bash_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    let cwd_roots = paths.read_roots_flat();
    vec![Box::new(bash::BashTool::new(
        cwd_roots,
        paths.sandbox_root.clone(),
    ))]
}

/// Alias for [`bash_tools`].
pub fn shell_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    bash_tools(paths)
}

/// Register time + notes tools. Prefer [`register_builtin_tools`].
pub fn register_time_and_notes(agent: &mut Agent, paths: &BuiltinToolPaths) {
    agent.register_tools(builtin_tools(paths));
}

/// Full host setup: personal context + all MVP tool waves.
pub fn register_builtin_tools(agent: &mut Agent, paths: BuiltinToolPaths) {
    register_builtin_tools_with_options(agent, paths, true, true);
}

/// Same as [`register_builtin_tools`] with control over LLM extract and power tools.
pub fn register_builtin_tools_with_options(
    agent: &mut Agent,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
) {
    let mut tools = builtin_tools(&paths);

    match agent.enable_personal_context(&paths.profile_path, llm_extract) {
        Ok(profile) => {
            tools.push(Box::new(profile::SaveUserFactTool::with_path(
                profile.clone(),
                paths.profile_path.clone(),
            )));
            tools.push(Box::new(profile::UpdateUserProfileTool::with_path(
                profile.clone(),
                paths.profile_path.clone(),
            )));
            tools.push(Box::new(profile::GetUserContextTool::new(profile)));
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "personal context enable failed; profile tools not registered"
            );
        }
    }

    if power_tools {
        tools.extend(os_tools(&paths));
        tools.extend(fs_tools(&paths));
        tools.extend(web_tools());
        tools.extend(bash_tools(&paths));
    }

    agent.register_tools(tools);
}
