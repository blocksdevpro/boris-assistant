# boris-agent

Voice-sized ReAct agent harness: tool loop, policy runtime, memory, sessions.

## Architecture

```text
Host (pipeline / desktop)
  └─ Agent                 agent/
       └─ agent_loop       loop_/
            └─ ToolRuntime runtime/
                 └─ dyn Tool    tools/* + tool/
```

## Host usage (minimal)

```rust
use std::sync::Arc;
use boris_agent::{
    Agent, BuiltinToolPaths, CapabilityPreset, OpenRouterClient,
    SandboxConfig, register_builtin_tools_with_preset, AgentOutcome,
};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
# let api_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
# let model = std::env::var("OPENROUTER_MODEL").ok();
let client = OpenRouterClient::new(api_key, model); // host reads the key env itself
let home = dirs_next_home().join(".boris"); // host-specific home
let mut sandbox = SandboxConfig::for_desktop_mvp(&home)
    .with_trusted_auto_moderate(true);

let mut agent = Agent::new(Box::new(client), "You are Boris, a concise voice assistant.");
agent.enable_memory_store(home.join("memory/memory.sqlite"))?;

// Register tools BEFORE configure_runtime: register_builtin_tools_with_preset
// mutates `sandbox` in place via `CapabilityPreset::apply_to_sandbox` (network/shell
// lockdown for VoiceSafe/LocalPower), so registration and the policy handed to
// `configure_runtime` start consistent.
register_builtin_tools_with_preset(
    &mut agent,
    BuiltinToolPaths {
        notes_path: home.join("memory/notes.jsonl"),
        // Legacy input for one-time migration; canonical runtime state is in
        // memory.sqlite because enable_memory_store was called above.
        profile_path: home.join("memory/profile.json"),
        sandbox_root: home.join("sandbox"),
        data_roots: vec![home.join("memory"), home.join("sessions")],
        allow_read: boris_agent::default_user_read_roots(),
        allow_write: vec![],
        boris_home: home.clone(),
    },
    true,  // llm extract
    true,  // power tools
    &mut sandbox,
    CapabilityPreset::Full,
);

// Now hand the (preset-adjusted) sandbox to the runtime.
agent.configure_runtime(sandbox, Some(home.join("audit.jsonl")));

match agent.prompt("What time is it?").await? {
    AgentOutcome::Speak { text, .. } => println!("{text}"),
    AgentOutcome::Silent => {}
    AgentOutcome::NeedsConfirmation { text, pending } => {
        println!("confirm: {text} ({})", pending.name);
        // host: resume_confirmation(&pending.id, approved)
    }
    AgentOutcome::NeedsInput { text, pending } => {
        println!("input: {text} ({})", pending.name);
        // host: resume_input(&pending.id, value)
    }
}
# Ok(())
# }
# fn dirs_next_home() -> std::path::PathBuf { std::env::temp_dir() }
```

Prefer crate-root re-exports (`Agent`, `SandboxConfig`, `register_builtin_tools`, …).
Nested modules are public for the pipeline but are not a stability guarantee.

LLM HTTP lives in `boris-ai` (re-exported). Paths come from the host / pipeline.

## Tool listing

The host selects builtin tool bundles from the user request before the first
completion. Research gets web and recall tools; coding gets workspace and
planning tools. Other requests add bundles for time, profile, clipboard,
system, artifacts, opening URLs or paths, and skills. This local selection
adds no model request and only lists tools registered by the host.

The shared set contains `tool_search`, `remember_note`, `present_artifact`, and
`collect_input`. Matching plugin categories add at most four tools. Discovery
adds specialized tools for the current objective, including short follow-ups
such as "continue" and "try again". A new objective clears those activations;
failed turns and checkpoint restores recover the preceding set. The existing
32-tool activation cap and 15-minute expiry still apply. Schema order follows
registration order, and the serialized table stays within 64 KiB.

`force_list_all` and disabling `progressive_listing` retain full listing,
subject to the schema budget. Explicit `should_list` opt-ins remain supported.
When the schema budget is exceeded, activated tools take priority over unused
shared tools; `tool_search` is removed last. Progressive turns ensure discovery
is registered even after `Agent::set_tools` replaces the registry.

`Agent::prompt` supplies selection automatically. Direct loop callers can set
`AgentLoopConfig::tool_selection` with
`boris_agent::runtime::ToolSelection::for_request`. Direct listing callers use
`ListToolsContext::selection`. Both fields default to `None`; task traits still
provide a fallback selection. Struct literals must include the new field or use
`..Default::default()`.

