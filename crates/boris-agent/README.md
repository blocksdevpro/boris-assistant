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

Every builtin and host-registered tool is listed on every request, in
registration order. The listing never depends on the request text, so
mishearings, Hinglish, or paraphrases cannot hide a capability, and the
serialized schema table stays byte-identical for the whole session (the
provider prompt-cache prefix stays warm). Tools that become unusable mid-turn
stay listed and refuse the call themselves; for example `present_artifact`
returns a recovery hint once its delivery budget is spent.

Extensions (MCP servers and plugins, `Tool::is_extension`) are the exception.
They stay out of the default listing so a large registry cannot crowd the
schema budget. `tool_search` is listed only when at least one extension is
registered, and it activates matching extensions. Activations are
session-scoped (32-entry LRU, 15-minute expiry), cleared on session bind and
unbind, and recovered by failed turns and checkpoint restores. Discovery
results include each extension's parameter schema, and a call to a registered
extension executes (validated against that schema) even before it is listed,
so a found tool can be called in the very next response.

The serialized table stays within 64 KiB (`MAX_TOOL_SCHEMA_CHARS`). Over
budget, unactivated opt-in extensions go first, then shared builtins, then the
core set (`remember_note`, `present_artifact`, `collect_input`); activated
extensions are kept longest and `tool_search` is removed last.
`force_list_all` and disabling `progressive_listing` list every registered
tool, extensions included.

Run `cargo run -p boris-agent --example tool_selection_audit` for an offline
check that the listing is identical across unrelated requests and fits the
schema budget. It uses actual builtin schemas and scripted model replies and
executes no external model, web, or shell calls. Pass a request-event JSON
export after `--` to use its `data.tools` schemas.

## Workspace awareness

`Agent::set_workspace(WorkspaceProfile)` renders a stable `<workspace>` block
into the system prompt: the Boris sandbox (default cwd for `bash` and file
tools), the shell the `bash` tool really runs (`tools::bash::shell_label`),
readable and writable roots, known projects, and the user's
`~/.boris/BORIS.md` as trusted `<user_instructions>` (bounded to 8,000
characters). `<user_info>` reports the sandbox root and real shell instead of
the process cwd and `COMSPEC`. Projects come from the host; `workspace`
provides bounded, cached git-repository discovery (`discover_git_repos_cached`)
under existing read roots. A project with its own `BORIS.md` is marked
`[has BORIS.md]` and the model reads it before working there. Listing a
project never widens write roots. Path denials list the allowed roots
(`describe_roots`) so the model can recover instead of guessing again.

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
paths; user skills win a name clash. The walk skips the user skills directory
itself (canonical-path compare), which a cwd under the home directory reaches.

Only skill names and descriptions enter the prompt, as a bounded JSON
`<skills_catalog_data>` user-role envelope paired with a small static trusted
`SKILLS_SYSTEM_POLICY`. `load_skill` reads a
full body on demand, so specialized guidance does not expand every turn.
Loaded playbooks are framed by source: the user's own skills (installed or
bundled under `<boris_home>/skills`, plus host-supplied extra paths) read as
guidance to follow where it fits the request, with the current user message
winning. Project skills found by walking up from the working directory stay
framed as untrusted reference data, since a cloned repository could plant one,
and are marked `"project": true` in the catalog. Every body keeps its
`</skill>` closers escaped. `load_skill` takes only `name`
(`additionalProperties: false`) and says it returns playbook text without
running anything; extra keys such as an `args` task string are ignored with a
short hint instead of failing the call.
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

