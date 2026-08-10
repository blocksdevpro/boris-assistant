//! Bash / shell execution — tau-style `bash` tool, adapted for Boris.
//!
//! Runs via `bash -lc` when available (Git Bash / WSL / Unix), otherwise falls
//! back to platform shell. **HITL confirmation is authoritative**; the deny list
//! in [`policy`] is best-effort only. Host [`ShellPolicy`](crate::runtime::ShellPolicy)
//! (Denied / Allowlist / OpenConfirm) gates registration-time capability.
//!
//! On Windows without bash, PowerShell is started with `-ExecutionPolicy Bypass`
//! for usability — that is not a sandbox.
//!
//! # Tool
//!
//! | Tool name | Type | Purpose |
//! |-----------|------|---------|
//! | `bash` | [`BashTool`] | Run a shell command under allowed cwd roots |
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`exec`]   | Process spawn, I/O, timeout / cancel, tool surface |
//! | [`policy`] | Best-effort hard-deny patterns (HITL is the real safety net) |
//! | [`output`] | Line/byte truncation of combined stdout+stderr |
//!
//! # Contributor notes
//!
//! - **Public surface**: only [`BashTool`] is re-exported. Keep tool name `"bash"`,
//!   confirm=true, and output caps stable for the model contract.
//! - **Semantics**: deny needles, timeout default/clamp (1–300s, default 120),
//!   truncation (last 2000 lines / 30KB), and fallback shells must not change
//!   without an intentional product decision.
//! - Prefer pure helpers (`policy`, `output`, timeout parsing) with unit tests
//!   over growing the execute path.
//! - Host must grant shell policy; registration is via [`crate::tools::bash_tools`].

mod exec;
mod output;
mod policy;

pub use exec::BashTool;

/// Default command timeout when the model omits `timeout`.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Max lines retained in command output (tail kept).
pub(crate) const MAX_LINES: usize = 2000;

/// Max bytes retained in command output before line cap.
pub(crate) const MAX_BYTES: usize = 30 * 1024;