Run `cargo run -p boris-agent --example tool_selection_audit` for an offline
comparison of full listing and task selection through `Agent::prompt`. It uses
actual builtin schemas and scripted model replies, reporting estimated schema
tokens and request counts. It executes no external model, web, or shell calls.
Pass a request-event JSON export after `--` to use its `data.tools` schemas.
The default audit excludes optional memory, profile, skill, and plugin tools.
These estimates do not measure live model latency.

## Canonical memory

Hosts should call `enable_memory_store` before registering built-in tools. It
creates the local `memory.sqlite` store and makes the profile/extraction
working set, turn evidence, retrieved records, and memory tools use that one
database. The Desktop host also queues the verified migration of legacy
`profile.json`, `MEMORY.md`, and session `memory.md` inputs.

Migration first stages each source and refines it with the configured LLM. It
only deletes a legacy source after all of its excerpts are refined and verified
in SQLite. Failure leaves the old files in place for retry.

The Markdown fallback exposes only `memory_search_files` and `memory_get_file`.
Its deprecated `memory_search` and `memory_get` aliases have been removed;
those names belong to the canonical store. Enabling the canonical store removes
fallback tools from the registry without deleting their source files.

## Skills

Skills are `SKILL.md` playbooks with `name` and `description` frontmatter.
`ensure_default_skills` installs the bundled starter set under
`<boris_home>/skills` and upgrades a stock file when its frontmatter version is
behind the bundled version (legacy stock files without a version upgrade too;
user forks are left alone). `load_skills` discovers project skills by walking
up from the working directory through every `.boris/skills` to the git root,
then user skills from `<boris_home>/skills`, followed by any explicit extra
paths; the first skill with a given name wins.

Only skill names and descriptions enter the prompt, as a bounded JSON
`<skills_catalog_data>` user-role envelope paired with a small static trusted
`SKILLS_SYSTEM_POLICY`. `load_skill` reads a
full body on demand, so specialized guidance does not expand every turn.
The redundant `list_skills` tool has been removed; the supplied catalog already
contains names and descriptions. The bundled set includes task execution,
research, daily briefs, remembering,
coding, root-cause debugging, code explanation, design rationale (`investigate-why`),
design, change review,
technical writing, mentoring, and skill creation. User intent controls whether
a matching playbook is loaded, and optional steps such as todos, research, or
artifacts are used only when they help produce the requested result.
Simple direct actions do not require a skill-loading round, and playbooks already
supplied in context should not be loaded again. Research reminders ask for missing
evidence rather than fixed search quotas; expected empty absence checks need no retry.

## Tool inputs and results

`collect_input` supports exact values, secrets, large pastes, and numbered
choices. A choice can include up to four short options for the host to speak;
the returned number is resolved to its option before the agent resumes. Hosts
should bind secrets to the masked input field. Secret observations are redacted
when transcripts are persisted.

When a host binds a session with `Agent::bind_session`, tool results that exceed
their result-size budget are saved under that session's `tool_outputs/` directory.
The agent receives a `get_tool_output` hint and can reread the saved result by
path and character offset instead of running the tool again. The store caps each
file at 256K characters and removes files older than seven days on a best-effort
basis.

File tools offer close sibling-name suggestions when a path is not found. They
do not choose a suggested path automatically. `file_read` recognizes common
image and PDF formats, but it does not inspect image pixels or extract PDF
text; the host can open the file on screen. `web_fetch` retries a likely
Cloudflare or bot-protection response once.

Optional `list_dir` / `glob` / `grep` roots accept omission, null, or blank as
the Boris sandbox root; required file targets remain strict. `web_fetch`
converts content to Markdown after dropping script/style, SVG, and navigation
subtrees, preserving article links, headings, and code.

Artifact IDs are existing returned references, not model-chosen creation slugs.
Omitted, null, or blank IDs create a card (`present_artifact`) or read the current
card (`get_artifact`). An unknown update still fails with a creation hint. After
an allowed presentation fails, its full report is preserved in an independent
`artifacts/recovery/` catalog and delivered through an unthrottled host event.
If saving also fails, the host still receives the full body for on-screen display.
The runtime permits one corrected retry, then withholds presentation for the
rest of the turn. Recovery cards remain available through list/get after restart.

