//! Built-in agent tools (v1) and host registration helpers.
//!
//! Individual tools live in submodules; this module assembles the default set
//! and exposes a small registration surface for the host (`boris-pipeline` /
//! desktop).

pub mod notes;
pub mod time;

use std::path::PathBuf;

use crate::engine::AgentEngine;
use crate::tool::Tool;

/// Paths for file-backed tools.
pub struct BuiltinToolPaths {
    pub notes_path: PathBuf,
}

/// Build the default v1 tool set: `get_time`, `get_date`, `remember_note`, `recall_notes`.
pub fn builtin_tools(paths: BuiltinToolPaths) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(time::GetTimeTool),
        Box::new(time::GetDateTool),
        Box::new(notes::RememberNoteTool::new(paths.notes_path.clone())),
        Box::new(notes::RecallNotesTool::new(paths.notes_path)),
    ]
}

/// Register all builtin tools onto an engine.
pub fn register_builtin_tools(engine: &mut AgentEngine, paths: BuiltinToolPaths) {
    engine.register_tools(builtin_tools(paths));
}
