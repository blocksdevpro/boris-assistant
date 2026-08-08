//! Sandbox roots + allow/deny/confirm policy for tool invocation.
//!
//! # Module layout
//!
//! - [`paths`] — path normalization, root checks, user read roots

mod paths;

use std::path::PathBuf;

use serde_json::Value;

use crate::tool::{Permission, ToolMeta, ToolRisk};

pub use paths::{default_user_read_roots, normalize_path, path_is_within, resolve_in_roots};
use paths::{args_path_string, check_path_allowed, PathAccess};

/// Network access policy (no tools use network yet; defaults stay closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    Allowlist(Vec<String>),
    /// Open network; policy will still force confirmation for network tools.
    Open,
}

/// Shell execution policy (no shell tool yet; default denied).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellPolicy {
    Denied,
    Allowlist(Vec<String>),
    /// Shell allowed but always confirms (Dangerous).
    OpenConfirm,
}

/// Host-injected sandbox + risk policy.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Default writable sandbox root (e.g. `~/.boris/sandbox`).
    pub sandbox_root: PathBuf,
    /// Always-allowed Boris data roots (memory, sessions, …).
    pub boris_data_roots: Vec<PathBuf>,
    /// Extra user-granted read roots.
    pub allow_read: Vec<PathBuf>,
    /// Extra user-granted write roots.
    pub allow_write: Vec<PathBuf>,
    pub network: NetworkPolicy,
    pub shell: ShellPolicy,
    /// Risks at or below this level auto-allow (unless `requires_confirmation`).
    pub auto_allow_up_to: ToolRisk,
    /// Risks at or above this level always need confirmation.
    pub force_confirm_at_or_above: ToolRisk,
    /// Max HITL confirmations per user turn before remaining calls are denied.
    pub max_confirms_per_turn: u32,
    /// When true, auto-allow tools up to Moderate even if `requires_confirmation`
    /// is set — still force-confirm Dangerous/Critical (trusted session / YOLO-lite).
    pub trusted_auto_moderate: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        // Neutral defaults for unit tests; host should inject real paths.
        Self {
            sandbox_root: PathBuf::from(".boris-sandbox"),
            boris_data_roots: vec![],
            allow_read: vec![],
            allow_write: vec![],
            network: NetworkPolicy::Off,
            shell: ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 3,
            trusted_auto_moderate: false,
        }
    }
}

impl SandboxConfig {
    /// Build a config rooted under a Boris home directory (closed network/shell).
    pub fn for_boris_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            sandbox_root: home.join("sandbox"),
            boris_data_roots: vec![home.join("memory"), home.join("sessions")],
            allow_read: vec![],
            allow_write: vec![],
            network: NetworkPolicy::Off,
            shell: ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 3,
            trusted_auto_moderate: false,
        }
    }

    /// Desktop MVP defaults: user read folders, open network, shell with confirm.
    pub fn for_desktop_mvp(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let mut cfg = Self::for_boris_home(&home);
        cfg.network = NetworkPolicy::Open;
        cfg.shell = ShellPolicy::OpenConfirm;
        cfg.allow_read = default_user_read_roots();
        // Writable only via sandbox_root (+ boris_data_roots for memory tools).
        cfg.allow_write = vec![];
        cfg
    }

    /// Enable trusted auto-allow for Moderate tools (sandbox writes, notes, clipboard…).
    pub fn with_trusted_auto_moderate(mut self, on: bool) -> Self {
        self.trusted_auto_moderate = on;
        self
    }
}

/// Result of policy evaluation before tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
    NeedsConfirmation { reason: String },
}

