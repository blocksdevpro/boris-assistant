# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Boris is a Windows-first desktop voice assistant: wake word → listen → transcribe → tool-using agent → speak. The product host is a **Tauri v2** app (`boris-desktop`, in `desktop/`); the voice/agent logic is a Rust Cargo workspace under `crates/`.

**The root `boris-assistant` package (`src/main.rs`) is a retired CLI stub.** `cargo run` at the repo root just prints a message and exits — it is not the product. Never extend it as a voice host. The real entrypoint is `desktop/` → `boris-desktop`, or the library crates directly.

## Commands

### Rust workspace (day-to-day)

```bash
# Library crates — no Tauri UI, no wake ONNX required
cargo test -p boris-core -p boris-ai -p boris-agent --lib
cargo test -p boris-audio -p boris-sense -p boris-inference --lib
cargo test -p boris-pipeline -p boris-tts-supertone --lib

# Single test / module (works with any -p crate)
cargo test -p boris-pipeline confirm
cargo test -p boris-agent some_test_name -- --nocapture

# Typecheck a crate with feature flags
cargo check -p boris-pipeline --features stt-parakeet,tts-supertone

# Full product build (wake/VAD ONNX are tracked assets — no extra setup)
cargo check -p boris-desktop
```

Prefer `cargo test -p boris-core -p boris-ai -p boris-agent --lib` for a quick validation subset; run the rest locally on Windows when touching audio/sense/pipeline.

Do **not** run `crates/boris-agent/tests/tool_live_smoke.rs` in routine local checks — it exercises a live environment.

### Desktop app

```bash
cd desktop
bun install
bun run tauri dev      # dev run
bun run tauri build    # release build
bun run build           # tsc + vite build (frontend only)
```

### Build note: wake/VAD models are tracked assets

`boris-desktop` embeds the wake-word ONNX via `include_bytes!` from `assets/models/livekit/boris-large.onnx` and the Silero graph from `assets/models/silero/silero_vad.onnx`. Both are tracked in git (`.gitignore` un-ignores them), so a bare clone can `cargo check -p boris-desktop` directly. Only the extra `boris-medium` / `just-boris` classifier variants stay gitignored.

## Architecture

Single sequential pipeline, not a worker mesh or event-bus/actor system — this is intentional and repeated across crate docs, so don't reintroduce multi-worker session-FSM patterns.

```
desktop/ (React + Vite + Bun + Tauri)  UI ↔ IPC ↔ boris-desktop (src-tauri)
        │
        ▼
boris-pipeline  — ONE engine thread, sequential turns
  Armed → wake → hear → STT → agent → TTS → play
        │            │            │
        ▼            ▼            ▼
  boris-audio   boris-sense   boris-agent
  (cpal I/O)   (VAD + wake)  (ReAct + tools) ──▶ boris-ai (OpenRouter)
        │            │
        └─────┬──────┘
              ▼
      boris-inference (object-safe STT/TTS trait ports)
              │
     ┌────────┴────────┐
     ▼                  ▼
boris-stt-parakeet  boris-tts-supertone
                    (boris-tts-kokoro = experimental, unused by product)
              │
              ▼
          boris-core  (shared types / errors, only thiserror)
```

| Crate | Role |
|-------|------|
| `boris-core` | Shared audio type aliases, `TurnId`, foundation `Error`/`Result`. Only `thiserror` (no cpal/ORT/HTTP/Tokio) so it compiles everywhere. |
| `boris-audio` | cpal capture/playback; resample to 16 kHz mono. RT audio callbacks only `try_send` — never block inside a cpal callback. |
| `boris-sense` | VAD (Silero ONNX via `ort`) + wake-word (LiveKit open-wake-word ONNX via `ort`). `init_onnx_runtime()` must be called once before constructing wake or VAD sessions. |
| `boris-inference` | Object-safe `SpeechToText`/`TextToSpeech` trait ports only — no concrete models. |
| `boris-stt-parakeet` | NVIDIA Parakeet STT adapter (via `transcribe-rs`), `backend_id = "parakeet"`. |
| `boris-tts-supertone` | Product TTS adapter (Supertonic 3 via `st-tts`), 44.1 kHz mono, `backend_id = "supertone"`. Owns inter-unit silence; runs its own private Tokio runtime via `block_on` on the sync engine thread. |
| `boris-tts-kokoro` | Experimental TTS adapter (Candle), not wired into the product pipeline. |
| `boris-ai` | LLM provider plane: `LlmClient` trait, `OpenRouterClient`, SSE streaming with blocking JSON fallback. No agent loop here — HTTP/parsing only. |
| `boris-agent` | ReAct tool-loop harness: policy runtime, memory, sessions, skills. See below. |
| `boris-pipeline` | Desktop voice engine: the engine thread, turn-phase state machine, `~/.boris` layout, settings, model install/download. |
| `boris-desktop` (`desktop/src-tauri`) | Tauri shell: commands, tray, overlay window, logging, packaging. |

