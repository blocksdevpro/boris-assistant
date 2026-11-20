//! `timer_set`, `reminder_set`, `reminders_list`, `reminder_cancel`.
//!
//! Low risk, no confirmation: they only edit Boris's own schedule file
//! (`~/.boris/state/reminders.json`). The voice host speaks items when due.

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Value};

use crate::reminders::{
    describe_duration, parse_duration, parse_when, Reminder, ReminderKind, ReminderStore, WhenError,
};
use crate::tool::{
    optional_string, require_object, require_string, Permission, Tool, ToolError, ToolKind,
    ToolMeta, ToolRisk,
};

/// All four tools on one shared store.
pub fn reminder_tools(store: ReminderStore) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(TimerSetTool {
            store: store.clone(),
        }),
        Box::new(ReminderSetTool {
            store: store.clone(),
        }),
        Box::new(RemindersListTool {
            store: store.clone(),
        }),
        Box::new(ReminderCancelTool { store }),
    ]
}

fn write_meta() -> ToolMeta {
    ToolMeta::with_risk(ToolRisk::Safe)
        .kind(ToolKind::Plan)
        .permissions(&[Permission::FsWrite])
        .read_only(false)
        .max_concurrency(1)
}

fn describe(item: &Reminder) -> String {
    let due = item.due_local();
    let now = Local::now();
    let day = if due.date_naive() == now.date_naive() {
        String::new()
    } else {
        format!(" on {}", due.format("%a %b %-d"))
    };
    let left = describe_duration(due - now);
    let label = match item.kind {
        ReminderKind::Timer => "timer",
        ReminderKind::Reminder => "reminder",
    };
    let message = if item.message.is_empty() {
        String::new()
    } else {
        format!(" \"{}\"", item.message)
    };
    format!(
        "#{} {label}{message} at {}{day} (in {left})",
        item.id,
        due.format("%-I:%M %p")
    )
}

pub struct TimerSetTool {
    store: ReminderStore,
}

#[async_trait]
impl Tool for TimerSetTool {
    fn name(&self) -> &str {
        "timer_set"
    }

    fn description(&self) -> &str {
        "Start a countdown timer. Boris says so out loud when it ends, even if the app restarts meanwhile."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "duration": {"type": "string", "description": "How long, e.g. \"10m\", \"90s\", \"1h30m\", \"2 hours\""},
                "message": {"type": "string", "description": "Optional label spoken when it ends, e.g. \"pasta\""}
            },
            "required": ["duration"],
            "additionalProperties": false
        })
    }

    fn meta(&self) -> ToolMeta {
        write_meta()
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let duration = require_string(obj, "duration")?;
        let span = parse_duration(&duration).map_err(ToolError::invalid_args)?;
        let message = optional_string(obj, "message").unwrap_or_default();
        let item = self
            .store
            .add(ReminderKind::Timer, &message, Local::now() + span)
            .map_err(ToolError::failed)?;
        Ok(format!(
            "Timer set for {}: {}.",
            describe_duration(span),
            describe(&item)
        ))
    }
}

pub struct ReminderSetTool {
    store: ReminderStore,
}

#[async_trait]
impl Tool for ReminderSetTool {
    fn name(&self) -> &str {
        "reminder_set"
    }

    fn description(&self) -> &str {
        "Schedule a spoken reminder at a time (\"5pm\", \"21:30\", \"tomorrow 9am\", \"2026-11-20 09:00\") or after a delay (\"in 20 minutes\"). Times are the user's local time. Pass the user's words for a bare hour as is (\"9\", \"at 9:30\"): it means the next 9:00 or 21:00, whichever comes first; if that is ambiguous the tool fails and you must ask the user which time they meant."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "when": {"type": "string", "description": "Local time or delay: \"in 20 minutes\", \"5:30pm\", \"9pm\", \"21:30\", a bare hour like \"9\" or \"9:30\" (next occurrence within 12 hours), \"tomorrow 9am\", \"2026-11-20 09:00\", or RFC 3339"},
                "message": {"type": "string", "description": "What to remind the user about, phrased to be spoken"}
            },
            "required": ["when", "message"],
            "additionalProperties": false
        })
    }

    fn meta(&self) -> ToolMeta {
        write_meta()
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let when = require_string(obj, "when")?;
        let message = require_string(obj, "message")?;
        // An ambiguous bare hour is a failed call telling the model to ask
        // the user; unreadable input is an argument error.
        let due = parse_when(&when, Local::now()).map_err(|error| match error {
            WhenError::Ambiguous(message) => ToolError::failed(message),
            WhenError::Invalid(message) => ToolError::invalid_args(message),
        })?;
        let item = self
            .store
            .add(ReminderKind::Reminder, &message, due)
            .map_err(ToolError::failed)?;
        Ok(format!("Reminder set: {}.", describe(&item)))
    }
}

