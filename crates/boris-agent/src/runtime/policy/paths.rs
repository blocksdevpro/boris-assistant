//! Path normalization and sandbox root checks for tool policy.
//!
//! # Symlinks and TOCTOU
//!
//! After lexical normalize, we **canonicalize when possible** (path or nearest
//! existing ancestor) and re-check [`path_is_within`] on the real path so a
//! symlink under an allowed root cannot point outside.
//!
//! **Residual TOCTOU**: a path may change (symlink retarget, mount, rename)
//! between this policy check and a later open/write in the tool body. Policy
//! cannot eliminate the race without openat-style `O_NOFOLLOW` everywhere, so
//! tools must treat policy as a pre-check, not a guarantee:
//!
//! - Prefer [`resolve_under_roots`] over [`resolve_in_roots`] when opening
//!   files: it re-canonicalizes the parent **after** the initial containment
//!   check ([`re_resolve_after_open`]) and re-verifies containment, shrinking
//!   (not closing) the race window.
//! - For session `cwd` fallbacks, use [`path_within_root`] (same
//!   canonicalizing containment) instead of a lexical `starts_with`, which is
//!   case-sensitive and `..`/symlink-blind.
//! - Env scrubbing and denylists elsewhere in policy are likewise best-effort;
//!   HITL confirmation is the authoritative control.
//!
//! TODO(policy): `tools/bash/exec.rs` session-`cwd` fallback currently does a
//! lexical check — it should call [`path_within_root`] here instead. Left as a
//! TODO because `tools/*` is owned by another group (do NOT edit `exec.rs`
//! from this slice).

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::SandboxConfig;

/// Keys treated as path-like in tool args (all must pass root checks when present).
const PATH_ARG_KEYS: &[&str] = &[
    "path",
    "paths",
    "file",
    "filepath",
    "file_path",
    "dir",
    "directory",
    "cwd",
    "source",
    "dest",
    "destination",
    "target",
    "from",
    "to",
];

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

#[derive(Debug, Clone, Copy)]
pub(super) enum PathAccess {
    Read,
    Write,
}

/// Collect **all** non-empty path-like string fields from tool args.
///
/// Checks both scalar string values (`"path": "a.txt"`) and array-of-string
/// values (`"paths": ["a.txt", "b.txt"]`) under the tracked [`PATH_ARG_KEYS`],
/// so a future tool with an array-of-paths arg gets the same path-policy
/// checking as scalar path args. No current builtin tool uses the array
/// shape (checked via grep across `tools/`); this is defense in depth.
pub(super) fn args_path_strings(args: &Value) -> Vec<&str> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else {
        return out;
    };
    for key in PATH_ARG_KEYS {
        let Some(value) = obj.get(*key) else {
            continue;
        };
        if let Some(s) = value.as_str() {
            if !s.is_empty() {
                out.push(s);
            }
        } else if let Some(arr) = value.as_array() {
            for item in arr {
                if let Some(s) = item.as_str() {
                    if !s.is_empty() {
                        out.push(s);
                    }
                }
            }
        }
    }
    out
}

pub(super) fn check_path_allowed(
    config: &SandboxConfig,
    raw: &str,
    access: PathAccess,
) -> Result<(), String> {
    // Relative model args are sandbox-relative (same contract as tools:
    // `resolve_under_roots` joins under sandbox first). Absolute paths are
    // checked as-is after normalize + best-effort canonicalize.
    let resolved = resolve_policy_candidate(config, raw)?;
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
        let root_n = resolve_path_for_policy(root)
            .unwrap_or_else(|_| normalize_path(root).unwrap_or_else(|_| root.clone()));
        if path_is_within(&resolved, &root_n) {
            return Ok(());
        }
    }

    Err(format!(
        "path `{}` is outside allowed roots",
        resolved.display()
    ))
}

