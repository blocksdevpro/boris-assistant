//! Local system information (no network).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::tool::{truncate_tool_result, Tool, ToolError, ToolKind, ToolMeta, ToolRisk};

/// Returns a short snapshot of the host machine.
#[derive(Debug, Clone)]
pub struct GetSystemInfoTool {
    boris_home: String,
}

impl GetSystemInfoTool {
    pub fn new(boris_home: impl Into<String>) -> Self {
        Self {
            boris_home: boris_home.into(),
        }
    }
}

#[async_trait]
impl Tool for GetSystemInfoTool {
    fn name(&self) -> &str {
        "get_system_info"
    }

    fn description(&self) -> &str {
        "Get local system info: OS, architecture, username, hostname, home directory, Boris home, and process cwd. Use for \"what machine am I on\"."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe).kind(ToolKind::System)
    }

    async fn execute(&self, _ctx: &crate::tool_context::ToolCallContext, _args: Value) -> Result<String, ToolError> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let username = std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "(unknown)".into());
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "(unknown)".into());
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "(unknown)".into());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(unknown)".into());

        let s = format!(
            "OS: {os}\nArch: {arch}\nUser: {username}\nHost: {hostname}\nHome: {home}\nBoris home: {}\nCwd: {cwd}",
            self.boris_home
        );
        Ok(truncate_tool_result(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn system_info_ok() {
        let t = GetSystemInfoTool::new("/tmp/.boris");
        let out = t.execute(&crate::tool_context::ToolCallContext::new("t"), json!({})).await.unwrap();
        assert!(out.contains("OS:"));
        assert!(out.contains("Boris home:"));
    }
}
