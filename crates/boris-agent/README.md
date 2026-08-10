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
    Agent, AgentOptions, BuiltinToolPaths, CapabilityPreset, OpenRouterClient,
    SandboxConfig, register_builtin_tools_with_preset, AgentOutcome,
};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = OpenRouterClient::from_env()?; // or construct with key + model
let home = dirs_next_home().join(".boris"); // host-specific home
let sandbox = SandboxConfig::for_desktop_mvp(&home);

let mut agent = Agent::from_options(AgentOptions {
    client: Box::new(client),
    system_prompt: "You are Boris, a concise voice assistant.".into(),
    max_tool_rounds: None,
    tools: vec![],
    sandbox: Some(sandbox),
    audit_path: Some(home.join("audit.jsonl")),
    session_id: None,
    trusted_auto_moderate: true,
});

register_builtin_tools_with_preset(
    &mut agent,
    BuiltinToolPaths {
        notes_path: home.join("memory/notes.jsonl"),
        profile_path: home.join("memory/profile.json"),
        sandbox_root: home.join("sandbox"),
        data_roots: vec![home.join("memory"), home.join("sessions")],
        allow_read: boris_agent::default_user_read_roots(),
        allow_write: vec![],
        boris_home: home.clone(),
    },
    true,  // llm extract
    true,  // power tools
    CapabilityPreset::Full,
);

match agent.prompt("What time is it?").await? {
    AgentOutcome::Speak { text, .. } => println!("{text}"),
    AgentOutcome::Silent => {}
    AgentOutcome::NeedsConfirmation { text, pending } => {
        println!("confirm: {text} ({})", pending.name);
        // host: resume_confirmation(&pending.id, approved)
    }
}
# Ok(())
# }
# fn dirs_next_home() -> std::path::PathBuf { std::env::temp_dir() }
```

Prefer crate-root re-exports (`Agent`, `SandboxConfig`, `register_builtin_tools`, …).
Nested modules are public for the pipeline but are not a stability guarantee.

LLM HTTP lives in `boris-ai` (re-exported). Paths come from the host / pipeline.

## Security model

| Layer | What it does |
|-------|----------------|
| **Capability preset** | `VoiceSafe` / `LocalPower` / `Full` filters which tools are registered; adjusts network/shell defaults via `apply_to_sandbox`. |
| **SandboxConfig** | Path roots (`sandbox_root`, `boris_data_roots`, `allow_read` / `allow_write`), `NetworkPolicy`, `ShellPolicy`, risk/HITL thresholds. |
| **Path policy** | All path-like args checked under roots; best-effort canonicalize for symlink escapes; case-insensitive on Windows. Residual TOCTOU between check and open. |
| **ShellPolicy** | `Denied` · `Allowlist` (binary/prefix) · `OpenConfirm`. Bash deny list is best-effort; **HITL is authoritative**. Windows PowerShell fallback uses `-ExecutionPolicy Bypass` for usability only. |
| **NetworkPolicy** | `Off` · `Allowlist` (host/suffix) · `Open`. `Open` still runs SSRF host blocks on `web_fetch` (loopback, RFC1918, link-local, metadata, IPv6 ULA). Redirects re-validated. DNS rebinding residual documented in code. |
| **HITL** | Dangerous tools pause for user yes/no. After grant, runtime **still enforces** path/shell/network hard gates — only the confirmation UI is skipped. |
| **Tool meta** | Production tools set explicit `read_only` / `max_concurrency`; only Read/Search kinds default RO when meta is unset. |

Desktop MVP: `SandboxConfig::for_desktop_mvp` opens network + shell-with-confirm and grants common user document roots for read.

## Multi-tool fan-out (wave scheduling)

One model response can include **many** `tool_calls` in a single assistant message.
There is **no hard cap** on count per message. The loop processes the full batch:

| Mode | When | Behavior |
|------|------|----------|
| **wave scheduling** (default) | batch auto-allowed | read-only tools run in parallel waves (`max_parallel_tools`, default **16**); writes run sequential |
| **legacy join_all** | `wave_scheduling=false` | all auto-allowed tools `join_all` at once |
| **sequential** | any call needs confirm, or batch size 1 | HITL-safe; **batch HITL** groups contiguous same-risk calls of the same shell-ness (writes together, bash together — never mixed) into one yes/no. After the user approves shell once in a turn, later bash in that turn skips the confirm UI (hard gates still apply). |

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
  tool/                Tool trait, ToolMeta, arg helpers, truncation
  tools/               builtin tools (files, web, bash, notes, …)
  session/             SessionStore + transcript
  memory/              profile + long-term MEMORY.md
  skills/              load, catalog, defaults
  …
```

## Tests

```bash
cargo test -p boris-agent --lib
```

Do not run `tests/tool_live_smoke.rs` in default CI (live environment).