/// Decide whether a tool may run given its metadata and args.
///
/// `confirms_used` counts prior HITL pauses in this turn (including none).
pub fn decide(
    config: &SandboxConfig,
    meta: &ToolMeta,
    args: &Value,
    confirms_used: u32,
) -> PolicyDecision {
    // Permission gates that are closed by default.
    if meta.permissions.contains(&Permission::Shell) {
        match &config.shell {
            ShellPolicy::Denied => {
                return PolicyDecision::Deny {
                    reason: "shell execution is disabled".into(),
                };
            }
            ShellPolicy::Allowlist(_) | ShellPolicy::OpenConfirm => {
                // Allowlist matching is applied when a shell tool exists.
            }
        }
    }

    if meta.permissions.contains(&Permission::Network) {
        if matches!(config.network, NetworkPolicy::Off) {
            return PolicyDecision::Deny {
                reason: "network access is disabled".into(),
            };
        }
    }

    // Path args: if present, must fall under allowed roots for the needed mode.
    if let Some(path) = args_path_string(args) {
        let needs_write = meta.permissions.contains(&Permission::FsWrite);
        let needs_read = meta.permissions.contains(&Permission::FsRead) || needs_write;
        if needs_write {
            if let Err(reason) = check_path_allowed(config, path, PathAccess::Write) {
                return PolicyDecision::Deny { reason };
            }
        } else if needs_read {
            if let Err(reason) = check_path_allowed(config, path, PathAccess::Read) {
                return PolicyDecision::Deny { reason };
            }
        }
    }

    // Trusted session: skip HITL for ≤ Moderate even when tool flags confirm.
    // Shell/network Dangerous+ still force confirm below.
    if config.trusted_auto_moderate
        && meta.risk <= ToolRisk::Moderate
        && meta.risk < config.force_confirm_at_or_above
        && !meta.permissions.contains(&Permission::Shell)
    {
        return PolicyDecision::Allow;
    }

    if meta.requires_confirmation || meta.risk >= config.force_confirm_at_or_above {
        if confirms_used >= config.max_confirms_per_turn {
            return PolicyDecision::Deny {
                reason: format!(
                    "confirmation limit ({}) reached for this turn",
                    config.max_confirms_per_turn
                ),
            };
        }
        return PolicyDecision::NeedsConfirmation {
            reason: if meta.requires_confirmation {
                "tool requires confirmation".into()
            } else {
                format!("risk {} requires confirmation", meta.risk.as_str())
            },
        };
    }

    if meta.risk <= config.auto_allow_up_to {
        return PolicyDecision::Allow;
    }

    // Between auto_allow and force_confirm: treat as confirm.
    if confirms_used >= config.max_confirms_per_turn {
        return PolicyDecision::Deny {
            reason: format!(
                "confirmation limit ({}) reached for this turn",
                config.max_confirms_per_turn
            ),
        };
    }
    PolicyDecision::NeedsConfirmation {
        reason: format!("risk {} needs approval", meta.risk.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Permission, ToolMeta, ToolRisk};
    use serde_json::json;
    use std::path::PathBuf;

    fn cfg() -> SandboxConfig {
        SandboxConfig {
            sandbox_root: PathBuf::from("C:\\Users\\me\\.boris\\sandbox"),
            boris_data_roots: vec![PathBuf::from("C:\\Users\\me\\.boris\\memory")],
            allow_read: vec![PathBuf::from("C:\\Users\\me\\Documents")],
            allow_write: vec![],
            network: NetworkPolicy::Off,
            shell: ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 3,
            trusted_auto_moderate: false,
        }
    }

    #[test]
    fn safe_auto_allows() {
        let meta = ToolMeta::safe_default();
        let d = decide(&cfg(), &meta, &json!({}), 0);
        assert_eq!(d, PolicyDecision::Allow);
    }

    #[test]
    fn dangerous_needs_confirm() {
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous);
        let d = decide(&cfg(), &meta, &json!({}), 0);
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn requires_confirmation_flag() {
        let meta = ToolMeta::safe_default().confirm(true);
        let d = decide(&cfg(), &meta, &json!({}), 0);
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn shell_denied() {
        // Shell permission with Denied policy fails closed even at Safe risk.
        let meta = ToolMeta {
            risk: ToolRisk::Safe,
            permissions: &[Permission::Shell],
            default_timeout: ToolRisk::Safe.default_timeout(),
            requires_confirmation: false,
            kind: crate::tool::ToolKind::Execute,
            max_result_chars: None,
            read_only: Some(false),
            max_concurrency: Some(1),
        };
        let d = decide(&cfg(), &meta, &json!({}), 0);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn path_outside_denied() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::FsRead]);
        let d = decide(
            &cfg(),
            &meta,
            &json!({ "path": "C:\\Windows\\System32\\config" }),
            0,
        );
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn path_in_sandbox_ok() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::FsWrite]);
        let d = decide(
            &cfg(),
            &meta,
            &json!({ "path": "C:\\Users\\me\\.boris\\sandbox\\note.txt" }),
            0,
        );
        assert_eq!(d, PolicyDecision::Allow);
    }

    #[test]
    fn confirm_cap_denies() {
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous);
        let d = decide(&cfg(), &meta, &json!({}), 3);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }
}
