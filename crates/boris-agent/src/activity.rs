//! Grok-style short tool lines for the status picture / overlay chip.
//!
//! Verb + object, not `tool · bash`. Shared by the pipeline UI and subagent
//! progress so parent and child labels stay in the same voice.

/// Present (running) vs past (just finished).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tense {
    Present,
    Past,
}

/// Semantic bucket for collapsing consecutive same-kind tools ("Read 4 files").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerbKind {
    File,
    Skill,
    Search,
    Dir,
    WebFetch,
    WebSearch,
    Memory,
    Command,
    Edit,
    Write,
    Subagent,
    Other,
}

impl VerbKind {
    fn verb(self, tense: Tense) -> &'static str {
        let (past, present) = match self {
            VerbKind::File | VerbKind::Skill => ("Read", "Reading"),
            VerbKind::Search | VerbKind::WebSearch | VerbKind::Memory => ("Searched", "Searching"),
            VerbKind::Dir => ("Listed", "Listing"),
            VerbKind::WebFetch => ("Fetched", "Fetching"),
            VerbKind::Command | VerbKind::Subagent | VerbKind::Other => ("Ran", "Running"),
            VerbKind::Edit => ("Edited", "Editing"),
            VerbKind::Write => ("Wrote", "Writing"),
        };
        match tense {
            Tense::Present => present,
            Tense::Past => past,
        }
    }

    fn noun(self, count: usize) -> &'static str {
        let (one, many) = match self {
            VerbKind::File | VerbKind::Edit | VerbKind::Write => ("file", "files"),
            VerbKind::Skill => ("skill", "skills"),
            VerbKind::Search => ("pattern", "patterns"),
            VerbKind::Dir => ("dir", "dirs"),
            VerbKind::WebFetch | VerbKind::WebSearch => ("site", "sites"),
            VerbKind::Memory => ("memory", "memories"),
            VerbKind::Command => ("command", "commands"),
            VerbKind::Subagent => ("subagent", "subagents"),
            VerbKind::Other => ("tool", "tools"),
        };
        if count == 1 {
            one
        } else {
            many
        }
    }
}

/// Map a registered tool name to a verb bucket.
pub fn verb_kind(tool_name: &str) -> VerbKind {
    match tool_name {
        "file_read" | "read_file" => VerbKind::File,
        "file_write" | "write_file" => VerbKind::Write,
        "file_edit" => VerbKind::Edit,
        "list_dir" => VerbKind::Dir,
        "grep" | "glob" => VerbKind::Search,
        "web_search" => VerbKind::WebSearch,
        "web_fetch" => VerbKind::WebFetch,
        "bash" => VerbKind::Command,
        "spawn_subagent" => VerbKind::Subagent,
        "load_skill" => VerbKind::Skill,
        "memory_search" | "memory_get" | "recall_notes" => VerbKind::Memory,
        _ => VerbKind::Other,
    }
}