Tools selected for the current objective are listed within the schema budget.
Use `tool_search` for a needed capability absent from that list. See
[tool listing](#tool-listing) for bundles and activation lifetime.
`tool_search` distinguishes actual current availability from new activations;
debug capture includes a `tool_listing` event with names,
estimated schema tokens, and availability changes for each model request.
Discovery hits are keyword matches, not proof of a capability. Check their actual
descriptions before using them, and repeat discovery only for a specific new lead.

## Tool execution status

`Tool::execute` returns `Result<String, ToolError>`. Failed operations return
a typed error with useful diagnostics. The runtime carries a `ToolObservation`
in `InvokeResult::Observation` and reads its status before rendering provider
text. For example, a successful file read containing "Error: quoted log line"
remains successful.

Status survives sequential and parallel batches, approval and typed-input
resumes, task-state updates, and `ToolExecutionEnd.ok` events. Post-tool
reminders and research evidence use that status rather than failure-related
words in the output. Timeout guidance distinguishes a safe retry from argument
repair.

Internal tool history stores `_tool_ok` and preserves it through transcript
serialization. Provider messages omit this field. Older history without the
field uses case-insensitive `Error:` and `Error [` prefixes. Current-turn
failure evidence resets at the next human message and remains available even
when the provider context is compacted.

The loop passes that evidence through `CompleteOptions::tool_error` for model
routing. `Some(false)` overrides error-looking successful output; the option
stays inside the host and is never serialized into the provider request.

Nonzero shell exits and missing `grep` search paths return failures with their
diagnostics. MCP calls with `result.isError: true` fail with the full result
object, including text and structured diagnostics.

## Optional MCP host integration

Hosts can load stdio MCP server definitions from `~/.boris/mcp.json`, discover
their tools with `discover_mcp_tools`, and register them with
`register_mcp_tools`. By default, only read-suggestive tool names register.
The file accepts either a `servers` array or an `mcpServers` object. Set
`allow_writes` in a server config to register other names. MCP calls use the
network permission, stay outside the read-only class, and always require human
confirmation. Capability presets still filter them. Results are marked as
untrusted data and truncated before they enter the agent context.

The Desktop host does not configure MCP servers. This API is for hosts that
choose to add that integration.

## Context, prompt hardening, and compaction

Trusted system content stays separate from untrusted user-role data:
personal context, the skills catalog, retrieved records, and task evidence are
wired as user-role messages after conversation history (the small memory hint
rides in the system prompt) so changing task evidence does not rewrite the early
prompt prefix. Ambiguous dictated restrictions (such as repeated negation) are
kept separately as clarification candidates, not settled constraints.
Tool observations arrive as raw text with
pending `<system-reminder>` controls flushed as their own message only once
the batch resolves. Token estimates run over serialized message JSON.
Summary compaction is lossless — only complete oldest turns fold into a
single `Summary` message and the unsummarized tail is never deleted — with
hysteresis before LLM compaction and smaller mechanical fallbacks. Requests
estimated past the input budget fail fast with `AgentErrorKind::InputTooLarge`
before any provider call. Summary-maintenance turns are forced onto the fast
tier with an explicit cap.

## Security model

| Layer | What it does |
|-------|----------------|
| **Capability preset** | `VoiceSafe` / `LocalPower` / `Full` filters which tools are registered; adjusts network/shell defaults via `apply_to_sandbox`. |
| **SandboxConfig** | Path roots (`sandbox_root`, `boris_data_roots`, `allow_read` / `allow_write`), `NetworkPolicy`, `ShellPolicy`, risk/HITL thresholds. |
| **Path policy** | All path-like args checked under roots; best-effort canonicalize for symlink escapes; case-insensitive on Windows. Residual TOCTOU between check and open. |
| **ShellPolicy** | `Denied` · `Allowlist` (binary/prefix) · `OpenConfirm`. Bash deny list is best-effort; **HITL is authoritative**. Windows prefers Git Bash (`bash -c`, no login profile); WSL `System32\bash.exe` is skipped. PowerShell fallback uses `-ExecutionPolicy Bypass` for usability only. |
| **NetworkPolicy** | `Off` · `Allowlist` (host/suffix) · `Open`. `Open` still runs SSRF host blocks on `web_fetch` (loopback, RFC1918, link-local, metadata, IPv6 ULA). Redirects re-validated. DNS rebinding residual documented in code. **`Allowlist` only constrains tools with an addressable URL arg** (e.g. `web_fetch`'s `url`) — it does **not** constrain `web_search`, which has no URL arg and always hits its fixed search backends (DuckDuckGo + Wikipedia, or Exa when a key is set) regardless of policy. |
| **HITL** | Dangerous tools pause for user yes/no. After grant, runtime **still enforces** path/shell/network hard gates — only the confirmation UI is skipped. |
| **Tool meta** | Production tools set explicit `read_only` / `max_concurrency`; only Read/Search kinds default RO when meta is unset (and only when risk ≤ Moderate with no confirm flag). |

Desktop MVP: `SandboxConfig::for_desktop_mvp` opens network + shell-with-confirm and grants common user document roots for read.

## Multi-tool fan-out (wave scheduling)

One model response can include **many** `tool_calls` in a single assistant message.
Native batches have **no hard cap** on count per message. Built-in registration
also exposes an explicit **`parallel`** tool for independent work:

```json
{
  "tool_uses": [
    { "recipient_name": "grep", "parameters": { "pattern": "handle_wake" } },
    { "recipient_name": "glob", "parameters": { "pattern": "**/*test*" } }
  ]
}
```

Use exact listed tool names and ordinary argument objects. The loop expands
this envelope into native child calls **before** it enters context; each child
gets its own call ID and result. No additional model request is needed. The raw
provider response and parent-to-child mapping remain in developer capture.
An envelope accepts 1–32 children; malformed or nested envelopes produce a
repairable error without executing any child. Native and explicit forms share
the same per-child schema validation, path/network/shell policy, audit,
concurrency limits, cancellation, and approval/input pause-and-resume paths.
Never duplicate the same work in both forms or batch result-dependent calls.

The scheduler processes the batch:

| Mode | When | Behavior |
|------|------|----------|
| **wave scheduling** (default) | contiguous auto-approved read-only runs | rolling parallel pool (`max_parallel_tools`, default **16**); writes, approval, and typed input are ordering barriers. Later reads never overtake a write. Parallelism resumes after a barrier. |
| **legacy parallel** | `wave_scheduling=false`, no pausing call | all auto-allowed calls in a bounded rolling pool |
| **sequential / HITL** | ordering barrier, input, or singleton | **batch HITL** groups contiguous same-risk calls of the same shell-ness (writes together, bash together — never mixed) into one yes/no. After the user approves shell once in a turn, later bash skips the confirm UI (hard gates still apply). |

Completion events reflect actual finish order; normal batch observations enter
context in request order. Unexpected approval/input pauses retain completed
sibling results and only reschedule unresolved calls, never replay completed
side effects. Unknown child tools are audited and soft-failed individually.
Developer capture records `parallel_expansion`, `parallel_batch_start`, and
`parallel_batch_end` (IDs, scheduler limit, wall time, failures, pauses, cancellation).
Home/overlay keep an explicit parallel batch activity label while child tools run.

Per user turn, tool **rounds** are capped (`DEFAULT_MAX_TOOL_ROUNDS` = 16, skills = 28).
HITL **confirm budget** defaults to **12** (`max_confirms_per_turn`; host may set via settings / `BORIS_MAX_CONFIRMS`).

**Trusted session** (`trusted_auto_moderate`): auto-allows ≤ Moderate tools **and** Dangerous sandbox `FsWrite` (file_write/file_edit under write roots). Shell, open URL, and Critical still need yes.

Host env (pipeline):
- `BORIS_WAVE_SCHEDULING=0` — disable (legacy `BORIS_CONCURRENCY_V2=0` still works)
- `BORIS_MAX_PARALLEL_TOOLS=N` — cap concurrent reads in a wave
- `BORIS_MAX_CONFIRMS=N` — HITL confirm budget per turn (default 12)
- `BORIS_TRUSTED=0|1` — override trusted auto-moderate

## Module map

```text
src/
  agent/               stateful host API
  loop_/               pure ReAct (complete → tools → events)
  runtime/             policy, timeout, audit, HITL, listing
  tool/                Tool trait, ToolMeta, arg helpers, truncation, output store
  tools/               built-in tools (files, web, bash, notes) + MCP
  session/             SessionStore + transcript + artifacts/ + tool_outputs/
  memory/              canonical ledger + legacy migration support
  skills/              load, catalog, defaults, frontmatter
  …
```

## Tests

```bash
cargo test -p boris-agent --lib
```

Do not run `tests/tool_live_smoke.rs` in routine checks (live environment).