### `boris-agent` internals

```
Host (pipeline / desktop)
  └─ Agent            agent/       stateful host API (prompt(), resume_confirmation())
       └─ agent_loop   loop_/      pure ReAct: complete → tools → events
            └─ ToolRuntime  runtime/   policy, timeout, audit, HITL, concurrency
                 └─ dyn Tool    tools/* + tool/   builtin tools (files, web, bash, notes, memory, skills…)
```

Also: `context/` (message history + compaction), `memory/` (profile + long-term notes), `session/` (SessionStore + transcripts), `skills/` (load/catalog/frontmatter), `capability.rs` (capability presets).

**Security/policy model** (read `crates/boris-agent/README.md` before touching runtime/policy code):
- `CapabilityPreset` (`VoiceSafe` / `LocalPower` / `Full`) filters which tools get registered and sets network/shell defaults.
- `SandboxConfig` defines path roots (`allow_read`/`allow_write`), `NetworkPolicy`, `ShellPolicy`, HITL thresholds. All path-like tool args are checked under configured roots.
- `ShellPolicy` (`Denied` / `Allowlist` / `OpenConfirm`): the bash deny-list is **best-effort only** — HITL confirmation is the real control. On top of `OpenConfirm`, user-managed `[agent] shell_auto_allow` prefixes skip the yes/no only for a single plain command (no `; & | $ > <`, backticks, or newlines anywhere) that starts with a prefix on a word boundary; hard gates still apply. Never document or market the agent as fully sandboxed (this is called out explicitly in `SECURITY.md`/`CONTRIBUTING.md`).
- `NetworkPolicy::Open` still SSRF-blocks loopback/RFC1918/link-local/metadata hosts on `web_fetch`; residual DNS-rebinding risk is documented in code, not solved.
- HITL (human-in-the-loop) pauses dangerous tool calls for yes/no; after approval the runtime still enforces hard path/shell/network gates.
- `bash` keeps a per-session cwd and can start `run_in_background` jobs (`bash_output` / `bash_kill`, max 4 running). They are OS processes owned by the agent's `ShellSession`, not engine workers, and are killed with their process tree on session change, `unbind_session` (engine off / shutdown), and agent drop (Windows Job Object kill-on-close also covers crashes).
- `file_edit`, and `file_write` over an existing file, require a `file_read` of it this session and refuse if it changed on disk since; `Tool::precheck` runs that before HITL.
- **Wave scheduling** (default): one assistant turn can return many `tool_calls`; read-only tools run in parallel waves (`max_parallel_tools`, default 16), writes run sequentially. Falls back to `max_parallel_tools`-bounded legacy `join_all` or fully sequential HITL-safe mode depending on config/risk. Tunable via `BORIS_WAVE_SCHEDULING`, `BORIS_MAX_PARALLEL_TOOLS`, `BORIS_MAX_CONFIRMS`, `BORIS_TRUSTED`.

### `boris-pipeline` engine phases

`Off → Quiet → Armed → (wake) → Hearing → Reading → Thinking → Talking → AwaitingReply` (plus `AwaitingConfirm` for HITL yes/no and `AwaitingInput` for typed input). Turn ordering is single-threaded: wake scoring, VAD capture, and STT run inline on the engine thread while TTS synthesis runs on a dedicated helper thread. During Thinking the agent turn runs on a scoped thread so the engine can still barge-in with wake + live-mic liveness (work keeps running until STT decides; an abandoned turn keeps tool work that already executed via `Agent::abandon_turn`, and only a turn where nothing ran is rolled back); confirm prompts and re-asks use the same barge watch and follow the agent `max_confirms_per_turn` budget (default 12). Status snapshots (`StatusPicture`, latest-wins by monotonic `seq`) are pushed to the UI, including a live reasoning tail while Thinking. Shutdown: prefer `Engine::shutdown_and_join`; `EngineHandle::shutdown` alone is fine if another owner joins later.

