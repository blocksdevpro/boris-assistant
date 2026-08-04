//! Sandbox roots + allow/deny/confirm policy for tool invocation.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::tool::{Permission, ToolMeta, ToolRisk};

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
}

/// Common Windows/user document folders for read-only file tools.
pub fn default_user_read_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let user = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    if let Some(home) = user {
        for name in ["Desktop", "Documents", "Downloads"] {
            let p = home.join(name);
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    roots
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

#[derive(Debug, Clone, Copy)]
enum PathAccess {
    Read,
    Write,
}

fn args_path_string(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    for key in [
        "path",
        "file",
        "filepath",
        "file_path",
        "dir",
        "directory",
        "cwd",
    ] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn check_path_allowed(
    config: &SandboxConfig,
    raw: &str,
    access: PathAccess,
) -> Result<(), String> {
    let resolved = normalize_path(Path::new(raw))?;
    let roots: Vec<PathBuf> = match access {
        PathAccess::Read => {
            let mut r = Vec::new();
            r.push(config.sandbox_root.clone());
            r.extend(config.boris_data_roots.iter().cloned());
            r.extend(config.allow_read.iter().cloned());
            r.extend(config.allow_write.iter().cloned());
            r
        }
        PathAccess::Write => {
            let mut r = Vec::new();
            r.push(config.sandbox_root.clone());
            r.extend(config.boris_data_roots.iter().cloned());
            r.extend(config.allow_write.iter().cloned());
            r
        }
    };

    for root in &roots {
        let root_n = normalize_path(root).unwrap_or_else(|_| root.clone());
        if path_is_within(&resolved, &root_n) {
            return Ok(());
        }
    }

    Err(format!(
        "path `{}` is outside allowed roots",
        resolved.display()
    ))
}

/// Normalize a path without requiring it to exist (no symlink resolve).
///
/// Rejects empty paths and keeps `..` from escaping via component folding.
pub fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }

    let mut out = PathBuf::new();
    // Preserve absolute prefix (drive / root).
    for (i, comp) in path.components().enumerate() {
        match comp {
            Component::Prefix(p) => {
                if i == 0 {
                    out.push(p.as_os_str());
                }
            }
            Component::RootDir => {
                out.push(comp.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err("path escapes with `..`".into());
                }
            }
            Component::Normal(s) => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("path resolved empty".into());
    }
    Ok(out)
}

/// True if `path` is equal to `root` or a descendant (component-wise).
pub fn path_is_within(path: &Path, root: &Path) -> bool {
    let path_c: Vec<_> = path.components().collect();
    let root_c: Vec<_> = root.components().collect();
    if root_c.is_empty() {
        return false;
    }
    if path_c.len() < root_c.len() {
        return false;
    }
    path_c
        .iter()
        .zip(root_c.iter())
        .all(|(a, b)| a.as_os_str() == b.as_os_str())
}

/// Public helper for future file tools: resolve `raw` under write/read roots.
pub fn resolve_in_roots(
    config: &SandboxConfig,
    raw: &str,
    write: bool,
) -> Result<PathBuf, String> {
    let access = if write {
        PathAccess::Write
    } else {
        PathAccess::Read
    };
    check_path_allowed(config, raw, access)?;
    normalize_path(Path::new(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Permission, ToolMeta, ToolRisk};
    use serde_json::json;

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
    fn parent_escape_rejected() {
        let err = normalize_path(Path::new("C:\\Users\\me\\.boris\\sandbox\\..\\..\\Windows"));
        // Folded path may still normalize; path_is_within should fail.
        let n = err.expect("normalize folds ..");
        let root = PathBuf::from("C:\\Users\\me\\.boris\\sandbox");
        assert!(!path_is_within(&n, &root));
    }

    #[test]
    fn confirm_cap_denies() {
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous);
        let d = decide(&cfg(), &meta, &json!({}), 3);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }
}
