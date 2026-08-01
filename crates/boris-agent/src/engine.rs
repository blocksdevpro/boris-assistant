use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{json, Value};
use tracing::{debug, error, info, warn};

use crate::{
    client::LlmClient,
    context::{Context, Message, Role},
    error::AgentError,
    memory::{
        extract_heuristic, extract_with_llm, should_llm_extract, ProfileStore, UserProfile,
        PERSONAL_CONTEXT_MAX_CHARS,
    },
    observe::TurnReport,
    outcome::AgentOutcome,
    tool::Tool,
};

/// Hard cap on tool-call rounds per user turn. Prevents unbounded ReAct loops
/// if the model keeps requesting tools (or invents unknown ones).
const MAX_TOOL_ROUNDS: usize = 5;

/// Max characters of user text included in turn-start logs (avoids dumping secrets).
const LOG_PREVIEW_CHARS: usize = 80;

/// Optional durable personal context attached to the engine.
struct PersonalMemory {
    store: ProfileStore,
    /// Shared with tools so mid-turn saves stay coherent.
    profile: Arc<Mutex<UserProfile>>,
    /// When true, may spend an extra LLM call after a turn to learn.
    llm_extract: bool,
}

pub struct AgentEngine {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
    /// Persona + channel rules without the dynamic personal_context block.
    base_system_prompt: String,
    personal: Option<PersonalMemory>,
}

/// Truncate `s` for logging; appends `…` when cut.
fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

