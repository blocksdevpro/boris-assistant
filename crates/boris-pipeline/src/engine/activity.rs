//! Pure string-formatting helpers for the overlay's tool-activity chip.
//!
//! These translate raw [`AgentEvent`]s (and tool-call arg summaries) into the
//! short `"tool · web_search · query"` style labels the UI displays. No I/O,
//! no engine state — moved out of `engine::mod` so the turn-loop file stays
//! focused on the loop itself.

use boris_agent::{AgentEvent, Role};

/// Compact tool-activity label for the overlay chip.
///
/// Wire format is stable (`tool ·`, `done ·`, `fail ·`, `thinking ·`, `confirm ·`)
/// so the UI humanizer can parse it. Prefer *what* is happening over step numbers.
///
/// `recent_tools` (most recent last) enriches post-tool thinking labels.
pub(super) fn activity_label(ev: &AgentEvent, recent_tools: &[String]) -> Option<String> {
    match ev {
        AgentEvent::ToolExecutionStart {
            tool_name,
            args_summary,
            ..
        } => {
            let detail = tool_start_detail(tool_name, args_summary);
            if detail.is_empty() {
                Some(format!("tool · {tool_name}"))
            } else {
                Some(format!("tool · {tool_name} · {detail}"))
            }
        }
        AgentEvent::ToolExecutionEnd { tool_name, ok, .. } => Some(if *ok {
            format!("done · {tool_name}")
        } else {
            format!("fail · {tool_name}")
        }),
        AgentEvent::ToolProgress {
            tool_name,
            message,
            ..
        } => {
            let msg = message.trim();
            if msg.is_empty() {
                Some(format!("tool · {tool_name}"))
            } else {
                let short = truncate_activity(msg, 56);
                Some(format!("tool · {tool_name} · {short}"))
            }
        }
        // Assistant decided on tools — show count before ToolExecutionStart fires.
        AgentEvent::MessageEnd { role, preview }
            if matches!(role, Role::Assistant) && preview.contains("tool call") =>
        {
            let n = preview
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if n.is_empty() {
                Some("thinking · calling tools".into())
            } else if n == "1" {
                Some("thinking · 1 tool next".into())
            } else {
                Some(format!("thinking · {n} tools next"))
            }
        }
        // Round 0 is the first LLM call (already shown as "thinking…").
        // Later rounds = model deciding after tools — name the last tools when known.
        AgentEvent::TurnStart { round } if *round > 0 => {
            if recent_tools.is_empty() {
                Some("thinking · next action".into())
            } else {
                let names = recent_tools
                    .iter()
                    .rev()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("thinking · after {names}"))
            }
        }
        AgentEvent::NeedsConfirmation { pending } => {
            Some(format!("confirm · {}", pending.name))
        }
        _ => None,
    }
}

/// Prefer a short args hint over the raw `tool (k=v)` audit summary.
fn tool_start_detail(tool_name: &str, args_summary: &str) -> String {
    let s = args_summary.trim();
    if s.is_empty() || s == tool_name {
        return String::new();
    }
    // args_summary is often `bash (command=ls -la)` — strip the name wrapper.
    let inner = s
        .strip_prefix(tool_name)
        .map(str::trim)
        .and_then(|rest| rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')))
        .unwrap_or(s);
    let inner = inner.trim();
    // Prefer the most useful arg for the chip (query / url / goal / command).
    for key in ["query", "url", "goal", "command", "path", "name"] {
        if let Some(v) = extract_arg_value(inner, key) {
            return truncate_activity(&v, 48);
        }
    }
    truncate_activity(inner, 48)
}

/// Pull `key=value` or `key="value"` from a compact args summary.
fn extract_arg_value(summary: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let idx = summary.find(&needle)?;
    let rest = &summary[idx + needle.len()..];
    if rest.starts_with('"') {
        let body = &rest[1..];
        let end = body.find('"')?;
        let v = body[..end].trim();
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    // Unquoted: until comma or end.
    let end = rest.find(',').unwrap_or(rest.len());
    let v = rest[..end].trim().trim_matches('"');
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn truncate_activity(s: &str, max: usize) -> String {
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
    use boris_agent::Role;

    #[test]
    fn activity_label_tools() {
        let empty: &[String] = &[];
        let start = AgentEvent::ToolExecutionStart {
            call_id: "1".into(),
            tool_name: "bash".into(),
            args_summary: String::new(),
        };
        assert_eq!(activity_label(&start, empty).as_deref(), Some("tool · bash"));

        let start_args = AgentEvent::ToolExecutionStart {
            call_id: "2".into(),
            tool_name: "bash".into(),
            args_summary: "bash (command=ls -la)".into(),
        };
        assert_eq!(
            activity_label(&start_args, empty).as_deref(),
            Some("tool · bash · ls -la")
        );

        let search = AgentEvent::ToolExecutionStart {
            call_id: "3".into(),
            tool_name: "web_search".into(),
            args_summary: "web_search (query=Uttam LinkedIn Dhanbad)".into(),
        };
        assert_eq!(
            activity_label(&search, empty).as_deref(),
            Some("tool · web_search · Uttam LinkedIn Dhanbad")
        );

        let end_ok = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: true,
            duration_ms: 1,
        };
        assert_eq!(activity_label(&end_ok, empty).as_deref(), Some("done · bash"));

        let end_fail = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: false,
            duration_ms: 1,
        };
        assert_eq!(
            activity_label(&end_fail, empty).as_deref(),
            Some("fail · bash")
        );

        let noise = AgentEvent::TurnStart { round: 0 };
        assert!(activity_label(&noise, empty).is_none());

        let round2 = AgentEvent::TurnStart { round: 1 };
        assert_eq!(
            activity_label(&round2, empty).as_deref(),
            Some("thinking · next action")
        );
        let after = vec!["web_search".into(), "web_fetch".into()];
        assert_eq!(
            activity_label(&round2, &after).as_deref(),
            Some("thinking · after web_search, web_fetch")
        );

        let tools_next = AgentEvent::MessageEnd {
            role: Role::Assistant,
            preview: "3 tool call(s)".into(),
        };
        assert_eq!(
            activity_label(&tools_next, empty).as_deref(),
            Some("thinking · 3 tools next")
        );
    }
}
