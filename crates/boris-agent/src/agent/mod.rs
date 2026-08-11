//! Agent facade — tau-style state + event bus over the pure [`crate::loop_`].
//!
//! Hosts (pipeline / desktop) call [`Agent::prompt`] and map [`AgentOutcome`]
//! to TTS / HITL. Observability is optional via [`Agent::subscribe`].
//!
//! # Module layout
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`options`] | [`AgentOptions`] construction bag |
//! | [`personal`] | durable personal context attach / extract |
//! | [`prompt_build`] | system prompt assembly |
//! | [`turn`] | `prompt` / resume HITL / finish |

mod options;
mod personal;
mod prompt_build;
mod turn;

pub use options::AgentOptions;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use boris_ai::LlmClient;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::context::{Context, Message, Role};
use crate::memory::LongTermMemory;
use crate::runtime::{
    new_activation_set, ActivationSet, JsonlAuditSink, NullAuditSink, PendingTurn, SandboxConfig,
    ToolRuntime, ToolRuntimeFeatures,
};
use crate::skills::{self, LoadedSkills};
use crate::tool::Tool;
use crate::tools::memory_tools::{memory_tools, SharedLongTermMemory};
use crate::tools::skills_tools::{skill_tools, SharedSkills};
use crate::types::{AgentEvent, EventListener, DEFAULT_MAX_TOOL_ROUNDS, SKILLS_MAX_TOOL_ROUNDS};

use personal::PersonalMemory;

/// Max characters of user text included in turn-start logs.
const LOG_PREVIEW_CHARS: usize = 80;

