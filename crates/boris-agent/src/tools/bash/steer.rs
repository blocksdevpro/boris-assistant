//! Steer the model off bash when a dedicated tool exists (Grok tool_calling).
//!
//! Only fires for *simple* commands (no pipes / redirects / chaining). Real
//! pipelines and compiler/package-manager commands are left alone.

/// If this shell command should have been a dedicated tool, return the
/// observation to send instead of spawning. `None` = run the command.
pub(super) fn steer_simple_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || is_compound(command) {
        return None;
    }
    let token = first_token(command);
    match token {
        "cat" | "less" | "more" | "head" | "tail" | "get-content" | "gc" => Some(format!(
            "Command was not run.\n\
             Do not use bash to read files. Call file_read with path set to the file \
             (offset/limit for large files). Numbered lines come back automatically.\n\
             Original command: {command}"
        )),
        "type" if looks_like_path_arg(command) => Some(format!(
            "Command was not run.\n\
             Do not use bash/cmd `type` to read files. Call file_read instead.\n\
             Original command: {command}"
        )),
        "grep" | "rg" | "ripgrep" | "findstr" | "select-string" => Some(format!(
            "Command was not run.\n\
             Do not use bash grep/rg/findstr. Call grep with pattern, optional path, \
             glob, -i, -C/-A/-B, type, or output_mode. Batch several greps in one message.\n\
             Original command: {command}"
        )),
        "find" | "fd" => Some(format!(
            "Command was not run.\n\
             Do not use bash find. Call glob for name patterns (e.g. **/*.rs) or \
             list_dir for one directory.\n\
             Original command: {command}"
        )),
        "ls" | "dir" | "tree" | "get-childitem" | "gci" | "ls.exe" => Some(format!(
            "Command was not run.\n\
             Do not use bash ls/dir. Call list_dir (one folder) or glob (name pattern).\n\
             Original command: {command}"
        )),
        "echo" | "write-output" | "write-host" | "printf" => Some(format!(
            "Command was not run.\n\
             Do not use bash echo/printf to talk to the user or to jot notes. \
             Put that text in your spoken reply (or present_artifact for long content).\n\
             Original command: {command}"
        )),
        "sed" | "awk" => Some(format!(
            "Command was not run.\n\
             Do not use bash sed/awk to edit files. Call file_read, then file_edit \
             (unique old_string → new_string) or file_write.\n\
             Original command: {command}"
        )),
        _ => None,
    }
}

fn is_compound(command: &str) -> bool {
    command.contains('|')
        || command.contains("&&")
        || command.contains("||")
        || command.contains(';')
        || command.contains('>')
        || command.contains('<')
        || command.contains('`')
        || command.contains("$(")
}

fn first_token(command: &str) -> &str {
    let raw = command.split_whitespace().next().unwrap_or("");
    let raw = raw.trim_matches(|c| c == '"' || c == '\'');
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat");
    // Compare case-insensitively without allocating when already lowercase.
    // Callers match on lowercase literals; normalize via a small stack buffer.
    // (Keep returning a borrowed lowercase-looking slice when possible.)
    lowercase_token(base)
}

fn lowercase_token(s: &str) -> &str {
    if s.bytes().all(|b| !b.is_ascii_uppercase()) {
        s
    } else {
        // Leak-free: only a handful of known tokens matter; compare via eq_ignore_ascii_case
        // in the match above instead. Map here to a static when it matches a known tool.
        for known in [
            "cat",
            "less",
            "more",
            "head",
            "tail",
            "get-content",
            "gc",
            "type",
            "grep",
            "rg",
            "ripgrep",
            "findstr",
            "select-string",
            "find",
            "fd",
            "ls",
            "dir",
            "tree",
            "get-childitem",
            "gci",
            "echo",
            "write-output",
            "write-host",
            "printf",
            "sed",
            "awk",
        ] {
            if s.eq_ignore_ascii_case(known) {
                return known;
            }
        }
        s
    }
}

fn looks_like_path_arg(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    let _ = parts.next();
    let Some(arg) = parts.next() else {
        return false;
    };
    arg.contains('/')
        || arg.contains('\\')
        || arg.contains('.')
        || arg.ends_with(".txt")
        || arg.ends_with(".rs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steers_cat_grep_find_ls_echo() {
        assert!(steer_simple_command("cat src/main.rs")
            .unwrap()
            .contains("file_read"));
        assert!(steer_simple_command("grep TODO .")
            .unwrap()
            .contains("grep"));
        assert!(steer_simple_command("rg -n foo").unwrap().contains("grep"));
        assert!(steer_simple_command("find . -name '*.rs'")
            .unwrap()
            .contains("glob"));
        assert!(steer_simple_command("ls -la").unwrap().contains("list_dir"));
        assert!(steer_simple_command("echo hello")
            .unwrap()
            .contains("spoken"));
    }

    #[test]
    fn allows_real_shell_work() {
        assert!(steer_simple_command("cargo test -p boris-agent --lib").is_none());
        assert!(steer_simple_command("git status").is_none());
        assert!(steer_simple_command("cat file | wc -l").is_none());
        assert!(steer_simple_command("ls && cargo build").is_none());
        assert!(steer_simple_command("python script.py").is_none());
    }

    #[test]
    fn type_builtin_without_path_is_left_alone() {
        assert!(steer_simple_command("type cargo").is_none());
        assert!(steer_simple_command("type src/main.rs").is_some());
    }

    #[test]
    fn case_insensitive_windows_tokens() {
        assert!(steer_simple_command("Get-Content foo.txt")
            .unwrap()
            .contains("file_read"));
        assert!(steer_simple_command("DIR").unwrap().contains("list_dir"));
    }
}
