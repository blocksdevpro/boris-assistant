//! Tool kinds + capability presets (Grok toolset filtering, voice-sized).

use crate::runtime::{NetworkPolicy, SandboxConfig, ShellPolicy};
use crate::tool::{Tool, ToolKind, ToolRisk};

/// How much of the tool surface the host exposes to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityPreset {
    /// Safe / moderate local facts only (time, notes, profile, skills list).
    /// No shell, network, or arbitrary filesystem writes outside memory.
    VoiceSafe,
    /// Sandboxed files + OS helpers; still no shell / network until confirmed host opts in.
    LocalPower,
    /// Full MVP tool suite (shell + web included; runtime still enforces HITL).
    #[default]
    Full,
}

impl CapabilityPreset {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VoiceSafe => "voice_safe",
            Self::LocalPower => "local_power",
            Self::Full => "full",
        }
    }

    /// Parse `voice_safe` / `local_power` / `full` (case-insensitive). Unknown → `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "voice_safe" | "voicesafe" | "safe" | "lite" => Some(Self::VoiceSafe),
            "local_power" | "localpower" | "local" => Some(Self::LocalPower),
            "full" | "power" | "mvp" => Some(Self::Full),
            _ => None,
        }
    }

    /// Whether power-tool waves (os/fs/web/bash) are registered before kind filtering.
    ///
    /// Always `true`: the preset filter drops disallowed tools. Core tools are
    /// always registered first; this only controls the power waves.
    pub fn wants_power_tools(self) -> bool {
        true
    }

    /// Adjust sandbox network/shell to match the preset (defense in depth).
    pub fn apply_to_sandbox(self, cfg: &mut SandboxConfig) {
        match self {
            Self::VoiceSafe => {
                cfg.network = NetworkPolicy::Off;
                cfg.shell = ShellPolicy::Denied;
            }
            Self::LocalPower => {
                cfg.network = NetworkPolicy::Off;
                cfg.shell = ShellPolicy::Denied;
            }
            Self::Full => {
                // Host may already have opened network/shell for desktop MVP.
            }
        }
    }

    /// Whether a tool may be listed / registered under this preset.
    pub fn allows_tool(self, tool: &dyn Tool) -> bool {
        let meta = tool.meta();
        match self {
            Self::Full => true,
            Self::VoiceSafe => {
                meta.risk <= ToolRisk::Moderate
                    && matches!(
                        meta.kind,
                        ToolKind::System
                            | ToolKind::Memory
                            | ToolKind::Skill
                            | ToolKind::Plan
                            | ToolKind::Other
                    )
            }
            Self::LocalPower => {
                // Block shell + network kinds; allow reads/writes in sandbox via policy.
                !matches!(meta.kind, ToolKind::Execute | ToolKind::Web)
            }
        }
    }
}

/// Drop tools the preset does not allow (preserves order).
pub fn filter_tools_for_preset(
    tools: Vec<Box<dyn Tool>>,
    preset: CapabilityPreset,
) -> Vec<Box<dyn Tool>> {
    tools
        .into_iter()
        .filter(|t| preset.allows_tool(t.as_ref()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Permission, ToolError, ToolMeta};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct Dummy {
        name: &'static str,
        kind: ToolKind,
        risk: ToolRisk,
    }

    #[async_trait]
    impl Tool for Dummy {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "d"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::with_risk(self.risk)
                .kind(self.kind)
                .permissions(&[Permission::None])
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            Ok("ok".into())
        }
    }

    #[test]
    fn voice_safe_blocks_shell_and_web() {
        let bash = Dummy {
            name: "bash",
            kind: ToolKind::Execute,
            risk: ToolRisk::Dangerous,
        };
        let time = Dummy {
            name: "get_time",
            kind: ToolKind::System,
            risk: ToolRisk::Safe,
        };
        assert!(!CapabilityPreset::VoiceSafe.allows_tool(&bash));
        assert!(CapabilityPreset::VoiceSafe.allows_tool(&time));
        assert!(CapabilityPreset::Full.allows_tool(&bash));
    }

    #[test]
    fn local_power_blocks_network_not_read() {
        let web = Dummy {
            name: "web_search",
            kind: ToolKind::Web,
            risk: ToolRisk::Dangerous,
        };
        let read = Dummy {
            name: "read_file",
            kind: ToolKind::Read,
            risk: ToolRisk::Safe,
        };
        assert!(!CapabilityPreset::LocalPower.allows_tool(&web));
        assert!(CapabilityPreset::LocalPower.allows_tool(&read));
    }

    #[test]
    fn parse_aliases() {
        assert_eq!(
            CapabilityPreset::parse("voice_safe"),
            Some(CapabilityPreset::VoiceSafe)
        );
        assert_eq!(
            CapabilityPreset::parse("FULL"),
            Some(CapabilityPreset::Full)
        );
        assert_eq!(CapabilityPreset::parse("nope"), None);
    }
}