Each schema advertises one canonical name per argument (`bash` takes a
required `command` and `timeout`). Older names models still send (`cmd`,
`shell`, `timeout_secs`; grep's `ignore_case`, `context`, `limit`) are declared
in `Tool::arg_aliases`; the runtime renames them to the canonical key before
validation and policy, and the canonical value wins when both are sent.

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

`file_edit`, and `file_write` over an existing file, require a `file_read` of
that file in the same session and refuse when it changed on disk since (length
and mtime, with a content-hash fallback so a touch does not count). The error
tells the model to read it first. New files need no read, Boris's own writes
refresh the record, and the check runs before HITL (`Tool::precheck`) so the
user is never asked to approve a doomed edit. Path policy and HITL are
unchanged. `file_edit` takes `replace_all: true` to change every occurrence.

File tools offer close sibling-name suggestions when a path is not found. They
do not choose a suggested path automatically. `file_read` attaches PNG (decoded
and downscaled to 1568 px), JPEG, GIF, and WebP images (passed through up to
~3.7 MB) so the model can see them; BMP and PDF get a text envelope (no PDF text
extraction), and the host can open the file on screen.

## Shell sessions and background jobs

`bash`, `bash_output`, and `bash_kill` share the agent's `ShellSession`
(`Agent::shell_session`). The last working directory a `bash` call used is
reused by later calls in the same session until the model sets `cwd` again; a
`cd` inside a command is not tracked (no robust way across Git Bash and the
PowerShell fallback). `run_in_background: true` spawns the command with
stdout and stderr redirected by the OS to `<session_dir>/jobs/<id>.log` and
returns a job id at once, with no timeout. `bash_output` returns output written
since the previous read (head and tail past 120 KB, older bytes stay in the
log) plus the status; `bash_kill` stops the job's whole process tree. At most
four jobs run at once, and a job whose log passes 256 MB is killed when polled.
Jobs are killed on session change, `unbind_session` (session end, engine off,
shutdown), and when the agent is dropped; on Windows each job sits in a Job
Object with kill-on-close, so the OS also kills the tree if Boris crashes.
They are OS processes owned by the tool runtime, not engine workers or a
scheduler. Starting a job is a `bash` call with the same policy and HITL;
`bash_output` is a safe read and `bash_kill` is moderate. Presets that drop
`bash` drop both job tools.

## Images and screen capture

`screen_capture` (Windows, GDI) captures the primary monitor or the active
window, downscales to 1568 px on the long side, and is privacy-gated: it is
`Dangerous` with `requires_confirmation`, so every capture needs the user's
yes, and `VoiceSafe` does not register it. Tools attach images through
`ToolCallContext::attach_image`; the runtime queues them and the loop delivers
them before the next request as a user-role message (text label plus an
`image_url` data-URL part), because tool messages are text-only on many
providers. The tool result says the image follows. Images are ephemeral
(`MessageOrigin::ToolImage`): never persisted, replaced by `[image omitted]`
text at the next human turn, skipped by compaction digests, and counted at a
fixed ~1,600 tokens each instead of their base64 length. If the provider
rejects image input, the loop omits the images and retries the round once. `web_fetch` retries a likely
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

All builtins are listed within the schema budget; `tool_search` only finds
extensions. See [tool listing](#tool-listing) for activation lifetime. Debug
capture includes a `tool_listing` event with names, estimated schema tokens,
and availability changes for each model request. Discovery hits are keyword
matches, not proof of a capability.

## Failed and interrupted turns

Tool side effects are real, so a turn that errors or is cancelled after a tool
executed is not rolled back. The human message, assistant tool calls, and tool
results stay in context and history; any unrun call of the latest batch gets a
failed `Error: not run: turn interrupted` result so the provider sequence stays
valid; an undelivered final reply is dropped; and a short persisted
assistant-role `[Host note: …]` names the tools that ran and how the turn
ended. A turn where nothing executed (synthetic cancellation rows do not count)
is restored to its pre-turn snapshot exactly. Hosts handling a barge-in call
`Agent::abandon_turn(checkpoint)`, which applies the same rule against a
checkpoint taken before the turn; `restore_checkpoint` remains a pure rollback.

## Follow-ups ("continue")

`context::followup` classifies short follow-ups without an LLM call. Fillers
and STT noise ("uh", "okay so", "Tss", "boris") are dropped; a message whose
remaining words all belong to a small follow-up vocabulary and that contains
continue / resume / keep going / go ahead / finish (it) / try again is a
resume, and a question such as "did you finish the task?" is a status check.
Anything with other substance ("continue by searching for Rust news") is a new
request. On a resume or status follow-up, the task state keeps the earlier
objective instead of replacing it with "continue", and a turn-scoped host
control restates that objective verbatim, its status (unfinished,
interrupted, waiting, or reported complete), open todos, and the last
`[Host note: …]` if the previous turn was interrupted. Routing uses the same
classifier.

## Streamed speech

`Agent::set_stream_speech(true)` (off by default; the desktop pipeline turns it
on) feeds each response's content deltas into a `SpeechSegmenter`
(`speech_stream.rs`) and emits `AgentEvent::Speech { signal }`:
`Chunk(text)` (one sanitized sentence), `Cancel` (drop this response's queued
or playing speech), and `Commit` (the chunks so far are the whole accepted
answer). Content sent with tool calls is an on-screen note and is never
committed. Because tool-call deltas can arrive after content, the first
sentence is held until the stream has run `DEFAULT_RELEASE_GRACE` (900 ms) past
a sentence boundary with no tool call, or until the response ends; a tool call
after release sends `Cancel`. Text that may be `[silent]`, tool markup, JSON,
or code is never released early. On acceptance the loop calls `finish`, which
sends the unspoken rest plus `Commit`, or `Cancel` if the accepted reply no
longer starts with what was streamed (the host then speaks the reply itself).
Rejected rounds (markup re-prompt, todo gate) cancel at the next request.

## Staying silent

A model reply of exactly `[silent]` (`SILENCE_TOKEN`) becomes
`AgentOutcome::Silent` when no tool ran this turn, so a host can stay quiet on
background or non-directed speech. After tool work the agent always speaks.

## Finish checks

A content-only reply ends the turn unless one of three non-keyword checks
applies: speech that contains tool markup is rejected with a protocol
reminder (budget 2); two consecutive rounds of only unknown tools inject the
available tool names; and open todos re-prompt (budget 3) only when the turn
already used tools. No gate guesses from the request text that more research
or local work is needed, and no research playbook is injected; skills load
when the model calls `load_skill`.

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
Provider `reasoning_details` on assistant tool-call messages are sent back
unmodified and in order across the tool rounds of the current human turn, so
a reasoning model does not re-plan from scratch each round. A new human turn
(or a history reload) strips them; they are never persisted, summarized, or
spoken, and encrypted payloads are excluded from token estimates.
Summary compaction is lossless — only complete oldest turns fold into a
single `Summary` message and the unsummarized tail is never deleted — with
hysteresis before LLM compaction and smaller mechanical fallbacks.
Mechanical compaction is cache-stable: each tool observation is capped once,
at a fixed 24,000 characters, when it enters context (head and tail, keeping
any saved-output hint, idempotent so it is never rewritten again). Below the
hard budget no earlier message changes, so consecutive requests share a
byte-identical serialized prefix and the provider prompt cache stays warm.
Over the hard budget, old tool chains collapse in one deep pass (down to four,
then two recent turns until under the soft budget) rather than a little more
every round. Requests
estimated past the input budget fail fast with `AgentErrorKind::InputTooLarge`
before any provider call. The summary is structured working memory (the
user's requests near verbatim, decisions, files and paths, artifacts, concrete
findings, open todos including the current todo file, current state, and any
interruption host notes), written on the strong route at the planning stage
with a 6,144-token output cap from a digest of up to 60,000 characters. The
transcript stays framed as untrusted data.

## Security model

| Layer | What it does |
|-------|----------------|
| **Capability preset** | `VoiceSafe` / `LocalPower` / `Full` filters which tools are registered; adjusts network/shell defaults via `apply_to_sandbox`. |
| **SandboxConfig** | Path roots (`sandbox_root`, `boris_data_roots`, `allow_read` / `allow_write`), `NetworkPolicy`, `ShellPolicy`, risk/HITL thresholds. |
| **Path policy** | All path-like args checked under roots; best-effort canonicalize for symlink escapes; case-insensitive on Windows. Residual TOCTOU between check and open. |
| **ShellPolicy** | `Denied` · `Allowlist` (binary/prefix) · `OpenConfirm`. Bash deny list is best-effort; **HITL is authoritative**. Windows prefers Git Bash (`bash -c`, no login profile); WSL `System32\bash.exe` is skipped. PowerShell fallback uses `-ExecutionPolicy Bypass` for usability only. |
| **Shell auto-allow** | User-managed prefixes on top of `OpenConfirm` (`SandboxConfig::shell_auto_allow`; desktop `[agent] shell_auto_allow = ["git status", "cargo check"]`). Skips the yes/no only for a single plain command that equals a prefix or continues it after a space; any `;` `&` `|` backtick `$` `>` `<` or line break anywhere (even quoted) means it still asks. Case-sensitive. Shell `Denied`/`Allowlist`, the bash deny list, and cwd roots still apply. This narrows prompts; it is not a sandbox. |
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

Per user turn, tool **rounds** are capped (`DEFAULT_MAX_TOOL_ROUNDS` = 16, skills = 28;
coding, multi-step, or complex turns get `COMPLEX_MAX_TOOL_ROUNDS` = 40), every
loop invocation also has a 10-minute wall-clock budget (`DEFAULT_TURN_TIME_BUDGET`),
and nothing exceeds `MAX_TOOL_ROUNDS_CEILING` = 64. When the cap or the clock
stops ongoing tool work, tools are withheld and the model gives a short spoken
progress check-in; `LoopResult::budget_exhausted` keeps the task state open
(`Active`) so a "continue" follow-up resumes it.
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