Two helpers sit beside the engine thread without breaking the single-pipeline rule: `engine/device_watch` (owned by the engine loop) polls `AudioHealth` once a second and recovers from unplug/re-plug by sending the ordinary `SwitchInput`/`SwitchOutput` commands; `EngineHandle::send` records host switches as the user's pick. `RunSelfCheck` runs the on-device voice-chain check on the engine thread only while idle or Armed — mid-turn hear helpers drop it, and it must never cancel a turn or a pending confirmation.

TTS loading overlaps agent work. The final answer streams into TTS sentence by
sentence while it generates: the agent's `SpeechSegmenter` (`speech_stream.rs`)
turns `ContentDelta`s into sanitized `AgentEvent::Speech` chunks, holding the
first sentence until the stream has gone 900 ms past a sentence boundary with
no tool call (or the response ends); a tool call that starts later sends
`Cancel`, and acceptance sends the rest plus `Commit`. Content beside tool
calls is never committed, and `[silent]`/markup/code is never streamed early.
The engine plays chunks from a channel inside `run_thinking` (feed-mode
`stream_reply`, same Talking barge watch); a committed, fully played answer
skips the normal speech path. Confirm/input resumes do not stream (their reply
is spoken after acceptance). `BORIS_STREAM_SPEECH=0` restores speak-after-accept.

### `~/.boris` (product runtime data root, override with `BORIS_HOME`)

```
~/.boris/
  config.toml    # prefs: save_settings rewrites [models], [capability], [audio], [speech],
                 #   [agent], [ui] unconditionally, and [logging] when a filter is set;
                 #   unknown tables/keys outside those are preserved
  auth.json      # secrets: openrouter_api_key, exa_api_key (plaintext — never commit)
  BORIS.md       # optional user instructions (system prompt <user_instructions>)
  models/        # parakeet/, supertone/onnx/, supertone/voices/, optional livekit/ + silero/ seeds
  sessions/      # transcripts + per-session artifacts/
  memory/        # memory.sqlite canonical store + optional notes.jsonl
  skills/        # skill playbooks
  logs/          # boris.YYYY-MM-DD.log (packaged Windows builds have no console)
  traces/        # turns.jsonl per-turn latency traces
  speaker/       # live.json — wake liveness enroll (acoustic takes)
  state/workspace/ # sandboxed agent workspace
  state/projects.json # daily cache of discovered git repos (known projects)
  state/reminders.json # timers + reminders (agent tools; spoken by the engine while Armed)
```

Optional hand-edited `[workspace]` in `config.toml` (`projects = [...]`, `discover_projects`) lists known projects; they become read roots only and are rendered into the system prompt with the sandbox, real shell, and roots.

Product runtime prefers `~/.boris/models` (downloaded/bootstrapped), not the repo's `assets/` (only the wake classifier + Silero graph are tracked there).

### Key environment variables