impl AgentEngine {
    /// Create a new engine.
    ///
    /// `system_prompt` sets the assistant's persona and hard rules.
    /// Tools are optional — register via [`Self::register_tool`] / builtins.
    pub fn new(client: Box<dyn LlmClient>, system_prompt: &str) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, system_prompt);
        Self {
            client,
            tools: vec![],
            context,
            base_system_prompt: system_prompt.to_string(),
            personal: None,
        }
    }

    /// Enable durable personal context stored at `profile_path`.
    ///
    /// Loads existing profile (if any), injects a `<personal_context>` block into
    /// the system message, and runs active extraction after successful turns.
    ///
    /// `llm_extract`: when true, may call the LLM side-channel to learn deeper
    /// facts (extra latency/cost). Heuristics always run.
    pub fn enable_personal_context(
        &mut self,
        profile_path: impl Into<PathBuf>,
        llm_extract: bool,
    ) -> Result<Arc<Mutex<UserProfile>>, String> {
        let store = ProfileStore::new(profile_path);
        let profile = store.load()?;
        let shared = Arc::new(Mutex::new(profile));
        self.personal = Some(PersonalMemory {
            store,
            profile: shared.clone(),
            llm_extract,
        });
        self.refresh_system_prompt();
        info!(path = %self.personal.as_ref().unwrap().store.path().display(), "personal context enabled");
        Ok(shared)
    }

    /// Shared profile handle for registering profile tools.
    pub fn personal_profile(&self) -> Option<Arc<Mutex<UserProfile>>> {
        self.personal.as_ref().map(|p| p.profile.clone())
    }

    pub fn profile_store_path(&self) -> Option<PathBuf> {
        self.personal
            .as_ref()
            .map(|p| p.store.path().to_path_buf())
    }

    /// Compose base persona + personal context into the live system message.
    pub fn refresh_system_prompt(&mut self) {
        let composed = self.composed_system_prompt();
        self.context.set_system(composed);
    }

    fn composed_system_prompt(&self) -> String {
        let Some(mem) = &self.personal else {
            return self.base_system_prompt.clone();
        };
        let block = match mem.profile.lock() {
            Ok(p) => p.render_block(PERSONAL_CONTEXT_MAX_CHARS),
            Err(_) => String::new(),
        };
        if block.is_empty() {
            self.base_system_prompt.clone()
        } else {
            format!("{}\n\n{block}", self.base_system_prompt)
        }
    }

    /// Update the base persona prompt (without losing personal context).
    pub fn set_base_system_prompt(&mut self, system_prompt: &str) {
        self.base_system_prompt = system_prompt.to_string();
        self.refresh_system_prompt();
    }

    /// Register a tool the LLM may call during the ReAct-style loop.
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Register many tools (e.g. the default set from [`crate::tools::builtin_tools`]).
    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.register_tool(tool);
        }
    }

    /// Clear conversation to a fresh system-only context (new session).
    ///
    /// Personal profile is **kept**; only chat history resets.
    /// Tools and the LLM client are left unchanged.
    pub fn reset_conversation(&mut self, system_prompt: &str) {
        self.base_system_prompt = system_prompt.to_string();
        self.context.messages.clear();
        let composed = self.composed_system_prompt();
        self.context.push(Role::System, composed);
    }

    /// Load prior user/assistant/tool messages after the system prompt.
    pub fn load_session_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.base_system_prompt = system_prompt.to_string();
        let composed = self.composed_system_prompt();
        self.context.load_history(&composed, history);
    }

    /// Snapshot messages for saving (clone).
    pub fn export_messages(&self) -> Vec<Message> {
        self.context.messages().to_vec()
    }

    fn tools_json(&self) -> Value {
        if self.tools.is_empty() {
            return Value::Null;
        }
        let list: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name":        t.name(),
                        "description": t.description(),
                        "parameters":  t.parameters(),
                    }
                })
            })
            .collect();
        json!(list)
    }

    /// Back-compat entry used by the pipeline. Delegates to [`Self::run_turn`].
    pub fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn(message)
    }

    /// Primary turn API: one user message → [`AgentOutcome`].
    pub fn run_turn(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.run_turn_with_report(user_text)
            .map(|(outcome, _report)| outcome)
    }

    /// Run one user turn and return both the outcome and a [`TurnReport`].
    ///
    /// After a successful turn, actively updates personal context (heuristics
    /// always; optional LLM extract when enabled and the turn looks personal).
    pub fn run_turn_with_report(
        &mut self,
        user_text: &str,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        // Ensure tools that mutated the shared profile mid-turn are reflected
        // in the system prompt before we start (best-effort).
        if self.personal.is_some() {
            self.refresh_system_prompt();
        }

        let started = Instant::now();
        let preview = log_preview(user_text, LOG_PREVIEW_CHARS);
        info!(
            model = %self.client.model(),
            message_len = user_text.len(),
            preview = %preview,
            "agent turn start"
        );

        let snapshot = self.context.messages.clone();
        self.context.push(Role::User, user_text);

        match self.execute_turn_loop() {
            Ok(TurnLoopResult {
                outcome,
                tool_rounds,
                tools_used,
            }) => {
                let duration = started.elapsed();
                let outcome_label = match &outcome {
                    AgentOutcome::Speak(_) => "speak",
                    AgentOutcome::Silent => "silent",
                };
                let approx_chars_in = self.context.as_json().to_string().len();
                let report = TurnReport {
                    duration,
                    tool_rounds,
                    tools_used: tools_used.clone(),
                    outcome: outcome_label.to_string(),
                    approx_chars_in,
                };
                info!(
                    outcome = outcome_label,
                    duration_ms = duration.as_millis() as u64,
                    tool_rounds,
                    tools = ?tools_used,
                    approx_chars_in,
                    "agent turn end"
                );

                // Active personal context learning (does not affect this turn's speech).
                let assistant_text = match &outcome {
                    AgentOutcome::Speak(s) => s.as_str(),
                    AgentOutcome::Silent => "",
                };
                self.after_turn_learn(user_text, assistant_text, &tools_used);

                Ok((outcome, report))
            }
            Err(e) => {
                self.context.messages = snapshot;
                error!(
                    error = %e,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "agent turn failed"
                );
                Err(e)
            }
        }
    }

    /// Heuristic + optional LLM extract → merge → persist → refresh system prompt.
    fn after_turn_learn(&mut self, user_text: &str, assistant_text: &str, tools_used: &[String]) {
        let Some(mem) = &self.personal else {
            return;
        };
        let llm_extract_enabled = mem.llm_extract;

        // 1) Heuristics on what the user said.
        let mut delta = extract_heuristic(user_text);
        let heuristic_hit = !delta.is_empty();

        // 2) Turns counter + maybe LLM extract.
        let (turns_seen, profile_summary, do_llm) = {
            let Ok(mut p) = mem.profile.lock() else {
                return;
            };
            p.turns_seen = p.turns_seen.saturating_add(1);
            let turns_seen = p.turns_seen;
            let summary = if p.is_empty() {
                "(empty)".to_string()
            } else {
                p.render_block(400)
            };
            let do_llm = llm_extract_enabled
                && should_llm_extract(user_text, tools_used, turns_seen, heuristic_hit);
            (turns_seen, summary, do_llm)
        };

        if do_llm {
            match extract_with_llm(self.client.as_ref(), user_text, assistant_text, &profile_summary)
            {
                Ok(llm_delta) if !llm_delta.is_empty() => {
                    debug!(turns_seen, "personal llm extract produced updates");
                    // Merge: heuristics first, then LLM (LLM can refine name etc.).
                    if let Some(n) = llm_delta.preferred_name.clone() {
                        delta.preferred_name = Some(n);
                    }
                    if let Some(a) = llm_delta.address_as.clone() {
                        delta.address_as = Some(a);
                    }
                    delta.preferences_add.extend(llm_delta.preferences_add);
                    delta.facts_add.extend(llm_delta.facts_add);
                    delta
                        .facts_remove_query
                        .extend(llm_delta.facts_remove_query);
                    delta.ongoing_add.extend(llm_delta.ongoing_add);
                    if llm_delta.ongoing_replace.is_some() {
                        delta.ongoing_replace = llm_delta.ongoing_replace;
                    }
                }
                Ok(_) => {
                    debug!(turns_seen, "personal llm extract empty");
                }
                Err(e) => {
                    warn!(error = %e, "personal llm extract failed");
                }
            }
        }

        if delta.is_empty() && !heuristic_hit {
            // Still persist turns_seen bump.
            if let Some(mem) = &self.personal {
                if let Ok(p) = mem.profile.lock() {
                    let _ = mem.store.save(&p);
                }
            }
            return;
        }

        if let Some(mem) = &self.personal {
            if let Ok(mut p) = mem.profile.lock() {
                let before_empty = p.is_empty();
                delta.apply(&mut p);
                if let Err(e) = mem.store.save(&p) {
                    warn!(error = %e, "failed to save personal profile");
                } else {
                    info!(
                        was_empty = before_empty,
                        name = ?p.preferred_name,
                        facts = p.facts.len(),
                        prefs = p.preferences.len(),
                        "personal context updated"
                    );
                }
            }
        }

        // Tools may also have written; always re-inject after learning.
        self.refresh_system_prompt();
    }

    /// Internal ReAct loop after the user message is already on context.
    fn execute_turn_loop(&mut self) -> Result<TurnLoopResult, AgentError> {
        let mut tools_used: Vec<String> = Vec::new();
        let mut tool_rounds: u32 = 0;

        for round in 0..=MAX_TOOL_ROUNDS {
            let response = self
                .client
                .complete(self.context.as_json(), self.tools_json())?;

            let tool_calls = &response["tool_calls"];
            if let Some(calls) = tool_calls.as_array() {
                if !calls.is_empty() {
                    if round == MAX_TOOL_ROUNDS {
                        return Err(AgentError::tool_loop(format!(
                            "tool loop exceeded {MAX_TOOL_ROUNDS} rounds without a final reply"
                        )));
                    }

                    tool_rounds += 1;
                    self.context.push(Role::Assistant, response.clone());

                    for call in calls {
                        let call_id = call["id"].as_str().unwrap_or("").to_string();
                        let fn_name = call["function"]["name"].as_str().unwrap_or("");
                        let args: Value = serde_json::from_str(
                            call["function"]["arguments"].as_str().unwrap_or("{}"),
                        )
                        .unwrap_or(json!({}));

                        let tool =
                            self.tools
                                .iter()
                                .find(|t| t.name() == fn_name)
                                .ok_or_else(|| {
                                    AgentError::unknown_tool(format!(
                                        "unknown tool requested by model: {fn_name}"
                                    ))
                                })?;

                        tools_used.push(fn_name.to_string());

                        let result = match tool.execute(args) {
                            Ok(output) => output,
                            Err(e) => format!("Error: {}", e.message),
                        };

                        self.context.push(
                            Role::Tool,
                            json!({ "tool_call_id": call_id, "content": result }),
                        );
                    }

                    // Profile tools may have updated shared state — refresh before next LLM hop.
                    if tools_used.iter().any(|n| {
                        n == "save_user_fact"
                            || n == "update_user_profile"
                            || n == "get_user_context"
                    }) {
                        self.refresh_system_prompt();
                    }

                    continue;
                }
            }

            let reply = response["content"]
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
            self.context.push(Role::Assistant, reply.clone());
            let outcome = if reply.is_empty() {
                AgentOutcome::Silent
            } else {
                AgentOutcome::Speak(reply)
            };
            return Ok(TurnLoopResult {
                outcome,
                tool_rounds,
                tools_used,
            });
        }

        Err(AgentError::tool_loop("tool loop exhausted"))
    }
}

struct TurnLoopResult {
    outcome: AgentOutcome,
    tool_rounds: u32,
    tools_used: Vec<String>,
}
