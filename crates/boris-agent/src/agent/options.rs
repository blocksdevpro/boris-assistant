//! Construction options for [`super::Agent`].

use std::path::PathBuf;

use boris_ai::LlmClient;

use crate::runtime::SandboxConfig;
use crate::tool::Tool;

/// Construction options for [`super::Agent::from_options`].
pub struct AgentOptions {
    pub client: Box<dyn LlmClient>,
    pub system_prompt: String,
    pub max_tool_rounds: Option<u32>,
    pub tools: Vec<Box<dyn Tool>>,
    pub sandbox: Option<SandboxConfig>,
    pub audit_path: Option<PathBuf>,
    pub session_id: Option<String>,
    /// When true, Moderate tools auto-allow (see [`SandboxConfig::trusted_auto_moderate`]).
    pub trusted_auto_moderate: bool,
}