| Var | Purpose |
|-----|---------|
| `BORIS_HOME` | Override `~/.boris` root |
| `OPENROUTER_API_KEY` | LLM key (also settable via `auth.json`) |
| `OPENROUTER_MODEL` / `BORIS_STRONG_MODEL` / `BORIS_FAST_MODEL` | Model ids |
| `BORIS_CAPABILITY` | `voice_safe` \| `local_power` \| `full` |
| `BORIS_TRUSTED` | `0` disables auto-allow for moderate-risk tools |
| `BORIS_MEMORY` | `0` disables long-term memory |
| `BORIS_WAVE_SCHEDULING` / `BORIS_MAX_PARALLEL_TOOLS` / `BORIS_MAX_CONFIRMS` | Tool runtime tuning |
| `BORIS_DISCOVER_PROJECTS` | `0` disables git-repo discovery for the known-projects list |
| `BORIS_VAD_THRESHOLD` | Silero speech-probability threshold in `(0, 1]` (default `0.5`) |
| `BORIS_WAKE_LIVENESS` | `0` disables the taught wake filter (TV / speaker playback) |
| `BORIS_BARGE_IN` | `0` disables wake-word barge-in while Talking, Thinking, or confirming |
| `BORIS_WAKE_CHIME` | `0` disables the chime played when a wake is accepted |
| `BORIS_STREAM_SPEECH` | `0` speaks only after the final answer is accepted (no sentence streaming) |
| `BORIS_PROGRESS_LINE` | `0` disables the one spoken progress line on long tool turns (`[speech] progress_line`) |
| `BORIS_AUDIO_FRONTEND` | `0` bypasses capture HPF/AGC/AEC |
| `BORIS_MODEL_BASE_URL`, `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` | Model download |
| `BORIS_MODEL_RESIDENCY` | `low_memory` \| `balanced` \| `low_latency` model residency |
| `BORIS_TTS_VOICE` | TTS voice id |
| `BORIS_LLM_FIRST_BYTE_SECS` / `BORIS_LLM_IDLE_SECS` | LLM stream stall budgets (default 20 / 30 s; `0` disables); a stall is retried once |
| `BORIS_CONTEXT_WINDOW_TOKENS` | LLM context window override |
| `BORIS_MODEL_PROVIDER` / `BORIS_STRONG_PROVIDER` / `BORIS_FAST_PROVIDER` / `BORIS_PIN_PROVIDER` | OpenRouter host order / pinning |
| `BORIS_LOG` / `RUST_LOG` | Log filters |

Dev secrets go in `.env` (copy from `.env.example`), gitignored.

## Working conventions

- **Keep low-level crates thin.** `boris-core` has only `thiserror` by design; `boris-inference` has no `ort`/vendor SDKs/Tokio; adapters map failures to `boris_core::Error` at the trait edge. Don't add HTTP/ORT/Tokio deps to a crate whose README says it's deliberately kept small.
- **Import from crate roots**, not internal modules, unless the crate's own README says otherwise (e.g. `boris-agent` marks nested modules public for the pipeline but not a stability guarantee).
- **Never block inside a cpal RT callback** (`boris-audio`) — convert/`try_send` only; do real work on the worker thread.
- **Never let `ort` load an ONNX Runtime library it cannot use.** With `ort/load-dynamic` (pulled in by `st-tts`), `ort` 2.0.0-rc.12 deadlocks — no panic, no log — on any failed load (missing DLL, too old, not ORT). `boris_sense::init_onnx_runtime` pre-checks the library first (`preflight_dylib`), and `boris-desktop` pins `ORT_DYLIB_PATH` to the DLL beside the exe (Windows 11 ships ORT 1.17 in System32). Keep both until `ort` fixes the re-entrant `Once`. Running `boris-sense` tests in one `cargo test` with `boris-desktop` unifies `load-dynamic`; `lock_ort_for_test` points those builds at `target/<profile>/onnxruntime.dll`.
- **Never block the Tauri UI thread** (`boris-desktop`). Tray/menu handlers and non-`async` commands run on it, and window calls from other threads (overlay `is_visible`, `scale_factor`, …) marshal to it and wait. So: no engine stop/join there (tray Quit stops on a background thread), commands that lock the engine `handle` are `#[tauri::command(async)]`, and `AppState.status` is never held across a UI dispatch — `emit_order` orders delivery instead (lock order `emit_order` → `status`). Breaking this froze the app on tray "Quit Boris".
- **Preserve tool execution status.** Carry `ToolObservation` through runtime,
  batching, and resumes. Read its status before rendering text; successful logs
  can contain error-related words. Internal `_tool_ok` survives history and
  transcripts but stays out of provider messages. Routing uses
  `CompleteOptions::tool_error`, and reminders use host-known status.
- Each crate has its own `README.md` with a more detailed module map, public API, and design notes — read the relevant one before making non-trivial changes in that crate.
- Architecture maps: `desktop/docs/ARCHITECTURE.md` and the per-crate READMEs.
- PRs should stay single-concern (one crate, one feature slice, or docs) — see `CONTRIBUTING.md`.
