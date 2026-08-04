//! Async tool body execution with wall-clock timeout.

use std::time::Duration;

use serde_json::Value;

use crate::tool::{Tool, ToolError, ToolErrorKind};
use crate::tool_context::ToolCallContext;

/// Await `tool.execute(ctx, args)`, aborting the wait after `timeout`.
///
/// On timeout returns [`ToolError::timeout`]. The underlying future is dropped
/// (cancel-safe tools should honor drop; pure CPU work may still run until the
/// next await point).
pub async fn run_with_timeout(
    tool: &dyn Tool,
    ctx: &ToolCallContext,
    args: Value,
    timeout: Duration,
) -> Result<String, ToolError> {
    let name = tool.name().to_string();
    match tokio::time::timeout(timeout, tool.execute(ctx, args)).await {
        Ok(result) => result,
        Err(_) => Err(ToolError::timeout(format!(
            "tool `{name}` timed out after {}ms",
            timeout.as_millis()
        ))),
    }
}

/// True if this error is a timeout.
pub fn is_timeout(err: &ToolError) -> bool {
    err.kind() == ToolErrorKind::Timeout
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use crate::tool::{Tool, ToolError, ToolMeta};
    use serde_json::json;
    use std::time::Duration;

    struct SlowTool;

    #[async_trait]
    impl Tool for SlowTool {
        fn name(&self) -> &str {
            "slow"
        }
        fn description(&self) -> &str {
            "sleeps"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        fn meta(&self) -> ToolMeta {
            ToolMeta::safe_default().timeout(Duration::from_millis(50))
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok("done".into())
        }
    }

    struct FastTool;

    #[async_trait]
    impl Tool for FastTool {
        fn name(&self) -> &str {
            "fast"
        }
        fn description(&self) -> &str {
            "instant"
        }
        fn parameters(&self) -> Value {
            json!({"type":"object","properties":{},"required":[]})
        }
        async fn execute(
            &self,
            _ctx: &crate::tool_context::ToolCallContext,
            _args: Value,
        ) -> Result<String, ToolError> {
            Ok("ok".into())
        }
    }

    #[tokio::test]
    async fn timeout_fires() {
        let ctx = crate::tool_context::ToolCallContext::new("c1");
        let err = run_with_timeout(&SlowTool, &ctx, json!({}), Duration::from_millis(80))
            .await
            .unwrap_err();
        assert!(is_timeout(&err), "{err}");
    }

    #[tokio::test]
    async fn fast_ok() {
        let ctx = crate::tool_context::ToolCallContext::new("c2");
        let out = run_with_timeout(&FastTool, &ctx, json!({}), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(out, "ok");
    }
}
