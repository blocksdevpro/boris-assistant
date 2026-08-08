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

## Multi-tool fan-out (wave scheduling)

One model response can include **many** `tool_calls` in a single assistant message.
There is **no hard cap** on count per message. The loop processes the full batch:

| Mode | When | Behavior |
|------|------|----------|
| **wave scheduling** (default) | batch auto-allowed | read-only tools run in parallel waves (`max_parallel_tools`, default **16**); writes run sequential |
| **legacy join_all** | `wave_scheduling=false` | all auto-allowed tools `join_all` at once |
| **sequential** | any call needs confirm, or batch size 1 | HITL-safe one-by-one (can pause mid-batch) |

“Wave scheduling” used to be called `concurrency_v2` during rollout — that was just
“the second concurrency strategy” (v1 = naive join_all), not a versioned protocol.

Per user turn, tool **rounds** are capped (`DEFAULT_MAX_TOOL_ROUNDS` = 16, skills = 28) — that is rounds of “model → tools”, not tools per message.

Prompt + `ToolMeta::read_only` drive whether the model batches and whether tools join the parallel read wave. Network lookups (`web_search`, `web_fetch`) are read-only so they fan out with file greps/reads.

Host env (pipeline):
- `BORIS_WAVE_SCHEDULING=0` — disable (legacy `BORIS_CONCURRENCY_V2=0` still works)
- `BORIS_MAX_PARALLEL_TOOLS=N` — cap concurrent reads in a wave

## Module map

```text
src/
  agent/               stateful host API
    mod.rs             construction, tools/skills registration
    turn.rs            prompt / HITL resume
    personal.rs        profile + after-turn extract
    prompt_build.rs    system prompt assembly
    options.rs         AgentOptions
  loop_/               pure ReAct (complete → tools → events)
  runtime/             policy, timeout, audit, HITL, listing
  tool/                Tool trait, ToolMeta, arg helpers, truncation
  tools/
    files/             list_dir, file_read/write/edit
    web/               web_search, web_fetch
    bash/              shell
    notes/             remember_note, recall_notes
    grep/ glob/        search tools (+ path_pattern)
    …
  session/
    store/             SessionStore
    transcript/        chat_history.jsonl (Grok wire format)
  memory/
    profile/           UserProfile + personal_context render
    long_term/         MEMORY.md + session logs
    extract/           heuristic + LLM profile deltas
    store.rs           profile.json I/O
  skills/              load, catalog, defaults
  context/             chat history, prune, compact
  routing.rs           fast/strong model routing
  prompt_profile.rs    system prompt sections
  finish_gate.rs       todo completion nudge
```

## Host usage

```rust
use boris_agent::{
    Agent, OpenRouterClient, register_builtin_tools, BuiltinToolPaths, AgentOutcome,
};
```

LLM HTTP lives in `boris-ai` (re-exported). Paths come from the host / pipeline.

## Tests

```bash
cargo test -p boris-agent --lib
```

Do not run `tests/tool_live_smoke.rs` in default CI (live environment).
