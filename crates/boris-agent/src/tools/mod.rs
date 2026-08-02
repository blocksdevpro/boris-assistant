//! Built-in agent tools and host registration helpers.
//!
//! Individual tools live in submodules; this module assembles the default set
//! and exposes a small registration surface for the host (`boris-pipeline` /
//! desktop).

pub mod clipboard;
pub mod files;
pub mod fs_common;
pub mod notes;
pub mod open_tool;
pub mod profile;
pub mod shell;
pub mod system;
pub mod time;
pub mod todo;
pub mod web;

use std::path::PathBuf;

use crate::engine::AgentEngine;
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

/// Sandboxed filesystem tools.
pub fn fs_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    let roots = paths.fs_roots();
    vec![
        Box::new(files::ListDirTool::new(roots.clone())),
        Box::new(files::ReadFileTool::new(roots.clone())),
        Box::new(files::WriteFileTool::new(roots)),
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

/// Shell tool (requires host shell policy OpenConfirm + HITL).
pub fn shell_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    let cwd_roots = paths.read_roots_flat();
    vec![Box::new(shell::RunCommandTool::new(
        cwd_roots,
        paths.sandbox_root.clone(),
    ))]
}

/// Register time + notes tools. Prefer [`register_builtin_tools`].
pub fn register_time_and_notes(engine: &mut AgentEngine, paths: &BuiltinToolPaths) {
    engine.register_tools(builtin_tools(paths));
}

/// Full host setup: personal context + all MVP tool waves (core, OS, fs, web, shell).
pub fn register_builtin_tools(engine: &mut AgentEngine, paths: BuiltinToolPaths) {
    register_builtin_tools_with_options(engine, paths, true, true);
}

/// Same as [`register_builtin_tools`] with control over LLM extract and power tools.
pub fn register_builtin_tools_with_options(
    engine: &mut AgentEngine,
    paths: BuiltinToolPaths,
    llm_extract: bool,
    power_tools: bool,
) {
    let mut tools = builtin_tools(&paths);

    match engine.enable_personal_context(&paths.profile_path, llm_extract) {
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
        tools.extend(shell_tools(&paths));
    }

    engine.register_tools(tools);
}