/// Build the absolute path policy should check for `raw`.
///
/// Relative paths join [`SandboxConfig::sandbox_root`] before normalize so
/// containment compares absolute roots against absolute candidates.
fn resolve_policy_candidate(config: &SandboxConfig, raw: &str) -> Result<PathBuf, String> {
    let raw_path = Path::new(raw);
    let candidate = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        config.sandbox_root.join(raw_path)
    };
    resolve_path_for_policy(&candidate)
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

/// Lexical normalize + best-effort canonicalize so symlink targets are checked.
///
/// See module docs for TOCTOU residual risk.
pub fn resolve_path_for_policy(path: &Path) -> Result<PathBuf, String> {
    let normalized = normalize_path(path)?;
    if let Some(canon) = try_canonicalize(&normalized) {
        return Ok(canon);
    }
    // Walk up to an existing ancestor, canonicalize it, re-append suffix.
    let mut cur = normalized.as_path();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Some(canon) = try_canonicalize(cur) {
            let mut out = canon;
            for s in suffix.iter().rev() {
                out.push(s);
            }
            return normalize_path(&out);
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) if parent != cur => {
                suffix.push(name.to_os_string());
                cur = parent;
            }
            _ => break,
        }
    }
    Ok(normalized)
}

fn try_canonicalize(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok().map(strip_windows_verbatim)
}

/// Strip Windows `\\?\` / `\\?\UNC\` prefixes so component compares work with
/// normal absolute roots from config.
fn strip_windows_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        if let Some(unc) = rest.strip_prefix(r"UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        return PathBuf::from(rest);
    }
    p
}

/// True if `path` is equal to `root` or a descendant (component-wise).
///
/// On Windows, component comparison is **case-insensitive** (NTFS default).
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
        .all(|(a, b)| os_str_eq_path(a.as_os_str(), b.as_os_str()))
}

