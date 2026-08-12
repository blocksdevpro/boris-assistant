//! Local date/time tools (no network).

use async_trait::async_trait;
use chrono::{Datelike, Local, Timelike};
use serde_json::{json, Value};

use crate::tool::{truncate_tool_result, Tool, ToolError, ToolKind, ToolMeta, ToolRisk};

/// Returns the current local wall-clock time.
#[derive(Debug, Default, Clone, Copy)]
pub struct GetTimeTool;

#[async_trait]
impl Tool for GetTimeTool {
    fn name(&self) -> &str {
        "get_time"
    }

    fn description(&self) -> &str {
        "get current local time for answering \"what time is it\""
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::System)
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        let now = Local::now();
        let (is_pm, hour) = now.hour12();
        let minute = now.minute();
        let ampm = if is_pm { "PM" } else { "AM" };
        let s = format!("Local time: {hour}:{minute:02} {ampm}");
        Ok(truncate_tool_result(s))
    }
}

/// Returns today's local calendar date.
#[derive(Debug, Default, Clone, Copy)]
pub struct GetDateTool;

#[async_trait]
impl Tool for GetDateTool {
    fn name(&self) -> &str {
        "get_date"
    }

    fn description(&self) -> &str {
        "get today's local date"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn meta(&self) -> ToolMeta {
        ToolMeta::with_risk(ToolRisk::Safe)
            .kind(ToolKind::System)
            .read_only(true)
            .max_concurrency(8)
    }

    async fn execute(
        &self,
        _ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        let now = Local::now();
        let weekday = now.format("%A");
        let month = now.format("%B");
        let day = now.day();
        let year = now.year();
        let s = format!("Today's date: {weekday}, {month} {day}, {year}");
        Ok(truncate_tool_result(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn get_time_returns_ok_non_empty() {
        let tool = GetTimeTool;
        assert_eq!(tool.name(), "get_time");
        let out = tool
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .expect("get_time should succeed");
        assert!(!out.is_empty());
        assert!(out.starts_with("Local time: "), "got: {out}");
    }

    #[tokio::test]
    async fn get_date_returns_ok_non_empty() {
        let tool = GetDateTool;
        assert_eq!(tool.name(), "get_date");
        let out = tool
            .execute(&crate::tool_context::ToolCallContext::new("t"), json!({}))
            .await
            .expect("get_date should succeed");
        assert!(!out.is_empty());
        assert!(out.starts_with("Today's date: "), "got: {out}");
    }
}
