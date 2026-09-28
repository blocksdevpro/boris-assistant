//! Overlay chip labels — Grok-style verb + object, stable wire prefixes.
//!
//! Wire format stays parseable by the UI (`tool ·`, `done ·`, `fail ·`,
//! `thinking ·`, `confirm ·`). The text after the prefix is the human line
//! (`Reading finish_gate.rs (471-520)`, `Thought for 9.0s`, `Read 4 files`).

use boris_agent::{describe_batch, describe_tool, ActivityWave, AgentEvent, Role, Tense};

/// Wire prefixes for the overlay chip. The UI parses on these — `mod.rs` must
/// use [`is_tool_chip`] instead of re-stating string prefixes.
pub(super) const TOOL_PREFIX: &str = "tool · ";
/// True while the chip shows live tool work (not thinking/confirm/input).
/// Used to avoid clobbering a tool label with the think timer.
pub(super) fn is_tool_chip(activity: &str) -> bool {
    activity.starts_with(TOOL_PREFIX)
}

/// Compact tool-activity label for the overlay chip.
///
/// `wave` counts consecutive same-kind starts so parallel reads collapse.
pub(super) fn activity_label(ev: &AgentEvent, wave: &ActivityWave) -> Option<String> {
    match ev {
        AgentEvent::ToolExecutionStart {
            tool_name,
            args_summary,
            ..
        } => {
            let n = wave.count();
            let phrase = if n >= 2 {
                if let Some(kind) = wave.kind() {
                    describe_batch(kind, n, Tense::Present)
                } else {
                    describe_tool(tool_name, args_summary, Tense::Present)
                }
            } else {
                describe_tool(tool_name, args_summary, Tense::Present)
            };
            Some(format!("tool · {phrase}"))
        }
        AgentEvent::ToolExecutionEnd { tool_name, ok, .. } => {
            let n = wave.count();
            let name = if wave.last_name().is_empty() {
                tool_name.as_str()
            } else {
                wave.last_name()
            };
            let phrase = if n >= 2 {
                wave.kind()
                    .map(|kind| describe_batch(kind, n, Tense::Past))
                    .unwrap_or_else(|| describe_tool(name, wave.last_args(), Tense::Past))
            } else {
                describe_tool(name, wave.last_args(), Tense::Past)
            };
            if *ok {
                Some(format!("done · {phrase}"))
            } else {
                Some(format!("fail · {phrase}"))
            }
        }
        // Stdout floods must not replace "Running cargo test". Subagent
        // `via …` lines are useful and stay.
        AgentEvent::ToolProgress { message, .. } => {
            let msg = message.trim();
            if msg.is_empty() {
                return None;
            }
            if msg.starts_with("via ")
                || msg.to_ascii_lowercase().starts_with("research:")
                || msg.to_ascii_lowercase().starts_with("step ")
            {
                let short = truncate_activity(msg, 56);
                Some(format!("tool · {short}"))
            } else {
                None
            }
        }
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
        AgentEvent::NeedsConfirmation { pending } => {
            let phrase = describe_tool(&pending.name, &pending.args_summary, Tense::Present);
            Some(format!("confirm · {phrase}"))
        }
        AgentEvent::NeedsInput { pending } => {
            let label = pending
                .input
                .as_ref()
                .map(|i| i.label.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("on screen");
            Some(format!("input · {label}"))
        }
        _ => None,
    }
}

/// Record a tool start on the wave (same-kind consecutive collapse).
pub(super) fn note_tool_start(wave: &mut ActivityWave, tool_name: &str, args_summary: &str) {
    let _ = wave.on_start(tool_name, args_summary);
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

    fn wave_for(names: &[&str]) -> ActivityWave {
        let mut w = ActivityWave::default();
        for n in names {
            w.on_start(n, "");
        }
        w
    }

    #[test]
    fn activity_label_tools() {
        assert!(is_tool_chip("tool · Reading file.rs"));
        assert!(!is_tool_chip("done · Read file.rs"));
        let start = AgentEvent::ToolExecutionStart {
            call_id: "1".into(),
            tool_name: "bash".into(),
            args_summary: String::new(),
        };
        assert_eq!(
            activity_label(&start, &wave_for(&["bash"])).as_deref(),
            Some("tool · Running a command")
        );

        let start_args = AgentEvent::ToolExecutionStart {
            call_id: "2".into(),
            tool_name: "bash".into(),
            args_summary: "bash (command=ls -la)".into(),
        };
        assert_eq!(
            activity_label(&start_args, &wave_for(&["bash"])).as_deref(),
            Some("tool · Running ls -la")
        );

        let search = AgentEvent::ToolExecutionStart {
            call_id: "3".into(),
            tool_name: "web_search".into(),
            args_summary: "web_search (query=Uttam LinkedIn Dhanbad)".into(),
        };
        assert_eq!(
            activity_label(&search, &wave_for(&["web_search"])).as_deref(),
            Some("tool · Searching Uttam LinkedIn Dhanbad")
        );

        let read = AgentEvent::ToolExecutionStart {
            call_id: "4".into(),
            tool_name: "file_read".into(),
            args_summary:
                "file_read (path=crates/boris-agent/src/finish_gate.rs, offset=471, limit=50)"
                    .into(),
        };
        assert_eq!(
            activity_label(&read, &wave_for(&["file_read"])).as_deref(),
            Some("tool · Reading finish_gate.rs (471-520)")
        );

        let batch = AgentEvent::ToolExecutionStart {
            call_id: "5".into(),
            tool_name: "file_read".into(),
            args_summary: "file_read (path=a.rs)".into(),
        };
        assert_eq!(
            activity_label(
                &batch,
                &wave_for(&["file_read", "file_read", "file_read", "file_read"])
            )
            .as_deref(),
            Some("tool · Reading 4 files")
        );

        let end_ok = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: true,
            duration_ms: 1,
        };
        assert_eq!(
            activity_label(&end_ok, &wave_for(&["bash"])).as_deref(),
            Some("done · Ran a command")
        );

        let end_fail = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: false,
            duration_ms: 1,
        };
        assert_eq!(
            activity_label(&end_fail, &wave_for(&["bash"])).as_deref(),
            Some("fail · Ran a command")
        );

        let noise = AgentEvent::TurnStart { round: 0 };
        assert!(activity_label(&noise, &ActivityWave::default()).is_none());

        let round2 = AgentEvent::TurnStart { round: 1 };
        assert!(activity_label(&round2, &ActivityWave::default()).is_none());

        let tools_next = AgentEvent::MessageEnd {
            role: Role::Assistant,
            preview: "3 tool call(s)".into(),
        };
        assert_eq!(
            activity_label(&tools_next, &ActivityWave::default()).as_deref(),
            Some("thinking · 3 tools next")
        );

        let thoughts = AgentEvent::Reasoning {
            preview: "Need to search first.".into(),
        };
        assert!(
            activity_label(&thoughts, &ActivityWave::default()).is_none(),
            "reasoning uses StatusPicture.thinking, not the activity chip"
        );

        let flood = AgentEvent::ToolProgress {
            call_id: "1".into(),
            tool_name: "bash".into(),
            message: "a".repeat(200),
            byte_total: Some(200),
        };
        assert!(
            activity_label(&flood, &wave_for(&["bash"])).is_none(),
            "stdout chunks must not clobber the command line"
        );
    }
}