/// Stateful voice agent: context, tools, runtime, HITL, personal memory.
pub struct Agent {
    client: Arc<dyn LlmClient>,
    tools: Vec<Arc<dyn Tool>>,
    /// Live mirror of `tools` for tool_search / subagent (never a one-shot snapshot).
    shared_tools: Arc<Mutex<Vec<Arc<dyn Tool>>>>,
    /// Session-long progressive listing activations.
    activated: ActivationSet,
    /// Listing / concurrency / progress flags.
    features: ToolRuntimeFeatures,
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
    /// Event bus: `(listener_id, callback)` — unsubscribe removes by id.
    listeners: Arc<Mutex<Vec<(u64, EventListener)>>>,
    next_listener_id: AtomicU64,
    cancel: Option<CancellationToken>,
    /// Shared skill registry (catalog + load_skill tool).
    skills: Option<SharedSkills>,
    /// Cross-session markdown memory (MEMORY.md + session logs).
    long_term: Option<SharedLongTermMemory>,
    /// Todo finish-gate fires remaining (cap).
    finish_gate_remaining: u32,
    /// Sandbox snapshot for subagents.
    sandbox_snapshot: SandboxConfig,
    /// Session-bound todos file (`{session_dir}/todos.json`). None when unbound.
    todos_path: Option<PathBuf>,
    /// Session root for subagent artifacts (shared so tools can observe rebinds).
    subagent_session_root: Arc<Mutex<Option<PathBuf>>>,
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
            trusted_auto_moderate: false,
        })
    }

    pub fn from_options(opts: AgentOptions) -> Self {
        let mut context = Context::new(20);
        context.push(Role::System, opts.system_prompt.as_str());

        let mut policy = opts.sandbox.unwrap_or_default();
        if opts.trusted_auto_moderate {
            policy.trusted_auto_moderate = true;
        }
        let sandbox_snapshot = policy.clone();
        let runtime = match opts.audit_path {
            Some(path) => ToolRuntime::new(policy, Box::new(JsonlAuditSink::new(path))),
            None => ToolRuntime::new(policy, Box::new(NullAuditSink)),
        };

        let tools: Vec<Arc<dyn Tool>> = opts.tools.into_iter().map(Arc::from).collect();
        let shared_tools = Arc::new(Mutex::new(tools.clone()));

        Self {
            client: Arc::from(opts.client),
            tools,
            shared_tools,
            activated: new_activation_set(),
            features: ToolRuntimeFeatures::default(),
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
            next_listener_id: AtomicU64::new(1),
            cancel: None,
            skills: None,
            long_term: None,
            finish_gate_remaining: 3,
            sandbox_snapshot,
            todos_path: None,
            subagent_session_root: Arc::new(Mutex::new(None)),
        }
    }

    /// Sync `shared_tools` mutex from the authoritative `tools` vec.
    fn sync_shared_tools(&self) {
        if let Ok(mut g) = self.shared_tools.lock() {
            *g = self.tools.clone();
        }
    }

    /// Replace runtime feature flags (progressive listing, wave scheduling, …).
    pub fn set_features(&mut self, features: ToolRuntimeFeatures) {
        self.features = features;
        // Ensure tool_search is registered when progressive may be used.
        if self.features.progressive_listing {
            self.ensure_tool_search();
        }
        if self.features.progressive_listing {
            self.inject_progressive_prompt_hint();
        }
    }

    pub fn features(&self) -> &ToolRuntimeFeatures {
        &self.features
    }

    /// Register `tool_search` if missing.
    pub fn ensure_tool_search(&mut self) {
        if self.tools.iter().any(|t| t.name() == "tool_search") {
            return;
        }
        let tool = crate::tools::tool_search::ToolSearchTool::new(
            Arc::clone(&self.shared_tools),
            Arc::clone(&self.activated),
        );
        self.insert_tool(Arc::new(tool));
        info!("tool_search registered");
    }

    /// Shared LLM handle (for subagents / routing).
    pub fn client_arc(&self) -> Arc<dyn LlmClient> {
        Arc::clone(&self.client)
    }

    /// Register the lean `spawn_subagent` tool (read-mostly child loop).
    ///
    /// Shares [`Self::subagent_session_root`] with the tool so
    /// [`Self::set_subagent_session_root`] / [`Self::bind_session`] rebind
    /// child storage without re-registering.
    pub fn enable_subagents(&mut self) {
        if self.tools.iter().any(|t| t.name() == "spawn_subagent") {
            return;
        }
        self.sync_shared_tools();
        let tool = crate::tools::subagent::SpawnSubagentTool::new(
            Arc::clone(&self.client),
            Arc::clone(&self.shared_tools),
            self.sandbox_snapshot.clone(),
            Arc::clone(&self.subagent_session_root),
        );
        self.insert_tool(Arc::new(tool));
        info!("spawn_subagent tool enabled");
    }

    /// Enable markdown memory: global under `memory_root`, session logs under
    /// `sessions_root/{id}/memory.md` when bound.
    ///
    /// Registers `memory_search` / `memory_get`, injects a `<memory>` prompt hint,
    /// and appends each completed turn to the **session** `memory.md` after
    /// [`Self::bind_session`].
    pub fn enable_long_term_memory(
        &mut self,
        memory_root: impl Into<PathBuf>,
    ) -> Result<SharedLongTermMemory, String> {
        self.enable_long_term_memory_with_sessions(memory_root, None)
    }

    /// Same as [`Self::enable_long_term_memory`] with a sessions workspace for
    /// cross-chat search (e.g. `~/.boris/sessions/desktop`).
    pub fn enable_long_term_memory_with_sessions(
        &mut self,
        memory_root: impl Into<PathBuf>,
        sessions_root: Option<PathBuf>,
    ) -> Result<SharedLongTermMemory, String> {
        let mut ltm = LongTermMemory::new(memory_root);
        if let Some(sr) = sessions_root {
            ltm = ltm.with_sessions_root(sr);
        }
        ltm.ensure_dirs().map_err(|e| format!("memory dirs: {e}"))?;
        ltm.set_session_id(self.session_id.clone());
        let shared: SharedLongTermMemory = Arc::new(ltm);
        let already = self.tools.iter().any(|t| t.name() == "memory_search");
        if !already {
            self.register_tools(memory_tools(shared.clone()));
        }
        self.long_term = Some(shared.clone());
        self.sync_shared_tools();
        self.refresh_system_prompt();
        info!(
            root = %shared.root().display(),
            sessions = ?shared.sessions_root().map(|p| p.display().to_string()),
            "long-term markdown memory enabled"
        );
        Ok(shared)
    }

    pub fn long_term_memory(&self) -> Option<SharedLongTermMemory> {
        self.long_term.clone()
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
        self.sync_shared_tools();
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
        self.sandbox_snapshot = policy.clone();
        if let Some(path) = audit_path {
            self.runtime = ToolRuntime::new(policy, Box::new(JsonlAuditSink::new(path)));
        } else {
            self.runtime = ToolRuntime::new(policy, Box::new(NullAuditSink));
        }
    }

    pub fn set_session_id(&mut self, id: Option<String>) {
        self.session_id = id.clone();
        if let Some(ltm) = &self.long_term {
            ltm.set_session_id(id);
        }
    }

    /// Clear progressive-listing activations (session isolation).
    pub fn clear_activations(&mut self) {
        if let Ok(mut g) = self.activated.lock() {
            g.clear();
        }
    }

    /// Bind or clear the session-local todos file path.
    ///
    /// When `Some`, re-registers plan tools against that file (name-dedupe replace).
    /// When `None`, clears the field only — tools keep the last path until next bind
    /// (clean break: no workspace fallback re-registration).
    pub fn set_todos_path(&mut self, path: Option<PathBuf>) {
        self.todos_path = path.clone();
        if let Some(p) = path {
            self.register_tools(crate::tools::plan_tools_at(&p));
        }
    }

    /// Swap the audit sink to a session-local JSONL file, or null when unbound.
    pub fn set_audit_path(&mut self, path: Option<PathBuf>) {
        if let Some(p) = path {
            self.runtime
                .set_audit(Box::new(JsonlAuditSink::new(p)));
        } else {
            self.runtime.set_audit(Box::new(NullAuditSink));
        }
    }

    /// Set the session root used by subagents for session-local artifacts.
    pub fn set_subagent_session_root(&mut self, root: Option<PathBuf>) {
        if let Ok(mut g) = self.subagent_session_root.lock() {
            *g = root;
        }
    }

    /// Shared handle to the subagent session root (live updates via rebind).
    pub fn subagent_session_root(&self) -> Arc<Mutex<Option<PathBuf>>> {
        Arc::clone(&self.subagent_session_root)
    }

    /// Session-bound todos file, if any.
    pub fn todos_path(&self) -> Option<&std::path::Path> {
        self.todos_path.as_deref()
    }

    /// Bind all session-local paths. Best-effort sequential; no multi-step rollback.
    pub fn bind_session(&mut self, session_dir: &std::path::Path, session_id: &str) {
        self.set_session_id(Some(session_id.to_string()));
        self.clear_activations();
        let todos = session_dir.join("todos.json");
        self.set_todos_path(Some(todos));
        self.set_audit_path(Some(session_dir.join("tool_calls.jsonl")));
        self.set_subagent_session_root(Some(session_dir.to_path_buf()));
        if let Some(ltm) = &self.long_term {
            ltm.set_session_dir(Some(session_dir.to_path_buf()));
        }
    }

    /// Unbind session-local paths (session end / go_off).
    ///
    /// Clears session id, activations, todos path field, audit sink, subagent
    /// root, and session memory log binding. Plan tools keep pointing at the last
    /// todos path until the next bind (no workspace todos migration).
    pub fn unbind_session(&mut self) {
        self.set_session_id(None);
        self.clear_activations();
        self.set_todos_path(None);
        self.set_audit_path(None);
        self.set_subagent_session_root(None);
        if let Some(ltm) = &self.long_term {
            ltm.set_session_dir(None);
        }
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
        // Do not keep a turn-scoped shell grant after abort — next turn / retry
        // must re-confirm shell.
        self.runtime.clear_turn_grants();
    }

    /// Alias for [`Self::abort`] (legacy name used by pipeline).
    #[deprecated(note = "use Agent::abort")]
    pub fn cancel_pending(&mut self) {
        self.abort();
    }

    /// Subscribe to agent lifecycle events. Returns an unsubscribe handle.
    ///
    /// Listeners are stored by monotonic id; calling the returned closure
    /// removes that listener (no index-based leak / noop placeholders).
    pub fn subscribe(
        &mut self,
        f: impl Fn(&AgentEvent) + Send + Sync + 'static,
    ) -> impl FnOnce() {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        {
            // Poison-tolerant, same as `emit`: a listener panic during `emit()`
            // must not permanently break future `subscribe()` calls.
            let mut guard = self
                .listeners
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.push((id, Box::new(f)));
        }
        let listeners = Arc::clone(&self.listeners);
        move || {
            if let Ok(mut g) = listeners.lock() {
                g.retain(|(lid, _)| *lid != id);
            }
        }
    }

    fn emit(&self, event: &AgentEvent) {
        if let Ok(guard) = self.listeners.lock() {
            for (_, listener) in guard.iter() {
                listener(event);
            }
        }
    }

    /// Register a tool. If a tool with the same name already exists, it is
    /// **replaced** and a warning is logged (dedupe by name).
    pub fn register_tool(&mut self, tool: Box<dyn Tool>) {
        self.insert_tool(Arc::from(tool));
    }

    /// Register many tools (each name-deduped via [`Self::register_tool`]).
    pub fn register_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        for tool in tools {
            self.insert_tool(Arc::from(tool));
        }
    }

    /// Single mutation path for `tools` + live `shared_tools` mirror.
    fn insert_tool(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if let Some(pos) = self.tools.iter().position(|t| t.name() == name) {
            warn!(%name, "replacing existing tool registration (name dedupe)");
            self.tools[pos] = tool;
        } else {
            self.tools.push(tool);
        }
        self.sync_shared_tools();
    }

    pub fn set_tools(&mut self, tools: Vec<Box<dyn Tool>>) {
        self.tools = tools.into_iter().map(Arc::from).collect();
        self.sync_shared_tools();
    }

    /// Toggle trusted auto-allow for ≤ Moderate tools and Dangerous sandbox writes.
    pub fn set_trusted_auto_moderate(&mut self, on: bool) {
        let mut p = self.runtime.policy().clone();
        p.trusted_auto_moderate = on;
        self.sandbox_snapshot.trusted_auto_moderate = on;
        self.runtime.set_policy(p);
    }

    /// Cap HITL confirmations per user turn (multi-tool budget). Minimum 1.
    pub fn set_max_confirms_per_turn(&mut self, n: u32) {
        let n = n.max(1);
        let mut p = self.runtime.policy().clone();
        p.max_confirms_per_turn = n;
        self.sandbox_snapshot.max_confirms_per_turn = n;
        self.runtime.set_policy(p);
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
    #[deprecated(note = "use Agent::reset")]
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
}

/// Truncate `s` for turn-start logs (appends `…` when clipped).
fn log_preview(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let preview: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}