/// One-line description of a tool call (Grok header style).
pub fn describe_tool(tool_name: &str, args_summary: &str, tense: Tense) -> String {
    let args = parse_args_map(tool_name, args_summary);
    let kind = verb_kind(tool_name);
    match tool_name {
        "file_read" | "read_file" => {
            let file = args
                .get("path")
                .map(|p| basename(p))
                .filter(|s| !s.is_empty());
            let range = line_range(args.get("offset"), args.get("limit"));
            match (file, range, tense) {
                (Some(f), Some(r), Tense::Present) => format!("Reading {f} ({r})"),
                (Some(f), Some(r), Tense::Past) => format!("Read {f} ({r})"),
                (Some(f), None, Tense::Present) => format!("Reading {f}"),
                (Some(f), None, Tense::Past) => format!("Read {f}"),
                (_, _, t) => format!("{} a file", kind.verb(t)),
            }
        }
        "file_write" | "write_file" => {
            let file = args.get("path").map(|p| basename(p));
            match (file, tense) {
                (Some(f), Tense::Present) => format!("Writing {f}"),
                (Some(f), Tense::Past) => format!("Wrote {f}"),
                (_, t) => format!("{} a file", kind.verb(t)),
            }
        }
        "file_edit" => {
            let file = args.get("path").map(|p| basename(p));
            match (file, tense) {
                (Some(f), Tense::Present) => format!("Editing {f}"),
                (Some(f), Tense::Past) => format!("Edited {f}"),
                (_, t) => format!("{} a file", kind.verb(t)),
            }
        }
        "list_dir" => {
            let dir = args
                .get("path")
                .map(|p| basename(p))
                .filter(|s| !s.is_empty() && s != "." && s != "./");
            match (dir, tense) {
                (Some(d), Tense::Present) => format!("Listing {d}"),
                (Some(d), Tense::Past) => format!("Listed {d}"),
                (_, Tense::Present) => "Listing files".into(),
                (_, Tense::Past) => "Listed files".into(),
            }
        }
        "grep" => {
            let pat = args
                .get("pattern")
                .map(|s| truncate(s, 40))
                .unwrap_or_else(|| "code".into());
            match tense {
                Tense::Present => format!("Searching {pat}"),
                Tense::Past => format!("Searched {pat}"),
            }
        }
        "glob" => {
            let pat = args
                .get("pattern")
                .map(|s| truncate(s, 40))
                .unwrap_or_else(|| "files".into());
            match tense {
                Tense::Present => format!("Finding {pat}"),
                Tense::Past => format!("Found {pat}"),
            }
        }
        "bash" => {
            let cmd = args
                .get("command")
                .map(|c| truncate(&collapse_ws(c), 48))
                .filter(|s| !s.is_empty());
            match (cmd, tense) {
                (Some(c), Tense::Present) => format!("Running {c}"),
                (Some(c), Tense::Past) => format!("Ran {c}"),
                (_, Tense::Present) => "Running a command".into(),
                (_, Tense::Past) => "Ran a command".into(),
            }
        }
        "web_search" => {
            let q = args
                .get("query")
                .map(|s| truncate(s, 42))
                .unwrap_or_else(|| "the web".into());
            match tense {
                Tense::Present => format!("Searching {q}"),
                Tense::Past => format!("Searched {q}"),
            }
        }
        "web_fetch" => {
            let host = args
                .get("url")
                .map(|u| url_host(u))
                .unwrap_or_else(|| "a page".into());
            match tense {
                Tense::Present => format!("Fetching {host}"),
                Tense::Past => format!("Fetched {host}"),
            }
        }
        "open_url" => {
            let host = args
                .get("url")
                .map(|u| url_host(u))
                .unwrap_or_else(|| "a link".into());
            match tense {
                Tense::Present => format!("Opening {host}"),
                Tense::Past => format!("Opened {host}"),
            }
        }
        "open_path" => {
            let file = args.get("path").map(|p| basename(p));
            match (file, tense) {
                (Some(f), Tense::Present) => format!("Opening {f}"),
                (Some(f), Tense::Past) => format!("Opened {f}"),
                (_, Tense::Present) => "Opening a file".into(),
                (_, Tense::Past) => "Opened a file".into(),
            }
        }
        "load_skill" => {
            let name = args
                .get("name")
                .or_else(|| args.get("skill"))
                .map(|s| truncate(s, 32));
            match (name, tense) {
                (Some(n), Tense::Present) => format!("Loading {n}"),
                (Some(n), Tense::Past) => format!("Loaded {n}"),
                (_, Tense::Present) => "Loading a skill".into(),
                (_, Tense::Past) => "Loaded a skill".into(),
            }
        }
        "list_skills" => match tense {
            Tense::Present => "Listing skills".into(),
            Tense::Past => "Listed skills".into(),
        },
        "todo_write" => match tense {
            Tense::Present => "Updating todos".into(),
            Tense::Past => "Updated todos".into(),
        },
        "todo_read" => match tense {
            Tense::Present => "Checking todos".into(),
            Tense::Past => "Checked todos".into(),
        },
        "present_artifact" => {
            let title = args
                .get("title")
                .or_else(|| args.get("id"))
                .map(|s| truncate(s, 36));
            match (title, tense) {
                (Some(t), Tense::Present) => format!("Showing {t}"),
                (Some(t), Tense::Past) => format!("Showed {t}"),
                (_, Tense::Present) => "Showing a card".into(),
                (_, Tense::Past) => "Showed a card".into(),
            }
        }
        "spawn_subagent" => {
            let goal = args
                .get("goal")
                .or_else(|| args.get("task"))
                .map(|s| truncate(s, 40));
            match (goal, tense) {
                (Some(g), Tense::Present) => format!("Running subagent · {g}"),
                (Some(g), Tense::Past) => format!("Ran subagent · {g}"),
                (_, Tense::Present) => "Running a subagent".into(),
                (_, Tense::Past) => "Ran a subagent".into(),
            }
        }
        "get_time" => match tense {
            Tense::Present => "Checking the time".into(),
            Tense::Past => "Checked the time".into(),
        },
        "get_date" => match tense {
            Tense::Present => "Checking the date".into(),
            Tense::Past => "Checked the date".into(),
        },
        "get_system_info" => match tense {
            Tense::Present => "Checking system info".into(),
            Tense::Past => "Checked system info".into(),
        },
        "remember_note" => match tense {
            Tense::Present => "Saving a note".into(),
            Tense::Past => "Saved a note".into(),
        },
        "recall_notes" => match tense {
            Tense::Present => "Recalling notes".into(),
            Tense::Past => "Recalled notes".into(),
        },
        "memory_search" => {
            let q = args.get("query").map(|s| truncate(s, 36));
            match (q, tense) {
                (Some(q), Tense::Present) => format!("Searching memory · {q}"),
                (Some(q), Tense::Past) => format!("Searched memory · {q}"),
                (_, Tense::Present) => "Searching memory".into(),
                (_, Tense::Past) => "Searched memory".into(),
            }
        }
        "memory_get" => match tense {
            Tense::Present => "Reading memory".into(),
            Tense::Past => "Read memory".into(),
        },
        "clipboard_get" => match tense {
            Tense::Present => "Reading clipboard".into(),
            Tense::Past => "Read clipboard".into(),
        },
        "clipboard_set" => match tense {
            Tense::Present => "Copying to clipboard".into(),
            Tense::Past => "Copied to clipboard".into(),
        },
        "get_user_context" => match tense {
            Tense::Present => "Checking profile".into(),
            Tense::Past => "Checked profile".into(),
        },
        "save_user_fact" | "update_user_profile" => match tense {
            Tense::Present => "Updating profile".into(),
            Tense::Past => "Updated profile".into(),
        },
        "tool_search" => match tense {
            Tense::Present => "Finding tools".into(),
            Tense::Past => "Found tools".into(),
        },
        other => match tense {
            Tense::Present => format!("Running {}", other.replace('_', " ")),
            Tense::Past => format!("Ran {}", other.replace('_', " ")),
        },
    }
}

