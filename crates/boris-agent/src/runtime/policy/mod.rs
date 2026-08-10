//! Sandbox roots + allow/deny/confirm policy for tool invocation.
//!
//! # Security model (host + runtime)
//!
//! | Gate | Controlled by | Notes |
//! |------|---------------|--------|
//! | Path roots | [`SandboxConfig`] read/write roots | All path-like args; symlink-aware when possible |
//! | Shell | [`ShellPolicy`] | `Denied` / `Allowlist` / `OpenConfirm` |
//! | Network | [`NetworkPolicy`] | `Off` / `Allowlist` / `Open` |
//! | Risk / HITL | risk + `requires_confirmation` | User grant only skips the **confirm UI** — hard gates still run |
//!
//! [`NetworkPolicy::Open`] allows any host for tools with [`Permission::Network`],
//! but `web_fetch` still applies SSRF host blocks (loopback, private, metadata).
//! Prefer `Allowlist` when the product only needs known domains.
//!
//! # Module layout
//!
//! - [`paths`] — path normalization, root checks, multi-path arg collection

mod paths;

use std::path::PathBuf;

use serde_json::Value;

use crate::tool::{Permission, ToolMeta, ToolRisk};

pub use paths::{
    default_user_read_roots, normalize_path, path_is_within, resolve_in_roots,
    resolve_path_for_policy,
};
use paths::{args_path_strings, check_path_allowed, PathAccess};

/// Network access policy for tools that declare [`Permission::Network`].
///
/// - [`Off`](Self::Off): deny all network tools.
/// - [`Allowlist`](Self::Allowlist): only hosts matching entries (exact host or
///   DNS suffix, e.g. `example.com` allows `api.example.com`). Matched against
///   URL-like args (`url`, `uri`, `href`).
/// - [`Open`](Self::Open): any host at the policy layer; web tools still enforce
///   SSRF blocks on loopback/private/metadata. Policy may still force HITL for
///   network tools via risk / confirm flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPolicy {
    Off,
    Allowlist(Vec<String>),
    Open,
}

/// Shell execution policy for tools that declare [`Permission::Shell`].
///
/// - [`Denied`](Self::Denied): no shell tools.
/// - [`Allowlist`](Self::Allowlist): first argv token / command prefix must match
///   an entry (case-insensitive). Entries are binary names (`git`) or prefixes
///   (`git status`, `cargo `).
/// - [`OpenConfirm`](Self::OpenConfirm): shell allowed; risk/confirm still apply
///   (bash is Dangerous + confirm). Hard deny patterns in the bash tool remain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellPolicy {
    Denied,
    Allowlist(Vec<String>),
    OpenConfirm,
}

/// Host-injected sandbox + risk policy.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Default writable sandbox root (e.g. `~/.boris/state/workspace`).
    pub sandbox_root: PathBuf,
    /// Always-allowed Boris data roots (memory, sessions, workspace, …).
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
    /// is set, and auto-allow Dangerous sandbox-only `FsWrite` tools (file_write /
    /// file_edit under write roots). Shell, network, and Critical still confirm.
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
            max_confirms_per_turn: 12,
            trusted_auto_moderate: false,
        }
    }
}

