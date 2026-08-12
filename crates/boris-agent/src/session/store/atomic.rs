//! Atomic file write helper (tmp + rename, with fallback).

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Write `bytes` to `path` via a sibling `*.json.tmp`, then rename.
///
/// Falls back to direct write if rename fails (e.g. cross-device).
pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            let mut f = fs::File::create(path)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            let _ = e;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(label: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("boris-atomic-{nanos}-{n}-{label}"));
        let _ = fs::create_dir_all(&dir);
        dir.join("target.json")
    }

    #[test]
    fn write_atomic_creates_and_overwrites() {
        let path = temp_path("basic");
        write_atomic(&path, b"{\"a\":1}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}");
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}