/// Collapsed header for N consecutive same-kind tools: `Read 4 files`.
pub fn describe_batch(kind: VerbKind, count: usize, tense: Tense) -> String {
    let n = count.max(1);
    format!("{} {n} {}", kind.verb(tense), kind.noun(n))
}

/// End-of-turn sticky summary from the tools that actually ran this turn.
pub fn summarize_tools_used(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let mut parts: Vec<(VerbKind, usize)> = Vec::new();
    for name in names {
        let k = verb_kind(name);
        if let Some(last) = parts.last_mut() {
            if last.0 == k {
                last.1 += 1;
                continue;
            }
        }
        parts.push((k, 1));
    }
    parts
        .into_iter()
        .map(|(k, n)| describe_batch(k, n, Tense::Past))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `Thought for 9.0s` (Grok collapsed thinking header).
pub fn describe_thought(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs_f32();
    if secs < 0.05 {
        "Thinking…".into()
    } else {
        format!("Thought for {secs:.1}s")
    }
}

/// Consecutive same-kind tool wave (for live "Reading 4 files").
#[derive(Debug, Clone, Default)]
pub struct ActivityWave {
    kind: Option<VerbKind>,
    count: usize,
    last_name: String,
    last_args: String,
}

impl ActivityWave {
    pub fn on_start(&mut self, tool_name: &str, args_summary: &str) -> (VerbKind, usize) {
        let kind = verb_kind(tool_name);
        if self.kind == Some(kind) {
            self.count = self.count.saturating_add(1);
        } else {
            self.kind = Some(kind);
            self.count = 1;
        }
        self.last_name = tool_name.to_string();
        self.last_args = args_summary.to_string();
        (kind, self.count)
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn kind(&self) -> Option<VerbKind> {
        self.kind
    }

    pub fn last_name(&self) -> &str {
        &self.last_name
    }

    pub fn last_args(&self) -> &str {
        &self.last_args
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn parse_args_map(
    tool_name: &str,
    args_summary: &str,
) -> std::collections::HashMap<String, String> {
    let s = args_summary.trim();
    let inner = s
        .strip_prefix(tool_name)
        .map(str::trim)
        .and_then(|rest| rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')))
        .unwrap_or(s);
    let mut out = std::collections::HashMap::new();
    if inner.is_empty() || inner == tool_name {
        return out;
    }
    for part in inner.split(',') {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim();
        if !k.is_empty() && !v.is_empty() {
            out.insert(k.trim().to_string(), v.to_string());
        }
    }
    out
}

fn basename(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.rsplit('/').next().unwrap_or(path).to_string()
}

fn line_range(offset: Option<&String>, limit: Option<&String>) -> Option<String> {
    let start = offset.and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
    let limit = limit.and_then(|s| s.parse::<usize>().ok())?;
    if limit == 0 {
        return None;
    }
    let end = start.saturating_add(limit.saturating_sub(1));
    Some(format!("{start}-{end}"))
}

fn url_host(url: &str) -> String {
    let rest = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .trim_start_matches('/');
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.split('@').next_back().unwrap_or(host);
    if host.is_empty() {
        truncate(url, 36)
    } else {
        host.trim_start_matches("www.").to_string()
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_with_window() {
        let s = describe_tool(
            "file_read",
            "file_read (path=C:\\proj\\finish_gate.rs, offset=471, limit=50)",
            Tense::Present,
        );
        assert_eq!(s, "Reading finish_gate.rs (471-520)");
        let d = describe_tool("file_read", "file_read (path=src/main.rs)", Tense::Past);
        assert_eq!(d, "Read main.rs");
    }

    #[test]
    fn grep_and_bash() {
        assert_eq!(
            describe_tool(
                "grep",
                "grep (pattern=fn\\s+\\w+, path=src)",
                Tense::Present
            ),
            r"Searching fn\s+\w+"
        );
        assert_eq!(
            describe_tool(
                "bash",
                "bash (command=cargo test -p boris-agent --lib)",
                Tense::Past
            ),
            "Ran cargo test -p boris-agent --lib"
        );
    }

    #[test]
    fn web_and_fetch() {
        assert_eq!(
            describe_tool(
                "web_search",
                "web_search (query=Uttam LinkedIn Dhanbad)",
                Tense::Present
            ),
            "Searching Uttam LinkedIn Dhanbad"
        );
        assert_eq!(
            describe_tool(
                "web_fetch",
                "web_fetch (url=https://www.example.com/foo?x=1)",
                Tense::Past
            ),
            "Fetched example.com"
        );
    }

    #[test]
    fn batch_and_summary() {
        assert_eq!(
            describe_batch(VerbKind::File, 4, Tense::Present),
            "Reading 4 files"
        );
        assert_eq!(
            describe_batch(VerbKind::File, 4, Tense::Past),
            "Read 4 files"
        );
        let names = vec![
            "file_read".into(),
            "file_read".into(),
            "grep".into(),
            "bash".into(),
        ];
        assert_eq!(
            summarize_tools_used(&names),
            "Read 2 files, Searched 1 pattern, Ran 1 command"
        );
    }

    #[test]
    fn thought_format() {
        assert_eq!(
            describe_thought(std::time::Duration::from_millis(9040)),
            "Thought for 9.0s"
        );
        assert_eq!(
            describe_thought(std::time::Duration::from_millis(10)),
            "Thinking…"
        );
    }

    #[test]
    fn wave_counts_consecutive_kind() {
        let mut w = ActivityWave::default();
        assert_eq!(w.on_start("file_read", "").1, 1);
        assert_eq!(w.on_start("file_read", "").1, 2);
        assert_eq!(w.on_start("file_read", "").1, 3);
        assert_eq!(w.on_start("grep", "").1, 1);
        assert_eq!(w.kind(), Some(VerbKind::Search));
    }
}