fn os_str_eq_path(a: &OsStr, b: &OsStr) -> bool {
    #[cfg(windows)]
    {
        // NTFS is case-insensitive; compare Unicode lowercase via lossy UTF-16→str.
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Public helper for future file tools: resolve `raw` under write/read roots.
pub fn resolve_in_roots(config: &SandboxConfig, raw: &str, write: bool) -> Result<PathBuf, String> {
    let access = if write {
        PathAccess::Write
    } else {
        PathAccess::Read
    };
    check_path_allowed(config, raw, access)?;
    resolve_policy_candidate(config, raw)
}

/// Policy-side canonicalizing containment for session `cwd` fallbacks.
///
/// Unlike a lexical `starts_with` (case-sensitive, `..`/symlink-blind), this
/// resolves `..` lexically, strips Windows `\\?\` verbatim prefixes,
/// best-effort canonicalizes both sides (real path when it exists, nearest
/// existing ancestor + lexical suffix otherwise), and compares
/// case-insensitively on Windows via [`path_is_within`].
///
/// Intended for `tools/bash/exec.rs`-style `cwd` checks:
/// `path_within_root(&cwd_joined_candidate, &sandbox_root)`.
/// Returns `false` for empty/relative candidates that cannot be proven inside
/// an absolute root (fail-closed).
///
/// TODO(policy): migrate `tools/bash/exec.rs` `cwd` fallback to call this
/// helper (left as TODO — `tools/*` is read-only for this slice).
pub fn path_within_root(candidate: &Path, root: &Path) -> bool {
    if candidate.as_os_str().is_empty() || root.as_os_str().is_empty() {
        return false;
    }
    let cand = resolve_path_for_policy(candidate)
        .unwrap_or_else(|_| strip_verbatim_for_compare(candidate.to_path_buf()));
    let base = resolve_path_for_policy(root)
        .unwrap_or_else(|_| strip_verbatim_for_compare(root.to_path_buf()));
    // Fail closed on relative candidates vs absolute roots: `path_is_within`
    // already returns false there, but be explicit for readability.
    path_is_within(&cand, &base)
}

/// Strip `\\?\` for direct compares when canonicalize/normalize is unavailable.
fn strip_verbatim_for_compare(p: PathBuf) -> PathBuf {
    strip_windows_verbatim(p)
}

/// Best-effort re-resolve of a path's parent after a policy check.
///
/// Canonicalizes the parent (when it exists) and re-appends the file name so
/// a symlink swap between check and open is more likely to be noticed by a
/// second containment check. Still racy by construction (no `O_NOFOLLOW`
/// open); callers must document the residual TOCTOU and keep HITL as the
/// authoritative control.
pub fn re_resolve_after_open(path: &Path) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() {
        return Err("empty path".into());
    }
    let parent = path.parent();
    let file = path.file_name();
    match (parent, file) {
        (Some(par), Some(name)) => {
            if let Some(canon_par) = try_canonicalize(par) {
                let mut out = canon_par;
                out.push(name);
                return normalize_path(&out);
            }
            // Parent missing (not yet created): fall back to full resolve.
            resolve_path_for_policy(path)
        }
        _ => resolve_path_for_policy(path),
    }
}

/// TOCTOU-hardened (best-effort) resolve under sandbox roots.
///
/// 1. Runs the normal containment check ([`check_path_allowed`] via
///    [`resolve_in_roots`] semantics).
/// 2. Re-canonicalizes the parent via [`re_resolve_after_open`].
/// 3. Re-verifies containment of the re-resolved path.
///
/// Still racy (symlink retarget between step 3 and the caller's open), but
/// narrows the window versus a single pre-check. Always `#[must_use]` the
/// returned path (do not use the pre-check input) and keep HITL authoritative.
///
/// `#[must_use]` is on the function so callers cannot accidentally discard
/// the re-verified path and open the unchecked original.
#[must_use = "use the re-verified path; the input is only pre-check"]
pub fn resolve_under_roots(
    config: &SandboxConfig,
    raw: &str,
    write: bool,
) -> Result<PathBuf, String> {
    let access = if write {
        PathAccess::Write
    } else {
        PathAccess::Read
    };
    // Step 1: pre-check.
    check_path_allowed(config, raw, access)?;
    let resolved = resolve_policy_candidate(config, raw)?;
    // Step 2: re-resolve parent after check.
    let rechecked = re_resolve_after_open(&resolved)?;
    // Step 3: re-verify containment of the re-resolved path.
    let rechecked_str = rechecked.to_string_lossy().into_owned();
    // `check_path_allowed` joins relative inputs under the sandbox root; our
    // `rechecked` is absolute so it is checked as-is.
    check_path_allowed(config, &rechecked_str, access)?;
    Ok(rechecked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn test_sandbox_cfg(sandbox: PathBuf) -> SandboxConfig {
        SandboxConfig {
            sandbox_root: sandbox,
            boris_data_roots: vec![],
            allow_read: vec![],
            allow_write: vec![],
            network: super::super::NetworkPolicy::Off,
            shell: super::super::ShellPolicy::Denied,
            auto_allow_up_to: crate::tool::ToolRisk::Moderate,
            force_confirm_at_or_above: crate::tool::ToolRisk::Dangerous,
            max_confirms_per_turn: 12,
            trusted_auto_moderate: false,
        }
    }

    #[test]
    fn relative_write_allowed_under_sandbox() {
        let sandbox = PathBuf::from(r"C:\Users\me\.boris\state\workspace");
        let cfg = test_sandbox_cfg(sandbox);
        let result = check_path_allowed(&cfg, "note.txt", PathAccess::Write);
        assert!(
            result.is_ok(),
            "relative write under sandbox should be allowed: {result:?}"
        );
    }

    #[test]
    fn relative_escape_denied() {
        let sandbox = PathBuf::from(r"C:\Users\me\.boris\state\workspace");
        let cfg = test_sandbox_cfg(sandbox);
        // Escaping the sandbox via `..` must deny even after sandbox-join.
        let result = check_path_allowed(&cfg, "../outside", PathAccess::Write);
        assert!(
            result.is_err(),
            "relative path escaping sandbox should deny: {result:?}"
        );
        let result2 = check_path_allowed(&cfg, r"..\..\Windows\evil.txt", PathAccess::Write);
        assert!(result2.is_err());
    }

    #[test]
    fn relative_nested_write_allowed() {
        let sandbox = PathBuf::from(r"C:\Users\me\.boris\state\workspace");
        let cfg = test_sandbox_cfg(sandbox);
        let result = check_path_allowed(&cfg, "notes/daily/todo.md", PathAccess::Write);
        assert!(result.is_ok(), "{result:?}");
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
    fn args_path_strings_collects_all() {
        let args = serde_json::json!({
            "path": "a.txt",
            "cwd": "C:\\work",
            "source": "from.txt",
            "dest": "to.txt",
            "note": "not a path key"
        });
        let paths = args_path_strings(&args);
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"C:\\work"));
        assert!(paths.contains(&"from.txt"));
        assert!(paths.contains(&"to.txt"));
        assert_eq!(paths.len(), 4);
    }

    #[test]
    fn args_path_strings_recurses_into_string_arrays() {
        let args = serde_json::json!({
            "paths": ["a.txt", "../escape.txt"],
            "note": "not a path key",
        });
        let paths = args_path_strings(&args);
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"../escape.txt"));
        assert_eq!(paths.len(), 2);
    }

    #[test]
    #[cfg(windows)]
    fn path_is_within_case_insensitive_on_windows() {
        let root = Path::new(r"C:\Users\Me\Sandbox");
        let path = Path::new(r"c:\users\me\sandbox\file.txt");
        assert!(path_is_within(path, root));
        let outside = Path::new(r"C:\Users\Other\file.txt");
        assert!(!path_is_within(outside, root));
    }

    #[test]
    fn canonicalize_keeps_in_root_file() {
        let dir = std::env::temp_dir().join(format!("boris-path-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        {
            let mut f = fs::File::create(&file).unwrap();
            writeln!(f, "hi").unwrap();
        }
        let resolved = resolve_path_for_policy(&file).unwrap();
        let root = resolve_path_for_policy(&dir).unwrap();
        assert!(path_is_within(&resolved, &root));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(windows)]
    fn symlink_escape_denied_when_supported() {
        // Create dir with a symlink pointing outside; policy must reject when
        // Windows allows symlink creation for this user (strict: no allow-Ok).
        let base = std::env::temp_dir().join(format!("boris-symlink-test-{}", std::process::id()));
        let sandbox = base.join("sandbox");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&sandbox).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        fs::write(&secret, "x").unwrap();
        let link = sandbox.join("escape_link");
        // std::os::windows::fs::symlink_file / symlink_dir need privilege.
        let made = std::os::windows::fs::symlink_dir(&outside, &link).is_ok()
            || std::os::windows::fs::symlink_file(&secret, &link).is_ok();
        if !made {
            let _ = fs::remove_dir_all(&base);
            // Skip when privilege missing — not a failure.
            return;
        }
        let cfg = SandboxConfig {
            sandbox_root: sandbox.clone(),
            boris_data_roots: vec![],
            allow_read: vec![],
            allow_write: vec![],
            network: super::super::NetworkPolicy::Off,
            shell: super::super::ShellPolicy::Denied,
            auto_allow_up_to: crate::tool::ToolRisk::Moderate,
            force_confirm_at_or_above: crate::tool::ToolRisk::Dangerous,
            max_confirms_per_turn: 12,
            trusted_auto_moderate: false,
        };
        // If link is a dir symlink to outside, path under link is outside root after canon.
        let probe = if link.is_dir() {
            link.join("secret.txt")
        } else {
            link.clone()
        };
        let result = check_path_allowed(&cfg, probe.to_str().unwrap_or(""), PathAccess::Read);
        // Strict: a true symlink escape must deny. The old weak form allowed
        // Ok as "resolve ran without panic" — that hid escapes.
        assert!(
            result.is_err(),
            "symlink escape must deny, got Ok for probe {probe:?}"
        );
        // The hardened wrapper must deny too.
        let hardened = resolve_under_roots(
            &cfg,
            probe.to_str().unwrap_or(""),
            false,
        );
        assert!(
            hardened.is_err(),
            "resolve_under_roots must deny symlink escape, got {hardened:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escape_denied_on_unix() {
        let base =
            std::env::temp_dir().join(format!("boris-symlink-test-unix-{}", std::process::id()));
        let sandbox = base.join("sandbox");
        let outside = base.join("outside");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&sandbox).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        fs::write(&secret, "x").unwrap();
        let link = sandbox.join("escape_link");
        if std::os::unix::fs::symlink(&outside, &link).is_err() {
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let cfg = test_sandbox_cfg(sandbox.clone());
        let probe = link.join("secret.txt");
        let result = check_path_allowed(&cfg, probe.to_str().unwrap_or(""), PathAccess::Read);
        assert!(
            result.is_err(),
            "symlink escape must deny, got Ok for probe {probe:?}"
        );
        let hardened =
            resolve_under_roots(&cfg, probe.to_str().unwrap_or(""), false);
        assert!(
            hardened.is_err(),
            "resolve_under_roots must deny symlink escape, got {hardened:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn path_within_root_canonicalizes_and_fails_closed() {
        let dir = std::env::temp_dir().join(format!("boris-within-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sandbox")).unwrap();
        let root = dir.join("sandbox");
        // Direct child is within.
        assert!(path_within_root(&root.join("a.txt"), &root));
        // `..` escape is not within.
        assert!(!path_within_root(
            &root.join("..").join("outside.txt"),
            &root
        ));
        // Lexical `..` that folds back inside is within (documents that we
        // resolve `..` rather than doing a raw starts_with).
        assert!(path_within_root(
            &root.join("sub").join("..").join("a.txt"),
            &root
        ));
        // Empty / relative candidates fail closed against an absolute root.
        assert!(!path_within_root(Path::new(""), &root));
        // Case behavior follows `path_is_within` (case-insensitive on Windows).
        #[cfg(windows)]
        {
            let upper = PathBuf::from(root.to_string_lossy().to_uppercase());
            assert!(path_within_root(&upper.join("a.txt"), &root));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_under_roots_allows_inside_and_denies_outside() {
        let dir =
            std::env::temp_dir().join(format!("boris-under-roots-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sandbox")).unwrap();
        let sandbox = dir.join("sandbox");
        let cfg = test_sandbox_cfg(sandbox.clone());
        fs::write(sandbox.join("ok.txt"), "hi").unwrap();
        let ok = resolve_under_roots(&cfg, sandbox.join("ok.txt").to_str().unwrap(), false);
        assert!(ok.is_ok(), "inside root must resolve: {ok:?}");
        // Re-resolved path must be the must-use return value (still within).
        let p = ok.unwrap();
        assert!(path_within_root(&p, &sandbox));
        let outside = dir.join("evil.txt");
        fs::write(&outside, "x").unwrap();
        let bad = resolve_under_roots(&cfg, outside.to_str().unwrap(), false);
        assert!(bad.is_err(), "outside root must deny: {bad:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_resolve_after_open_recanonicalizes_parent() {
        let dir =
            std::env::temp_dir().join(format!("boris-reresolve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sandbox")).unwrap();
        let file = dir.join("sandbox").join("note.txt");
        fs::write(&file, "hi").unwrap();
        let again = re_resolve_after_open(&file).unwrap();
        assert!(path_is_within(&again, &dir.join("sandbox")));
        assert!(re_resolve_after_open(Path::new("")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
