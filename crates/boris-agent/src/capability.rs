//! Tool kinds + capability presets (Grok toolset filtering, voice-sized).

use crate::runtime::{NetworkPolicy, SandboxConfig, ShellPolicy};
use crate::tool::{Tool, ToolKind, ToolRisk};

/// How much of the tool surface the host exposes to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityPreset {
    /// Safe / moderate local facts only (time, notes, profile, skills, todos, cards).
    /// No shell, network, or arbitrary filesystem writes outside memory/sandbox plan files.
    VoiceSafe,
    /// Sandboxed files + OS helpers; still no shell / network until confirmed host opts in.
    ///
    /// Intentional: `LocalPower` still allows `Dangerous` file writes — they
    /// are confined to sandbox write roots by policy hard-gates and still go
    /// through HITL per risk — while `Execute` (bash) and `Web` kinds are
    /// filtered out and network/shell policy is forced closed. Use `Full`
    /// when the host explicitly opts into shell/web with HITL.
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
    /// - [`VoiceSafe`](Self::VoiceSafe): core + profile only (no power wave).
    /// - [`LocalPower`](Self::LocalPower) / [`Full`](Self::Full): register power
    ///   tools, then [`allows_tool`](Self::allows_tool) drops shell/web as needed.
    pub fn wants_power_tools(self) -> bool {
        !matches!(self, Self::VoiceSafe)
    }

    /// Adjust sandbox network/shell to match the preset (defense in depth).
    ///
    /// # Call order (load-bearing)
    ///
    /// Call this (or, preferably, `register_builtin_tools_with_preset`, which
    /// calls it internally) **before** `Agent::configure_runtime`, passing the
    /// same `&mut SandboxConfig` into both. `configure_runtime` snapshots the
    /// policy into the runtime; calling it first and adjusting the sandbox
    /// afterwards leaves the runtime with the un-adjusted policy (e.g.
    /// `VoiceSafe` tools registered but network/shell still open).
    ///
    /// Prefer [`CapabilityPreset::apply_to_sandbox_owned`] when the call site
    /// can move the config by value: it returns the adjusted config, making
    /// the order visible at the type level (`let cfg =
    /// preset.apply_to_sandbox_owned(cfg); agent.configure_runtime(cfg, …)`).
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
        debug_assert!(
            !matches!(self, Self::VoiceSafe | Self::LocalPower)
                || matches!(cfg.network, NetworkPolicy::Off)
                    && matches!(cfg.shell, ShellPolicy::Denied),
            "apply_to_sandbox must leave VoiceSafe/LocalPower with network Off + shell Denied"
        );
    }

    /// Owned variant of [`CapabilityPreset::apply_to_sandbox`] that enforces
    /// the load-bearing order at the type level.
    ///
    /// Takes the sandbox by value and returns it adjusted, so the caller is
    /// forced to thread the result into `Agent::configure_runtime`:
    /// `let cfg = preset.apply_to_sandbox_owned(cfg); agent.configure_runtime(cfg, …)`.
    /// With the `&mut` form it is easy to configure the runtime first and
    /// adjust afterwards (leaving the runtime with the stale policy); this
    /// form makes that mistake visible. In debug builds the post-condition
    /// (VoiceSafe/LocalPower ⇒ Off/Denied) is `debug_assert`ed.
    #[must_use = "pass the returned SandboxConfig into Agent::configure_runtime"]
    pub fn apply_to_sandbox_owned(self, mut cfg: SandboxConfig) -> SandboxConfig {
        self.apply_to_sandbox(&mut cfg);
        cfg
    }

    /// Whether a tool may be listed / registered under this preset.
    ///
    /// `LocalPower` intentionally keeps `Dangerous` file writes (kind
    /// `Write`, e.g. `file_write` under sandbox roots) while blocking
    /// `Execute`/`Web`: writes stay policy-confined + HITL-gated, whereas
    /// shell/network would escape the sandbox. See the `LocalPower` variant
    /// docs.
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

    #[test]
    fn wants_power_tools_by_preset() {
        assert!(!CapabilityPreset::VoiceSafe.wants_power_tools());
        assert!(CapabilityPreset::LocalPower.wants_power_tools());
        assert!(CapabilityPreset::Full.wants_power_tools());
    }

    #[test]
    fn local_power_allows_dangerous_writes_but_blocks_bash_web() {
        use crate::tool::ToolKind;
        // Intentional: Dangerous sandbox writes stay available under
        // LocalPower (policy roots + HITL still apply); shell/web do not.
        let write = Dummy {
            name: "file_write",
            kind: ToolKind::Write,
            risk: ToolRisk::Dangerous,
        };
        let bash = Dummy {
            name: "bash",
            kind: ToolKind::Execute,
            risk: ToolRisk::Dangerous,
        };
        let web = Dummy {
            name: "web_fetch",
            kind: ToolKind::Web,
            risk: ToolRisk::Moderate,
        };
        assert!(
            CapabilityPreset::LocalPower.allows_tool(&write),
            "LocalPower must keep Dangerous writes (sandbox-confined)"
        );
        assert!(!CapabilityPreset::LocalPower.allows_tool(&bash));
        assert!(!CapabilityPreset::LocalPower.allows_tool(&web));
    }

    #[test]
    fn apply_to_sandbox_order_owned_threading() {
        use crate::runtime::SandboxConfig;
        // The owned wrapper must apply the same lockdown as the &mut form, and
        // force callers to thread the result (e.g. into configure_runtime).
        let cfg = SandboxConfig::for_desktop_mvp(std::path::PathBuf::from("/tmp/boris-home"));
        assert_eq!(cfg.network, crate::runtime::NetworkPolicy::Open);
        let cfg = CapabilityPreset::LocalPower.apply_to_sandbox_owned(cfg);
        assert_eq!(cfg.network, crate::runtime::NetworkPolicy::Off);
        assert_eq!(cfg.shell, crate::runtime::ShellPolicy::Denied);
        // Full leaves host-opened policy alone.
        let cfg = SandboxConfig::for_desktop_mvp(std::path::PathBuf::from("/tmp/boris-home"));
        let cfg = CapabilityPreset::Full.apply_to_sandbox_owned(cfg);
        assert_eq!(cfg.network, crate::runtime::NetworkPolicy::Open);
    }
}
