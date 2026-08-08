//! Filesystem tools — tau-style `file_read` / `file_write` / `file_edit` + `list_dir`.
//!
//! Paths resolve under sandboxed roots (relative paths join the sandbox first).
//!
//! # Modules
//!
//! | Module | Tool | Name string |
//! |--------|------|-------------|
//! | [`list`] | [`ListDirTool`] | `list_dir` |
//! | [`read`] | [`ReadFileTool`] | `file_read` |
//! | [`write`] | [`WriteFileTool`] | `file_write` |
//! | [`edit`] | [`EditFileTool`] | `file_edit` |

mod edit;
mod list;
mod read;
mod write;

use std::path::PathBuf;

use crate::tools::fs_common::{read_roots, write_roots};

pub use edit::EditFileTool;
pub use list::ListDirTool;
pub use read::ReadFileTool;
pub use write::WriteFileTool;

// ── Shared limits ────────────────────────────────────────────────────────────

/// Hard cap on directory entries returned by `list_dir`.
pub(crate) const MAX_LIST: usize = 200;
/// Default `list_dir` entry limit when the model omits `limit`.
pub(crate) const DEFAULT_LIST_LIMIT: usize = 80;

/// Hard cap on lines returned by a single `file_read`.
pub(crate) const MAX_READ_LINES: usize = 2000;
/// Default `file_read` line limit when the model omits `limit`.
pub(crate) const DEFAULT_READ_LINES: usize = 200;
/// Soft byte budget for a single `file_read` body (before global tool truncate).
pub(crate) const MAX_READ_BYTES: usize = 200 * 1024;

/// Max content size accepted by `file_write`.
pub(crate) const MAX_WRITE_BYTES: usize = 512 * 1024;

// ── Root set ─────────────────────────────────────────────────────────────────

/// Sandbox + optional data / allowlisted roots shared by all FS tools.
#[derive(Debug, Clone)]
pub struct FsRoots {
    /// Default write/read sandbox (typically `~/.boris/sandbox`).
    pub sandbox: PathBuf,
    /// Boris data roots (memory, sessions) — readable and writable.
    pub data: Vec<PathBuf>,
    /// Extra user-granted read roots (Desktop, Documents, …).
    pub allow_read: Vec<PathBuf>,
    /// Extra user-granted write roots (usually empty).
    pub allow_write: Vec<PathBuf>,
}

impl FsRoots {
    /// Roots tools may read from (sandbox + data + allow_read + allow_write).
    pub fn readers(&self) -> Vec<PathBuf> {
        read_roots(
            &self.sandbox,
            &self.data,
            &self.allow_read,
            &self.allow_write,
        )
    }

    /// Roots tools may write to (sandbox + data + allow_write).
    pub fn writers(&self) -> Vec<PathBuf> {
        write_roots(&self.sandbox, &self.data, &self.allow_write)
    }
}

// ── Shared test helpers ──────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod test_util {
    use super::FsRoots;
    use std::path::PathBuf;

    /// Unique temp sandbox for FS tool integration tests.
    pub fn temp_roots() -> (FsRoots, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "boris-fs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp sandbox");
        (
            FsRoots {
                sandbox: dir.clone(),
                data: vec![],
                allow_read: vec![],
                allow_write: vec![],
            },
            dir,
        )
    }
}
