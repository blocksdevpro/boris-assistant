//! Agent facade — tau-style state + event bus over the pure [`crate::loop_`].
//!
//! Hosts (pipeline / desktop) call [`Agent::prompt`] and map [`AgentOutcome`]
//! to TTS / HITL. Observability is optional via [`Agent::subscribe`].

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use boris_ai::LlmClient;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::context::{Context, Message, Role};
use crate::error::AgentError;
use crate::loop_::{self, LoopState};
use crate::memory::{
    extract_heuristic, extract_with_llm, should_llm_extract, ProfileStore, UserProfile,
    PERSONAL_CONTEXT_MAX_CHARS,
};
use crate::observe::TurnReport;
use crate::outcome::AgentOutcome;
use crate::prompt_profile::{PromptContext, UserInfo};
use crate::runtime::{
    PendingTurn, SandboxConfig, ToolRuntime, JsonlAuditSink, NullAuditSink,
};
use crate::skills::{self, LoadedSkills};
use crate::tool::Tool;
use crate::tools::skills_tools::{skill_tools, SharedSkills};
use crate::types::{
    AgentEvent, AgentLoopConfig, EventListener, LoopResult, DEFAULT_MAX_TOOL_ROUNDS,
    SKILLS_MAX_TOOL_ROUNDS,
};

/// Max characters of user text included in turn-start logs.
const LOG_PREVIEW_CHARS: usize = 80;

/// Construction options for [`Agent::from_options`].
pub struct AgentOptions {
    pub client: Box<dyn LlmClient>,
    pub system_prompt: String,
    pub max_tool_rounds: Option<u32>,
    pub tools: Vec<Box<dyn Tool>>,
    pub sandbox: Option<SandboxConfig>,
    pub audit_path: Option<PathBuf>,
    pub session_id: Option<String>,
}

/// Optional durable personal context attached to the agent.
struct PersonalMemory {
    store: ProfileStore,
    profile: Arc<Mutex<UserProfile>>,
    llm_extract: bool,
}

/// Stateful voice agent: context, tools, runtime, HITL, personal memory.
pub struct Agent {
    client: Box<dyn LlmClient>,
    tools: Vec<Box<dyn Tool>>,
    context: Context,
    base_system_prompt: String,
    /// When true, inject live `<user_info>` (OS, cwd, date) into the system prompt.
    include_user_info: bool,
    personal: Option<PersonalMemory>,
    runtime: ToolRuntime,
    pending_turn: Option<PendingTurn>,
    session_id: Option<String>,
    turn_id: Option<String>,
    max_tool_rounds: u32,
    listeners: Arc<Mutex<Vec<EventListener>>>,
    cancel: Option<CancellationToken>,
    /// Shared skill registry (catalog + load_skill tool).
    skills: Option<SharedSkills>,
}

impl Agent {
    /// Create an agent with a client and system prompt (common host path).
    pub fn new(client: Box<dyn LlmClient>, system_prompt: &str) -> Self {
        Self::from_options(AgentOptions {
            client,
            system_prompt: system_prompt.to_string(),
            max_tool_rounds: None,
            tools: vec![],
            sandbox: None,
            audit_path: None,
            session_id: None,
        })
    }

