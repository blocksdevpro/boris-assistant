//! Tool images reach the model as a following user message, and a model that
//! rejects images gets the same turn text-only instead of failing.

use std::sync::Mutex;

use async_trait::async_trait;
use boris_ai::{LlmClient, LlmError};
use serde_json::{json, Value};

use super::*;
use crate::tool::{ToolError, ToolMeta};

struct Snap;

#[async_trait]
impl Tool for Snap {
    fn name(&self) -> &str {
        "snap"
    }
    fn description(&self) -> &str {
        "snap"
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{}})
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default().read_only(true)
    }
    async fn execute(
        &self,
        ctx: &crate::tool_context::ToolCallContext,
        _args: Value,
    ) -> Result<String, ToolError> {
        assert!(ctx.attach_image("snap", "image/png", "QUJD".into(), "test frame"));
        Ok("Captured. The image follows in the next message.".into())
    }
}

struct Client {
    requests: Mutex<Vec<Value>>,
    reject_images: bool,
}

#[async_trait]
impl LlmClient for Client {
    async fn complete(&self, messages: Value, _tools: Value) -> Result<Value, LlmError> {
        let has_image = messages.to_string().contains("image_url");
        let round = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages);
            requests.len()
        };
        if round == 1 {
            return Ok(json!({"role":"assistant","content":"","tool_calls":[{
                "id":"s1","type":"function","function":{"name":"snap","arguments":"{}"}}]}));
        }
        if has_image && self.reject_images {
            return Err(LlmError::new(
                "HTTP 404: No endpoints found that support image input",
            ));
        }
        Ok(json!({"role":"assistant","content":"I can see it."}))
    }
}

async fn run(client: &Client) -> (Result<LoopResult, AgentError>, Context) {
    let mut context = Context::new(20);
    context.push(Role::System, "sys");
    context.push(Role::User, "look at this");
    let runtime = ToolRuntime::null();
    let tools: Vec<std::sync::Arc<dyn Tool>> = vec![std::sync::Arc::new(Snap)];
    let state = LoopState {
        context: &mut context,
        tools: &tools,
        runtime: &runtime,
        client,
        activated: None,
    };
    let result = agent_loop(
        state,
        "look at this",
        &AgentLoopConfig::default(),
        vec![],
        0,
        0,
        None,
        None,
        None,
        0,
    )
    .await;
    (result, context)
}

#[tokio::test]
async fn attached_image_follows_the_tool_result_in_the_next_request() {
    let client = Client {
        requests: Mutex::new(Vec::new()),
        reject_images: false,
    };
    let (result, _context) = run(&client).await;
    assert!(result.is_ok());
    let requests = client.requests.lock().unwrap();
    let messages = requests[1].as_array().unwrap();
    let image = messages
        .iter()
        .position(|m| m["content"].is_array())
        .expect("image message");
    assert_eq!(messages[image - 1]["role"], "tool");
    assert_eq!(messages[image]["role"], "user");
    assert_eq!(
        messages[image]["content"][1]["image_url"]["url"],
        "data:image/png;base64,QUJD"
    );
}

#[tokio::test]
async fn image_rejection_retries_the_round_text_only() {
    let client = Client {
        requests: Mutex::new(Vec::new()),
        reject_images: true,
    };
    let (result, context) = run(&client).await;
    match result.unwrap().outcome {
        AgentOutcome::Speak { text, .. } => assert_eq!(text, "I can see it."),
        other => panic!("unexpected {other:?}"),
    }
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 3, "one retry without the image");
    let retry = requests[2].to_string();
    assert!(!retry.contains("image_url"));
    assert!(retry.contains("does not accept images"));
    assert!(!context.has_tool_images());
}