pub struct RemindersListTool {
    store: ReminderStore,
}

#[async_trait]
impl Tool for RemindersListTool {
    fn name(&self) -> &str {
        "reminders_list"
    }

    fn description(&self) -> &str {
        "List pending timers and reminders, soonest first, with their ids."
    }

    fn parameters(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::Plan)
            .permissions(&[Permission::FsRead])
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        let items = self.store.list();
        if items.is_empty() {
            return Ok("No timers or reminders are pending.".into());
        }
        let lines: Vec<String> = items.iter().map(describe).collect();
        Ok(format!("{} pending:\n{}", items.len(), lines.join("\n")))
    }
}

pub struct ReminderCancelTool {
    store: ReminderStore,
}

#[async_trait]
impl Tool for ReminderCancelTool {
    fn name(&self) -> &str {
        "reminder_cancel"
    }

    fn description(&self) -> &str {
        "Cancel timers or reminders by id (from reminders_list) or by words in their message."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "integer", "description": "Id to cancel"},
                "match": {"type": "string", "description": "Cancel every item whose message contains this text"}
            },
            "additionalProperties": false
        })
    }

    fn meta(&self) -> ToolMeta {
        write_meta()
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        args: Value,
    ) -> Result<String, ToolError> {
        let obj = require_object(&args)?;
        let removed = if let Some(id) = obj.get("id").and_then(Value::as_u64) {
            self.store.cancel(&[id])
        } else if let Some(text) = optional_string(obj, "match") {
            self.store.cancel_matching(&text)
        } else {
            return Err(ToolError::invalid_args(
                "give an id from reminders_list or match text",
            ));
        }
        .map_err(ToolError::failed)?;
        if removed.is_empty() {
            return Err(ToolError::failed(
                "nothing matched; call reminders_list to see ids",
            ));
        }
        let lines: Vec<String> = removed.iter().map(describe).collect();
        Ok(format!(
            "Cancelled {}:\n{}",
            removed.len(),
            lines.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_list_and_cancel_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "boris-reminder-tools-{}-{}.json",
            std::process::id(),
            Local::now().timestamp_millis()
        ));
        let store = ReminderStore::open(&path);
        let tools = reminder_tools(store.clone());
        let ctx = crate::tool_context::ToolCallContext::new("c1".to_string());
        let out = tools[0]
            .execute(&ctx, json!({"duration": "10m", "message": "tea"}))
            .await
            .unwrap();
        assert!(out.starts_with("Timer set for 10 minutes"), "{out}");
        let out = tools[1]
            .execute(&ctx, json!({"when": "in 2 hours", "message": "stretch"}))
            .await
            .unwrap();
        assert!(out.contains("\"stretch\""), "{out}");
        let listed = tools[2].execute(&ctx, json!({})).await.unwrap();
        assert!(listed.starts_with("2 pending"), "{listed}");
        assert!(listed.find("tea").unwrap() < listed.find("stretch").unwrap());
        let err = tools[1]
            .execute(&ctx, json!({"when": "whenever", "message": "x"}))
            .await
            .unwrap_err();
        assert!(err.message().contains("could not read"));
        tools[3]
            .execute(&ctx, json!({"match": "TEA"}))
            .await
            .unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(tools[3].execute(&ctx, json!({"id": 999})).await.is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reminder_tools_are_low_risk_and_never_confirm() {
        let store = ReminderStore::open(std::env::temp_dir().join("boris-reminder-meta.json"));
        for tool in reminder_tools(store) {
            let meta = tool.meta();
            assert_eq!(meta.risk, ToolRisk::Safe, "{}", tool.name());
            assert!(!meta.requires_confirmation, "{}", tool.name());
            assert!(crate::CapabilityPreset::VoiceSafe.allows_tool(tool.as_ref()));
        }
    }
}
