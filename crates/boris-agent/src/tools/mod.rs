//! Built-in agent tools (v1) and host registration helpers.
//!
//! Individual tools live in submodules; this module assembles the default set
//! and exposes a small registration surface for the host (`boris-pipeline` /
//! desktop).

pub mod notes;
pub mod profile;
pub mod time;

use std::path::PathBuf;

use crate::engine::AgentEngine;
use crate::tool::Tool;

/// Paths for file-backed tools.
pub struct BuiltinToolPaths {
    pub notes_path: PathBuf,
    /// Durable personal profile JSON (`~/.boris/memory/profile.json`).
    pub profile_path: PathBuf,
}

/// Build the default v1 tool set (time, notes). Profile tools need a shared handle.
pub fn builtin_tools(paths: &BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(time::GetTimeTool),
        Box::new(time::GetDateTool),
        Box::new(notes::RememberNoteTool::new(paths.notes_path.clone())),
        Box::new(notes::RecallNotesTool::new(paths.notes_path.clone())),
    ]
}

/// Register time + notes tools. Prefer [`register_builtin_tools`] which also
/// enables personal context + profile tools.
pub fn register_time_and_notes(engine: &mut AgentEngine, paths: &BuiltinToolPaths) {
    engine.register_tools(builtin_tools(paths));
}

/// Full host setup: enable personal context, register all builtin tools.
///
/// - Loads/saves profile at `paths.profile_path`
/// - Injects `<personal_context>` into the system prompt every turn
/// - Actively extracts facts after turns (`llm_extract = true`)
/// - Registers time, notes, and profile tools
pub fn register_builtin_tools(engine: &mut AgentEngine, paths: BuiltinToolPaths) {
    register_builtin_tools_with_options(engine, paths, true);
}

/// Same as [`register_builtin_tools`] with control over post-turn LLM extract.
pub fn register_builtin_tools_with_options(
    engine: &mut AgentEngine,
    paths: BuiltinToolPaths,
    llm_extract: bool,
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

    engine.register_tools(tools);
}
