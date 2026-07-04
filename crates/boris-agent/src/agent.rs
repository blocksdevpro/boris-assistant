use reqwest::blocking::Client;
use serde_json::{Value, json};

pub struct Agent {
    api_key: String,
    client: Client,
}

impl Agent {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: Client::new(),
        }
    }

    /// Send messages + available tools to the LLM.
    /// Returns the raw `choices[0].message` object.
    pub fn request_llm(&self, messages: Value, tools: Value) -> Value {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = json!({
            "model": "google/gemini-2.5-flash-lite",
            "messages": messages,
            "tools": tools,
            // Only call a tool when truly useful; "auto" lets the LLM decide.
            "tool_choice": "auto",
        });

        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .unwrap();

        let json: Value = response.json().unwrap();
        json["choices"][0]["message"].clone()
    }
}