    pub fn from_options(opts: AgentOptions) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, opts.system_prompt.as_str());

        let runtime = match (opts.sandbox, opts.audit_path) {
            (Some(policy), Some(path)) => {
                ToolRuntime::new(policy, Box::new(JsonlAuditSink::new(path)))
            }
            (Some(policy), None) => ToolRuntime::new(policy, Box::new(NullAuditSink)),
            (None, Some(path)) => {
                ToolRuntime::new(SandboxConfig::default(), Box::new(JsonlAuditSink::new(path)))
            }
            (None, None) => ToolRuntime::null(),
        };

        Self {
            client: opts.client,
            tools: opts.tools,
            context,
            base_system_prompt: opts.system_prompt,
            include_user_info: true,
            personal: None,
            runtime,
            pending_turn: None,
            session_id: opts.session_id,
            turn_id: None,
            max_tool_rounds: opts.max_tool_rounds.unwrap_or(DEFAULT_MAX_TOOL_ROUNDS),
            listeners: Arc::new(Mutex::new(Vec::new())),
            cancel: None,
            skills: None,
        }
    }

    /// Toggle `<user_info>` injection (default: on).
    pub fn set_include_user_info(&mut self, include: bool) {
        self.include_user_info = include;
        self.refresh_system_prompt();
    }

    /// Install discovered skills: inject catalog into system prompt, register
    /// `list_skills` / `load_skill`, and raise the tool-round budget for playbooks.
    pub fn enable_skills(&mut self, loaded: LoadedSkills) -> SharedSkills {
        let shared: SharedSkills = Arc::new(Mutex::new(loaded));
        // Avoid double-registering skill tools if called twice.
        let already = self.tools.iter().any(|t| t.name() == "load_skill");
        if !already {
            self.register_tools(skill_tools(shared.clone()));
        }
        if self.max_tool_rounds < SKILLS_MAX_TOOL_ROUNDS {
            self.max_tool_rounds = SKILLS_MAX_TOOL_ROUNDS;
        }
        self.skills = Some(shared.clone());
        self.refresh_system_prompt();
        if let Ok(g) = shared.lock() {
            info!(
                count = g.skills.len(),
                names = ?g.names(),
                "skills enabled"
            );
            for d in &g.diagnostics {
                warn!(path = %d.path.display(), message = %d.message, "skill diagnostic");
            }
        }
        shared
    }

    /// Reload skills from disk paths (project + user home).
    pub fn reload_skills(
        &mut self,
        cwd: Option<&std::path::Path>,
        boris_home: &std::path::Path,
    ) -> SharedSkills {
        let loaded = skills::load_skills(cwd, boris_home, &[], true);
        let existing = self.skills.clone();
        if let Some(shared) = existing {
            if let Ok(mut g) = shared.lock() {
                *g = loaded;
            }
            self.refresh_system_prompt();
            shared
        } else {
            self.enable_skills(loaded)
        }
    }

    pub fn skills(&self) -> Option<SharedSkills> {
        self.skills.clone()
    }

    /// Configure sandbox policy + optional JSONL audit path.
    pub fn configure_runtime(&mut self, policy: SandboxConfig, audit_path: Option<PathBuf>) {
        if let Some(path) = audit_path {
            self.runtime = ToolRuntime::new(policy, Box::new(JsonlAuditSink::new(path)));
        } else {
            self.runtime = ToolRuntime::new(policy, Box::new(NullAuditSink));
        }
    }

    pub fn set_session_id(&mut self, id: Option<String>) {
        self.session_id = id;
    }

    pub fn set_turn_id(&mut self, id: Option<String>) {
        self.turn_id = id;
    }

    pub fn set_max_tool_rounds(&mut self, n: u32) {
        self.max_tool_rounds = n;
    }

    /// True when a confirmation is outstanding (host must resume or abort).
    pub fn has_pending_confirmation(&self) -> bool {
        self.pending_turn.is_some()
    }

    /// Drop pending HITL state and cancel in-flight loop token.
    pub fn abort(&mut self) {
        if let Some(ct) = self.cancel.take() {
            ct.cancel();
        }
        if self.pending_turn.take().is_some() {
            info!("pending tool confirmation aborted");
        }
    }

    /// Alias for [`Self::abort`] (legacy name used by pipeline).
    pub fn cancel_pending(&mut self) {
        self.abort();
    }

    /// Subscribe to agent lifecycle events. Returns an unsubscribe handle.
    pub fn subscribe(
        &mut self,
        f: impl Fn(&AgentEvent) + Send + Sync + 'static,
    ) -> impl FnOnce() {
        let mut guard = self.listeners.lock().unwrap();
        guard.push(Box::new(f));
        let idx = guard.len() - 1;
        let listeners = Arc::clone(&self.listeners);
        move || {
            if let Ok(mut g) = listeners.lock() {
                if idx < g.len() {
                    g[idx] = Box::new(|_| {});
                }
            }
        }
    }

    fn emit(&self, event: &AgentEvent) {
        if let Ok(guard) = self.listeners.lock() {
            for listener in guard.iter() {
                listener(event);
            }
        }
    }

    /// Enable durable personal context stored at `profile_path`.
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
        info!(
            path = %self.personal.as_ref().unwrap().store.path().display(),
            "personal context enabled"
        );
        Ok(shared)
    }

    pub fn personal_profile(&self) -> Option<Arc<Mutex<UserProfile>>> {
        self.personal.as_ref().map(|p| p.profile.clone())
    }

    pub fn profile_store_path(&self) -> Option<PathBuf> {
        self.personal.as_ref().map(|p| p.store.path().to_path_buf())
    }

    pub fn refresh_system_prompt(&mut self) {
        let composed = self.prompt_context().render();
        self.context.set_system(composed);
    }

    /// Build the inspectable prompt profile (Grok-style `PromptContext`).
    pub fn prompt_context(&self) -> PromptContext {
        let personal = self.personal.as_ref().and_then(|mem| {
            mem.profile
                .lock()
                .ok()
                .map(|p| p.render_block(PERSONAL_CONTEXT_MAX_CHARS))
                .filter(|s| !s.is_empty())
        });
        let skills_catalog = self.skills.as_ref().and_then(|shared| {
            shared
                .lock()
                .ok()
                .map(|g| skills::format_skills_catalog(&g.skills))
                .filter(|s| !s.is_empty())
        });
        let mut ctx = PromptContext::new(self.base_system_prompt.clone())
            .with_personal(personal)
            .with_skills(skills_catalog);
        if self.include_user_info {
            ctx = ctx.with_user_info(UserInfo::capture());
        }
        ctx
    }

    fn composed_system_prompt(&self) -> String {
        self.prompt_context().render()
    }

    pub fn set_base_system_prompt(&mut self, system_prompt: &str) {
        self.base_system_prompt = system_prompt.to_string();
        self.refresh_system_prompt();
    }

    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.register_tool(tool);
        }
    }

    pub fn set_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools = tools;
    }

    /// Clear conversation to a fresh system-only context (new session).
    pub fn reset(&mut self, system_prompt: &str) {
        self.abort();
        self.base_system_prompt = system_prompt.to_string();
        self.context.messages.clear();
        let composed = self.composed_system_prompt();
        self.context.push(Role::System, composed);
    }

    /// Alias for [`Self::reset`].
    pub fn reset_conversation(&mut self, system_prompt: &str) {
        self.reset(system_prompt);
    }

    pub fn load_session_history(&mut self, system_prompt: &str, history: Vec<Message>) {
        self.abort();
        self.base_system_prompt = system_prompt.to_string();
        let composed = self.composed_system_prompt();
        self.context.load_history(&composed, history);
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.context.messages = messages;
    }

    pub fn export_messages(&self) -> Vec<Message> {
        self.context.messages().to_vec()
    }

    fn loop_config(&self) -> AgentLoopConfig {
        AgentLoopConfig {
            max_tool_rounds: self.max_tool_rounds,
            session_id: self.session_id.clone(),
            turn_id: self.turn_id.clone(),
        }
    }

    fn make_emit(&self) -> loop_::EmitFn {
        let listeners = Arc::clone(&self.listeners);
        Arc::new(move |event: AgentEvent| {
            if let Ok(guard) = listeners.lock() {
                for listener in guard.iter() {
                    listener(&event);
                }
            }
        })
    }

    /// Primary turn API: one user message → [`AgentOutcome`].
    pub async fn prompt(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt_with_report(user_text)
            .await
            .map(|(outcome, _)| outcome)
    }

    /// Back-compat alias for [`Self::prompt`].
    pub async fn run_turn(&mut self, user_text: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt(user_text).await
    }

    /// Back-compat alias for [`Self::prompt`].
    pub async fn chat(&mut self, message: &str) -> Result<AgentOutcome, AgentError> {
        self.prompt(message).await
    }

    /// Run one user turn and return both the outcome and a [`TurnReport`].
    pub async fn prompt_with_report(
        &mut self,
        user_text: &str,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        if self.pending_turn.is_some() {
            return Err(AgentError::new(
                "cannot start a new turn while a tool confirmation is pending",
            ));
        }

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
        self.emit(&AgentEvent::MessageEnd {
            role: Role::User,
            preview,
        });

        let snapshot = self.context.messages.clone();
        self.context.push(Role::User, user_text);

        let ct = CancellationToken::new();
        self.cancel = Some(ct.clone());
        let config = self.loop_config();
        let emit = self.make_emit();

        let loop_out = {
            let state = LoopState {
                context: &mut self.context,
                tools: &self.tools,
                runtime: &self.runtime,
                client: self.client.as_ref(),
            };
            loop_::agent_loop(
                state,
                user_text,
                &config,
                Vec::new(),
                0,
                0,
                Some(ct),
                Some(emit),
            )
            .await
        };

        self.cancel = None;

        match loop_out {
            Ok(loop_out) => {
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, user_text, loop_out).await
            }
            Err(e) => {
                self.context.messages = snapshot;
                self.pending_turn = None;
                self.emit(&AgentEvent::Error {
                    message: e.to_string(),
                });
                error!(
                    error = %e,
                    duration_ms = started.elapsed().as_millis() as u64,
                    "agent turn failed"
                );
                Err(e)
            }
        }
    }

    /// Continue after the host collected a yes/no for a pending tool.
    pub async fn resume_confirmation(
        &mut self,
        pending_id: &str,
        approved: bool,
    ) -> Result<AgentOutcome, AgentError> {
        self.resume_confirmation_with_report(pending_id, approved)
            .await
            .map(|(o, _)| o)
    }

    /// Same as [`Self::resume_confirmation`] with a [`TurnReport`].
    pub async fn resume_confirmation_with_report(
        &mut self,
        pending_id: &str,
        approved: bool,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let started = Instant::now();
        let pending_turn = self
            .pending_turn
            .take()
            .ok_or_else(|| AgentError::new("no pending tool confirmation to resume"))?;

        if pending_turn.pending.id != pending_id {
            let id = pending_turn.pending.id.clone();
            self.pending_turn = Some(pending_turn);
            return Err(AgentError::new(format!(
                "pending id mismatch: expected `{id}`, got `{pending_id}`"
            )));
        }

        let user_text = pending_turn.user_text.clone();
        let ct = CancellationToken::new();
        self.cancel = Some(ct.clone());
        let config = self.loop_config();
        let emit = self.make_emit();

        let loop_out = {
            let state = LoopState {
                context: &mut self.context,
                tools: &self.tools,
                runtime: &self.runtime,
                client: self.client.as_ref(),
            };
            loop_::resume_pending_tool(
                state,
                pending_turn,
                approved,
                &config,
                Some(emit),
                Some(ct),
            )
            .await
        };

        self.cancel = None;

        match loop_out {
            Ok(loop_out) => {
                self.pending_turn = loop_out.pending_turn.clone();
                self.maybe_refresh_after_tools(&loop_out.tools_used);
                self.finish_loop(started, &user_text, loop_out).await
            }
            Err(e) => {
                error!(error = %e, "agent resume failed");
                self.emit(&AgentEvent::Error {
                    message: e.to_string(),
                });
                Err(e)
            }
        }
    }

    async fn finish_loop(
        &mut self,
        started: Instant,
        user_text: &str,
        loop_out: LoopResult,
    ) -> Result<(AgentOutcome, TurnReport), AgentError> {
        let duration = started.elapsed();
        let outcome_label = match &loop_out.outcome {
            AgentOutcome::Speak { expect_reply, .. } if *expect_reply => "speak_await",
            AgentOutcome::Speak { .. } => "speak",
            AgentOutcome::Silent => "silent",
            AgentOutcome::NeedsConfirmation { .. } => "needs_confirm",
        };
        let approx_chars_in = self.context.as_json().to_string().len();
        let report = TurnReport {
            duration,
            tool_rounds: loop_out.tool_rounds,
            tools_used: loop_out.tools_used.clone(),
            outcome: match &loop_out.outcome {
                AgentOutcome::NeedsConfirmation { .. } => "needs_confirm".into(),
                AgentOutcome::Silent => "silent".into(),
                AgentOutcome::Speak { .. } => "speak".into(),
            },
            approx_chars_in,
        };
        info!(
            outcome = outcome_label,
            duration_ms = duration.as_millis() as u64,
            tool_rounds = loop_out.tool_rounds,
            tools = ?loop_out.tools_used,
            approx_chars_in,
            "agent turn end"
        );

        if !matches!(loop_out.outcome, AgentOutcome::NeedsConfirmation { .. }) {
            let assistant_text = match &loop_out.outcome {
                AgentOutcome::Speak { text, .. } => text.as_str(),
                AgentOutcome::Silent => "",
                AgentOutcome::NeedsConfirmation { .. } => "",
            };
            self.after_turn_learn(user_text, assistant_text, &loop_out.tools_used)
                .await;
        }

        Ok((loop_out.outcome, report))
    }

    async fn after_turn_learn(
        &mut self,
        user_text: &str,
        assistant_text: &str,
        tools_used: &[String],
    ) {
        let Some(mem) = &self.personal else {
            return;
        };
        let llm_extract_enabled = mem.llm_extract;

        let mut delta = extract_heuristic(user_text);
        let heuristic_hit = !delta.is_empty();

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
            match extract_with_llm(
                self.client.as_ref(),
                user_text,
                assistant_text,
                &profile_summary,
            )
            .await
            {
                Ok(llm_delta) if !llm_delta.is_empty() => {
                    debug!(turns_seen, "personal llm extract produced updates");
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

        self.refresh_system_prompt();
    }

    fn maybe_refresh_after_tools(&mut self, tools_used: &[String]) {
        if tools_used.iter().any(|n| {
            n == "save_user_fact" || n == "update_user_profile" || n == "get_user_context"
        }) {
            self.refresh_system_prompt();
        }
    }
}

fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}
