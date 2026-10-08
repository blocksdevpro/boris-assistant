//! Compare builtin or exported schemas through Agent::prompt with a scripted client.
//! cargo run -p boris-agent --example tool_selection_audit [-- request-export.json]
//! No web, shell, or external model calls are executed.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use boris_agent::tool::{Tool, ToolError, ToolMeta};
use boris_agent::tool_context::ToolCallContext;
use boris_agent::{Agent, ToolRuntimeFeatures};
use boris_ai::{LlmClient, LlmError};
use serde_json::{json, Value};

struct ExportedTool(Value);

#[async_trait]
impl Tool for ExportedTool {
    fn name(&self) -> &str {
        self.0["function"]["name"].as_str().unwrap()
    }
    fn description(&self) -> &str {
        self.0["function"]["description"].as_str().unwrap_or("")
    }
    fn parameters(&self) -> Value {
        self.0["function"]["parameters"].clone()
    }
    fn meta(&self) -> ToolMeta {
        ToolMeta::safe_default().read_only(true)
    }
    async fn execute(&self, _: &ToolCallContext, _: Value) -> Result<String, ToolError> {
        match self.name() {
            "web_search" => Ok("Search results: Example profile — https://example.com\nVerified identity and website link.".into()),
            "web_fetch" => Ok("Fetched https://example.com: Example profile, verified website.".into()),
            _ => Err(ToolError::failed("offline audit does not execute this tool")),
        }
    }
}

struct ScriptedClient {
    requests: Arc<Mutex<Vec<Value>>>,
    research: bool,
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn complete(&self, _: Value, tools: Value) -> Result<Value, LlmError> {
        let mut requests = self.requests.lock().unwrap();
        let round = requests.len();
        requests.push(tools);
        if self.research && round < 2 {
            let (name, args) = if round == 0 {
                ("web_search", json!({"query":"example website"}))
            } else {
                ("web_fetch", json!({"url":"https://example.com"}))
            };
            Ok(json!({"role":"assistant", "content":"", "tool_calls":[{
                "id":format!("audit_{round}"), "type":"function",
                "function":{"name":name, "arguments":args.to_string()}
            }]}))
        } else {
            Ok(json!({"role":"assistant", "content":"The website is verified."}))
        }
    }
}

fn estimate(tools: &Value) -> usize {
    let text = tools.to_string();
    text.bytes().filter(u8::is_ascii).count().div_ceil(4)
        + text.chars().filter(|ch| !ch.is_ascii()).count()
}

async fn run(tools: &[Value], prompt: &str, progressive: bool, research: bool) -> Value {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut agent = Agent::new(
        Box::new(ScriptedClient {
            requests: requests.clone(),
            research,
        }),
        "offline audit",
    );
    // Compare identical capability registries, including the discovery tool.
    agent.ensure_tool_search();
    agent.set_features(ToolRuntimeFeatures {
        progressive_listing: progressive,
        ..Default::default()
    });
    agent.register_tools(
        tools
            .iter()
            .filter(|tool| tool["function"]["name"] != "list_skills")
            .cloned()
            .map(|tool| Box::new(ExportedTool(tool)) as Box<dyn Tool>)
            .collect(),
    );
    agent.prompt(prompt).await.unwrap();
    let requests = requests.lock().unwrap();
    assert!(
        requests.windows(2).all(|pair| pair[0] == pair[1]),
        "schemas should remain stable within the task"
    );
    json!({
        "model_requests":requests.len(),
        "first_request_schema_tokens_est":estimate(&requests[0]),
        "total_schema_tokens_est":requests.iter().map(estimate).sum::<usize>(),
        "first_request_tools":requests[0].as_array().unwrap().iter()
            .map(|tool| tool["function"]["name"].clone()).collect::<Vec<_>>()
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tools = if let Some(path) = std::env::args().nth(1) {
        let event: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        event["data"]["tools"]
            .as_array()
            .ok_or("export has no tool schemas")?
            .clone()
    } else {
        let root = std::env::temp_dir().join("boris-tool-listing-audit");
        let paths = boris_agent::tools::BuiltinToolPaths {
            notes_path: root.join("notes.jsonl"),
            profile_path: root.join("profile.json"),
            sandbox_root: root.join("sandbox"),
            data_roots: vec![],
            allow_read: vec![],
            allow_write: vec![],
            boris_home: root,
        };
        let mut tools = boris_agent::tools::builtin_tools(&paths);
        tools.extend(boris_agent::tools::fs_tools(&paths));
        tools.extend(boris_agent::tools::os_tools(&paths));
        tools.extend(boris_agent::tools::web_tools());
        tools.extend(boris_agent::tools::bash_tools(&paths));
        tools.extend(boris_agent::tools::plan_tools(&paths));
        tools.extend(boris_agent::tools::artifact_tools(&paths));
        tools.extend(boris_agent::tools::output_tools(&paths));
        tools.push(Box::new(
            boris_agent::tools::collect_input::CollectInputTool,
        ));
        tools.into_iter().map(|tool| json!({"type":"function", "function":{
            "name":tool.name(), "description":tool.description(), "parameters":tool.parameters()
        }})).collect()
    };
    let eager = run(&tools, "look up my website", false, true).await;
    let selected = run(&tools, "look up my website", true, true).await;
    let greeting = run(&tools, "hi", true, false).await;
    assert_eq!(
        eager["model_requests"], selected["model_requests"],
        "selection must not add a discovery round"
    );
    let names = selected["first_request_tools"].as_array().unwrap();
    assert!(names.contains(&json!("web_search")) && names.contains(&json!("web_fetch")));
    assert!(!names.contains(&json!("bash")) && !names.contains(&json!("file_edit")));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "eager":eager, "task_selected":selected, "greeting":greeting,
            "measurement":"Offline scripted task; estimated schema tokens only."
        }))?
    );
    Ok(())
}
