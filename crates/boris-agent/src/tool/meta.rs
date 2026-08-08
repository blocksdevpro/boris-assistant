//! Static tool metadata for policy, timeout, and concurrency.

use std::time::Duration;

use super::output::MAX_TOOL_RESULT_CHARS;

/// High-level tool category (used for capability presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ToolKind {
    /// Local read (files, notes recall).
    Read,
    /// Local write.
    Write,
    /// Search / grep / glob.
    Search,
    /// Shell / process execution.
    Execute,
    /// Network.
    Web,
    /// Personal / long-term memory.
    Memory,
    /// Skill playbooks.
    Skill,
    /// System info / clipboard / open.
    System,
    /// Planning / todos.
    Plan,
    /// Unclassified.
    #[default]
    Other,
}

impl ToolKind {
    /// Stable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Search => "search",
            Self::Execute => "execute",
            Self::Web => "web",
            Self::Memory => "memory",
            Self::Skill => "skill",
            Self::System => "system",
            Self::Plan => "plan",
            Self::Other => "other",
        }
    }

    /// True when the tool does not mutate external state by design.
    pub fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::Read | Self::Search | Self::Memory | Self::Skill | Self::System | Self::Other
        )
    }
}

/// How dangerous a tool is for policy and HITL defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolRisk {
    /// Read-only local facts (time, recall notes, get profile).
    Safe = 0,
    /// Local durable writes in Boris data (notes, profile updates).
    Moderate = 1,
    /// External or mutable side effects (shell, write outside memory, open URL).
    Dangerous = 2,
    /// Irreversible / high-impact (delete, send, admin) — always confirm.
    Critical = 3,
}

impl ToolRisk {
    /// Stable lowercase label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Moderate => "moderate",
            Self::Dangerous => "dangerous",
            Self::Critical => "critical",
        }
    }

    /// Default wall-clock budget for tools at this risk level.
    pub fn default_timeout(self) -> Duration {
        match self {
            Self::Safe => Duration::from_secs(5),
            Self::Moderate => Duration::from_secs(15),
            Self::Dangerous | Self::Critical => Duration::from_secs(60),
        }
    }
}

/// Capability scopes a tool may need. Policy gates these independently of risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// No special capability.
    None,
    /// Read filesystem under allowed roots.
    FsRead,
    /// Write filesystem under allowed roots.
    FsWrite,
    /// Outbound network.
    Network,
    /// Shell execution.
    Shell,
    /// Clipboard read/write.
    Clipboard,
    /// UI / open URL-type side effects.
    UiControl,
}

/// Static metadata the runtime uses for policy, timeout, confirmation, and
/// (with wave scheduling) batch planning.
#[derive(Debug, Clone)]
pub struct ToolMeta {
    /// Risk class for HITL defaults.
    pub risk: ToolRisk,
    /// Capability scopes required.
    pub permissions: &'static [Permission],
    /// Wall-clock budget for `execute`.
    pub default_timeout: Duration,
    /// When true, runtime always pauses for HITL before execute (unless granted).
    pub requires_confirmation: bool,
    /// Category for capability presets and parallel scheduling.
    pub kind: ToolKind,
    /// Override observation char cap after execute (`None` → [`MAX_TOOL_RESULT_CHARS`]).
    pub max_result_chars: Option<usize>,
    /// Explicit schedule class for wave scheduling. `None` falls back to the
    /// legacy kind/risk heuristic (tests / unannotated tools only).
    pub read_only: Option<bool>,
    /// Max concurrent invocations of this tool name under wave scheduling.
    /// `None` → 8 for read-only, 1 for writers.
    pub max_concurrency: Option<u32>,
}

impl ToolMeta {
    /// Safe, no special permissions, 5s timeout, no confirmation.
    pub fn safe_default() -> Self {
        Self {
            risk: ToolRisk::Safe,
            permissions: &[Permission::None],
            default_timeout: ToolRisk::Safe.default_timeout(),
            requires_confirmation: false,
            kind: ToolKind::Other,
            max_result_chars: None,
            read_only: None,
            max_concurrency: None,
        }
    }

    /// Start from a risk class (timeout follows risk defaults).
    pub fn with_risk(risk: ToolRisk) -> Self {
        Self {
            risk,
            permissions: &[Permission::None],
            default_timeout: risk.default_timeout(),
            requires_confirmation: false,
            kind: ToolKind::Other,
            max_result_chars: None,
            read_only: None,
            max_concurrency: None,
        }
    }

    /// Set permission list.
    pub fn permissions(mut self, permissions: &'static [Permission]) -> Self {
        self.permissions = permissions;
        self
    }

    /// Override timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Require HITL confirmation before execute.
    pub fn confirm(mut self, requires: bool) -> Self {
        self.requires_confirmation = requires;
        self
    }

    /// Set tool kind.
    pub fn kind(mut self, kind: ToolKind) -> Self {
        self.kind = kind;
        self
    }

    /// Cap observation size after execute.
    pub fn max_result_chars(mut self, max: usize) -> Self {
        self.max_result_chars = Some(max);
        self
    }

    /// Explicit read-only schedule class (preferred over kind heuristic).
    pub fn read_only(mut self, v: bool) -> Self {
        self.read_only = Some(v);
        self
    }

    /// Cap concurrent runs of this tool name under wave scheduling.
    pub fn max_concurrency(mut self, n: u32) -> Self {
        self.max_concurrency = Some(n.max(1));
        self
    }

    /// Prefer explicit `read_only` when set; else kind/risk heuristic.
    pub fn is_read_only(&self) -> bool {
        if let Some(v) = self.read_only {
            return v;
        }
        self.kind.is_read_only() && self.risk <= ToolRisk::Moderate && !self.requires_confirmation
    }

    /// Effective concurrency under wave scheduling (defaults: 8 RO / 1 write).
    pub fn effective_max_concurrency(&self) -> u32 {
        self.max_concurrency
            .unwrap_or_else(|| if self.is_read_only() { 8 } else { 1 })
            .max(1)
    }

    /// Character budget for truncating observations.
    pub fn result_char_budget(&self) -> usize {
        self.max_result_chars.unwrap_or(MAX_TOOL_RESULT_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_heuristic_and_override() {
        let m = ToolMeta::with_risk(ToolRisk::Safe).kind(ToolKind::Read);
        assert!(m.is_read_only());
        let m = m.read_only(false);
        assert!(!m.is_read_only());
    }

    #[test]
    fn concurrency_defaults() {
        let ro = ToolMeta::safe_default().read_only(true);
        assert_eq!(ro.effective_max_concurrency(), 8);
        let w = ToolMeta::with_risk(ToolRisk::Dangerous).read_only(false);
        assert_eq!(w.effective_max_concurrency(), 1);
        assert_eq!(w.max_concurrency(4).effective_max_concurrency(), 4);
    }
}
