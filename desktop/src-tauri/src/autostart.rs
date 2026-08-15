//! Start-with-Windows via the current-user Run key.
//!
//! The host writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Boris`
//! so login launches the packaged exe with `--autostart`. That flag is how
//! setup knows to stay in the tray and turn the engine on — no
//! main-window flash.
//!
//! Registry I/O is Windows-only. Other targets treat enable as an error so
//! the Settings toggle cannot silently lie.

use std::path::Path;

/// CLI flag the Run-key command line appends.
pub const AUTOSTART_FLAG: &str = "--autostart";

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Boris";

/// True when this process was started by the Windows logon Run entry.
pub fn launched_from_windows_startup() -> bool {
    args_include_autostart(std::env::args())
}

/// Parse argv for the silent-start flag (first arg is the exe; ignored).
pub fn args_include_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|a| matches!(a.as_ref(), "--autostart" | "-autostart"))
}

/// Quote `exe` and append [`AUTOSTART_FLAG`] for the Run-key value.
pub fn format_launch_command(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_FLAG}", exe.display())
}

/// Enable or disable the current-user Run entry. Refreshes the exe path when
/// enabling so an in-place update keeps launching the live binary.
pub fn apply(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        apply_windows(enabled)
    }
    #[cfg(not(windows))]
    {
        if enabled {
            return Err("Start with Windows is only available on Windows".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn apply_windows(enabled: bool) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(RUN_SUBKEY)
        .map_err(|e| format!("open HKCU Run key: {e}"))?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| format!("resolve current exe: {e}"))?;
        let cmd = format_launch_command(&exe);
        key.set_value(VALUE_NAME, &cmd)
            .map_err(|e| format!("write HKCU Run value: {e}"))?;
        tracing::info!(command = %cmd, "windows startup enabled");
    } else {
        match key.delete_value(VALUE_NAME) {
            Ok(()) => tracing::info!("windows startup disabled"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("windows startup already off");
            }
            Err(e) => return Err(format!("remove HKCU Run value: {e}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_autostart_flag() {
        assert!(args_include_autostart(["boris", "--autostart"]));
        assert!(args_include_autostart(["boris.exe", "-autostart"]));
        assert!(!args_include_autostart(["boris"]));
        assert!(!args_include_autostart(["boris", "--help"]));
    }

    #[test]
    fn quotes_exe_path_with_spaces() {
        let exe = PathBuf::from(r"C:\Program Files\Boris\boris.exe");
        assert_eq!(
            format_launch_command(&exe),
            r#""C:\Program Files\Boris\boris.exe" --autostart"#
        );
    }
}
