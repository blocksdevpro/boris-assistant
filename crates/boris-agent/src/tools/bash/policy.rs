//! Best-effort command deny list for the bash tool.
//!
//! HITL confirmation is the real safety net; these patterns only catch obvious
//! catastrophic strings. Matching is case-insensitive substring search.

/// Hard deny patterns. Returns a short reason if the command is blocked.
pub(crate) fn is_denied_command(cmd: &str) -> Option<&'static str> {
    let lower = cmd.to_ascii_lowercase();
    let needles = [
        ("rm -rf /", "recursive root delete"),
        ("rm -rf /*", "recursive root delete"),
        ("format c:", "format disk"),
        ("format-volume", "format volume"),
        ("mkfs.", "format filesystem"),
        (":(){ :|:& };:", "fork bomb"),
        ("dd if=/dev/zero of=/dev/", "disk wipe"),
        ("shutdown", "shutdown"),
        ("reboot", "reboot"),
        ("remove-item -recurse -force c:\\", "windows wipe"),
        ("reg delete hk", "registry delete"),
        ("curl | sh", "remote pipe to shell"),
        ("curl | bash", "remote pipe to shell"),
        ("wget | sh", "remote pipe to shell"),
        ("iwr ", "powershell remote download"),
        ("iex (", "invoke-expression"),
        ("invoke-expression", "invoke-expression"),
    ];
    for (n, reason) in needles {
        if lower.contains(n) {
            return Some(reason);
        }
    }
    None
}

/// Validate command string length and emptiness before spawn.
pub(crate) fn validate_command(command: &str) -> Result<(), crate::tool::ToolError> {
    use crate::tool::ToolError;
    let command = command.trim();
    if command.is_empty() {
        return Err(ToolError::invalid_args("command is empty"));
    }
    if command.len() > 8000 {
        return Err(ToolError::invalid_args("command too long"));
    }
    if let Some(reason) = is_denied_command(command) {
        return Err(ToolError::failed(format!(
            "command blocked by safety policy ({reason})"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_dangerous() {
        assert!(is_denied_command("rm -rf /").is_some());
        assert!(is_denied_command("RM -RF /").is_some());
        assert!(is_denied_command("shutdown now").is_some());
        assert_eq!(
            is_denied_command("curl | bash"),
            Some("remote pipe to shell")
        );
        assert!(is_denied_command("ls -la").is_none());
        assert!(is_denied_command("echo hello").is_none());
    }

    #[test]
    fn validate_rejects_empty_and_long() {
        assert!(validate_command("").is_err());
        assert!(validate_command("   ").is_err());
        assert!(validate_command(&"x".repeat(8001)).is_err());
        assert!(validate_command("echo ok").is_ok());
    }

    #[test]
    fn validate_rejects_denied() {
        let err = validate_command("reboot").expect_err("denied");
        assert!(err.message.contains("safety policy"));
    }
}