impl SandboxConfig {
    /// Build a config rooted under a Boris home directory (closed network/shell).
    ///
    /// Layout matches Grok / pipeline defaults:
    /// - write root: `{home}/state/workspace`
    /// - data roots: memory, sessions, workspace
    pub fn for_boris_home(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let workspace = home.join("state").join("workspace");
        Self {
            sandbox_root: workspace.clone(),
            boris_data_roots: vec![
                home.join("memory"),
                home.join("sessions"),
                workspace,
            ],
            allow_read: vec![],
            allow_write: vec![],
            network: NetworkPolicy::Off,
            shell: ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 12,
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

    /// Enable trusted auto-allow for ≤ Moderate tools and Dangerous sandbox FsWrite.
    pub fn with_trusted_auto_moderate(mut self, on: bool) -> Self {
        self.trusted_auto_moderate = on;
        self
    }

    /// Cap HITL confirmations per user turn (multi-tool budget). Minimum 1.
    pub fn with_max_confirms_per_turn(mut self, n: u32) -> Self {
        self.max_confirms_per_turn = n.max(1);
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
///
/// **Hard gates** (shell/network/path allowlists and denials) always apply.
/// HITL confirmation is separate: after a user grant, the runtime sets
/// `skip_confirmation` only to bypass the NeedsConfirmation branch — it must
/// still call [`decide`] so hard gates remain authoritative.
pub fn decide(
    config: &SandboxConfig,
    meta: &ToolMeta,
    args: &Value,
    confirms_used: u32,
) -> PolicyDecision {
    // ── Hard gates (never skipped by HITL grant) ───────────────────────────
    if meta.permissions.contains(&Permission::Shell) {
        match &config.shell {
            ShellPolicy::Denied => {
                return PolicyDecision::Deny {
                    reason: "shell execution is disabled".into(),
                };
            }
            ShellPolicy::Allowlist(patterns) => {
                let Some(cmd) = args_command_string(args) else {
                    return PolicyDecision::Deny {
                        reason: "shell allowlist requires a `command` argument".into(),
                    };
                };
                if !shell_command_allowed(cmd, patterns) {
                    return PolicyDecision::Deny {
                        reason: format!(
                            "command not on shell allowlist (first token / prefix must match)"
                        ),
                    };
                }
            }
            ShellPolicy::OpenConfirm => {
                // Allowed at policy layer; risk/confirm still apply below.
            }
        }
    }

    if meta.permissions.contains(&Permission::Network) {
        match &config.network {
            NetworkPolicy::Off => {
                return PolicyDecision::Deny {
                    reason: "network access is disabled".into(),
                };
            }
            NetworkPolicy::Allowlist(hosts) => {
                if let Some(url) = args_url_string(args) {
                    match host_from_urlish(url) {
                        Some(host) if network_host_allowed(&host, hosts) => {}
                        Some(host) => {
                            return PolicyDecision::Deny {
                                reason: format!("host `{host}` is not on the network allowlist"),
                            };
                        }
                        None => {
                            return PolicyDecision::Deny {
                                reason: "could not parse host from network tool args".into(),
                            };
                        }
                    }
                }
                // No URL arg (e.g. web_search with only `query`): allowlist still
                // permits the tool; search backends are fixed by the tool impl.
            }
            NetworkPolicy::Open => {
                // Open at policy layer. web_fetch still applies SSRF host blocks.
            }
        }
    }

    // Path args: every path-like field must fall under allowed roots.
    let paths = args_path_strings(args);
    if !paths.is_empty() {
        let needs_write = meta.permissions.contains(&Permission::FsWrite);
        let needs_read = meta.permissions.contains(&Permission::FsRead) || needs_write;
        for path in &paths {
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
    }

    // ── Soft gates (confirmation / risk) ───────────────────────────────────
    // Trusted session: skip HITL for ≤ Moderate even when tool flags confirm.
    // Shell/network Dangerous+ still force confirm below (except sandbox writes).
    if config.trusted_auto_moderate
        && meta.risk <= ToolRisk::Moderate
        && meta.risk < config.force_confirm_at_or_above
        && !meta.permissions.contains(&Permission::Shell)
    {
        return PolicyDecision::Allow;
    }

    // Trusted session: auto-allow Dangerous (not Critical) sandbox file writes.
    // Path hard-gates above already ensure every path is under write roots.
    // Shell / network stay confirm; empty path args fall through to confirm.
    if config.trusted_auto_moderate
        && meta.risk == ToolRisk::Dangerous
        && meta.permissions.contains(&Permission::FsWrite)
        && !meta.permissions.contains(&Permission::Shell)
        && !meta.permissions.contains(&Permission::Network)
        && !paths.is_empty()
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

/// First non-empty command string from tool args.
pub(crate) fn args_command_string(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    for key in ["command", "cmd", "shell"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// URL-like field for network allowlist checks.
pub(crate) fn args_url_string(args: &Value) -> Option<&str> {
    let obj = args.as_object()?;
    for key in ["url", "uri", "href"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Extract host from `http(s)://host/...` or bare `host` / `host:port`.
pub(crate) fn host_from_urlish(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Only trust Url::parse when it yields an actual host (bare `host:port` can
    // parse as a weird scheme without host).
    if let Ok(u) = reqwest::Url::parse(s) {
        if let Some(h) = u.host_str() {
            return Some(h.trim_end_matches('.').to_ascii_lowercase());
        }
    }
    // Bare host or host:port without scheme.
    let no_path = s.split('/').next().unwrap_or(s);
    // Strip [ipv6]:port
    if let Some(inner) = no_path.strip_prefix('[') {
        let host = inner.split(']').next()?.to_ascii_lowercase();
        return if host.is_empty() { None } else { Some(host) };
    }
    let host = no_path
        .split(':')
        .next()
        .unwrap_or(no_path)
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Allowlist match: exact host or DNS suffix (`example.com` → `a.example.com`).
pub(crate) fn network_host_allowed(host: &str, allowlist: &[String]) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if h.is_empty() {
        return false;
    }
    for entry in allowlist {
        let e = normalize_allowlist_host(entry);
        if e.is_empty() {
            continue;
        }
        if h == e || h.ends_with(&format!(".{e}")) {
            return true;
        }
    }
    false
}

fn normalize_allowlist_host(entry: &str) -> String {
    let mut e = entry.trim().to_ascii_lowercase();
    if let Some(rest) = e.strip_prefix("https://") {
        e = rest.to_string();
    } else if let Some(rest) = e.strip_prefix("http://") {
        e = rest.to_string();
    }
    let e = e.split('/').next().unwrap_or(&e);
    // host:port → host (not for bare IPv6)
    if e.starts_with('[') {
        return e
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or(e)
            .trim_end_matches('.')
            .to_string();
    }
    e.split(':')
        .next()
        .unwrap_or(e)
        .trim_end_matches('.')
        .to_string()
}

/// Shell allowlist: match first token (binary) or full command prefix.
pub(crate) fn shell_command_allowed(command: &str, allowlist: &[String]) -> bool {
    let cmd = command.trim();
    if cmd.is_empty() || allowlist.is_empty() {
        return false;
    }
    let first = first_shell_token(cmd);
    let cmd_lower = cmd.to_ascii_lowercase();
    let first_lower = first.to_ascii_lowercase();
    // Strip Windows path / extension for binary compare.
    let first_bin = binary_basename(&first_lower);

    for pattern in allowlist {
        let p = pattern.trim();
        if p.is_empty() {
            continue;
        }
        let p_lower = p.to_ascii_lowercase();
        let p_bin = binary_basename(&p_lower);
        // Exact binary name match (git, cargo, ls).
        if first_bin == p_bin || first_lower == p_lower {
            return true;
        }
        // Prefix match on full command ("git status", "cargo ").
        if cmd_lower.starts_with(&p_lower) {
            return true;
        }
    }
    false
}

fn first_shell_token(cmd: &str) -> &str {
    let cmd = cmd.trim();
    // Skip env assignments FOO=bar
    let mut rest = cmd;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return "";
        }
        // Quoted binary path (handles spaces, e.g. "C:\Program Files\Git\cmd\git.exe").
        if let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            if let Some(end) = rest[1..].find(q) {
                return &rest[1..1 + end];
            }
        }
        let token = rest.split_whitespace().next().unwrap_or("");
        if token.contains('=')
            && !token.starts_with('-')
            && !token.contains('/')
            && !token.contains('\\')
        {
            rest = rest[token.len()..].trim_start();
            if rest.is_empty() {
                return token;
            }
            continue;
        }
        return token;
    }
}

fn binary_basename(token: &str) -> String {
    let t = token.trim_matches('"').trim_matches('\'');
    let base = t
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(t)
        .to_ascii_lowercase();
    // Drop .exe / .cmd / .bat on Windows-style names.
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(s) = base.strip_suffix(ext) {
            return s.to_string();
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Permission, ToolMeta, ToolRisk};
    use serde_json::json;
    use std::path::PathBuf;

    fn cfg() -> SandboxConfig {
        SandboxConfig {
            sandbox_root: PathBuf::from("C:\\Users\\me\\.boris\\state\\workspace"),
            boris_data_roots: vec![
                PathBuf::from("C:\\Users\\me\\.boris\\memory"),
                PathBuf::from("C:\\Users\\me\\.boris\\sessions"),
                PathBuf::from("C:\\Users\\me\\.boris\\state\\workspace"),
            ],
            allow_read: vec![PathBuf::from("C:\\Users\\me\\Documents")],
            allow_write: vec![],
            network: NetworkPolicy::Off,
            shell: ShellPolicy::Denied,
            auto_allow_up_to: ToolRisk::Moderate,
            force_confirm_at_or_above: ToolRisk::Dangerous,
            max_confirms_per_turn: 12,
            trusted_auto_moderate: false,
        }
    }

    #[test]
    fn for_boris_home_uses_state_workspace_layout() {
        let home = PathBuf::from(r"C:\Users\me\.boris");
        let c = SandboxConfig::for_boris_home(&home);
        let workspace = home.join("state").join("workspace");
        assert_eq!(c.sandbox_root, workspace);
        assert!(c.boris_data_roots.contains(&home.join("memory")));
        assert!(c.boris_data_roots.contains(&home.join("sessions")));
        assert!(c.boris_data_roots.contains(&workspace));
    }

    #[test]
    fn relative_path_write_allowed_by_decide() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::FsWrite]);
        let d = decide(&cfg(), &meta, &json!({ "path": "note.txt" }), 0);
        assert_eq!(d, PolicyDecision::Allow);
    }

    #[test]
    fn relative_path_escape_denied_by_decide() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::FsWrite]);
        let d = decide(&cfg(), &meta, &json!({ "path": "../outside.txt" }), 0);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
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
    fn shell_allowlist_allows_and_denies() {
        let mut c = cfg();
        c.shell = ShellPolicy::Allowlist(vec!["git".into(), "cargo test".into()]);
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::Shell])
            .confirm(true);
        let allow = decide(&c, &meta, &json!({ "command": "git status" }), 0);
        assert!(matches!(allow, PolicyDecision::NeedsConfirmation { .. }));
        let allow2 = decide(&c, &meta, &json!({ "command": "cargo test -p foo" }), 0);
        assert!(matches!(allow2, PolicyDecision::NeedsConfirmation { .. }));
        let deny = decide(&c, &meta, &json!({ "command": "rm -rf /" }), 0);
        assert!(matches!(deny, PolicyDecision::Deny { reason } if reason.contains("allowlist")));
    }

    #[test]
    fn network_off_denies() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::Network]);
        let d = decide(&cfg(), &meta, &json!({ "url": "https://example.com" }), 0);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn network_allowlist_host_match() {
        let mut c = cfg();
        c.network = NetworkPolicy::Allowlist(vec!["example.com".into(), "api.github.com".into()]);
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::Network]);
        let ok = decide(
            &c,
            &meta,
            &json!({ "url": "https://docs.example.com/a" }),
            0,
        );
        assert_eq!(ok, PolicyDecision::Allow);
        let ok2 = decide(
            &c,
            &meta,
            &json!({ "url": "https://api.github.com/repos" }),
            0,
        );
        assert_eq!(ok2, PolicyDecision::Allow);
        let bad = decide(
            &c,
            &meta,
            &json!({ "url": "https://evil.example.org/" }),
            0,
        );
        assert!(matches!(bad, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn network_open_allows() {
        let mut c = cfg();
        c.network = NetworkPolicy::Open;
        let meta = ToolMeta::with_risk(ToolRisk::Moderate).permissions(&[Permission::Network]);
        let d = decide(&c, &meta, &json!({ "url": "https://example.com" }), 0);
        assert_eq!(d, PolicyDecision::Allow);
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
    fn multi_path_any_outside_denied() {
        let meta = ToolMeta::with_risk(ToolRisk::Moderate)
            .permissions(&[Permission::FsRead, Permission::FsWrite]);
        let d = decide(
            &cfg(),
            &meta,
            &json!({
                "source": "C:\\Users\\me\\.boris\\state\\workspace\\a.txt",
                "dest": "C:\\Windows\\evil.txt"
            }),
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
            &json!({ "path": "C:\\Users\\me\\.boris\\state\\workspace\\note.txt" }),
            0,
        );
        assert_eq!(d, PolicyDecision::Allow);
    }

    #[test]
    fn confirm_cap_denies() {
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous);
        // Intentionally exercise the limit with a low cap.
        let mut c = cfg();
        c.max_confirms_per_turn = 3;
        let d = decide(&c, &meta, &json!({}), 3);
        assert!(matches!(d, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn trusted_auto_allows_dangerous_sandbox_write() {
        let mut c = cfg();
        c.trusted_auto_moderate = true;
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::FsWrite])
            .confirm(true);
        let d = decide(
            &c,
            &meta,
            &json!({ "path": "C:\\Users\\me\\.boris\\state\\workspace\\note.txt" }),
            0,
        );
        assert_eq!(d, PolicyDecision::Allow);
    }

    #[test]
    fn trusted_still_confirms_shell() {
        let mut c = cfg();
        c.trusted_auto_moderate = true;
        c.shell = ShellPolicy::OpenConfirm;
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::Shell])
            .confirm(true);
        let d = decide(&c, &meta, &json!({ "command": "echo hi" }), 0);
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn trusted_off_confirms_dangerous_sandbox_write() {
        let c = cfg(); // trusted_auto_moderate = false
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::FsWrite])
            .confirm(true);
        let d = decide(
            &c,
            &meta,
            &json!({ "path": "C:\\Users\\me\\.boris\\state\\workspace\\note.txt" }),
            0,
        );
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn trusted_does_not_auto_allow_critical_write() {
        let mut c = cfg();
        c.trusted_auto_moderate = true;
        let meta = ToolMeta::with_risk(ToolRisk::Critical)
            .permissions(&[Permission::FsWrite])
            .confirm(true);
        let d = decide(
            &c,
            &meta,
            &json!({ "path": "C:\\Users\\me\\.boris\\state\\workspace\\note.txt" }),
            0,
        );
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn trusted_dangerous_write_without_path_falls_through() {
        let mut c = cfg();
        c.trusted_auto_moderate = true;
        let meta = ToolMeta::with_risk(ToolRisk::Dangerous)
            .permissions(&[Permission::FsWrite])
            .confirm(true);
        let d = decide(&c, &meta, &json!({}), 0);
        assert!(matches!(d, PolicyDecision::NeedsConfirmation { .. }));
    }

    #[test]
    fn shell_command_allowed_helpers() {
        let list = vec!["git".into(), "cargo build".into()];
        assert!(shell_command_allowed("git status", &list));
        assert!(shell_command_allowed("GIT status", &list));
        assert!(shell_command_allowed("cargo build --release", &list));
        assert!(!shell_command_allowed("cargo test", &list));
        assert!(!shell_command_allowed("rm -rf /", &list));
        assert!(shell_command_allowed(
            r#""C:\Program Files\Git\cmd\git.exe" status"#,
            &list
        ));
    }

    #[test]
    fn host_from_urlish_parses() {
        assert_eq!(
            host_from_urlish("https://API.Example.COM/x"),
            Some("api.example.com".into())
        );
        assert_eq!(host_from_urlish("example.com:443"), Some("example.com".into()));
    }
}
