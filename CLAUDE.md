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
cargo test -p boris-pipeline --lib

# Single test / module (works with any -p crate)
cargo test -p boris-pipeline confirm
cargo test -p boris-agent some_test_name -- --nocapture

# Typecheck a crate with feature flags
cargo check -p boris-pipeline --features stt-parakeet,tts-supertone

# Full product build (needs wake ONNX + frontend toolchain — see "Build gotcha" below)
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

### Build gotcha: wake model required to compile `boris-desktop`

`boris-desktop` embeds the wake-word ONNX via `include_bytes!` from `assets/models/livekit/boris-large.onnx`. That path is **gitignored** — a bare clone cannot `cargo check -p boris-desktop` or run the desktop app until you place that file locally. Pure library crates (`boris-core` … `boris-pipeline --lib`) don't need it.

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
         boris-core (shared types / errors, dependency-free)
```

| Crate | Role |
|-------|------|
| `boris-core` | Shared audio type aliases, `TurnId`, foundation `Error`/`Result`. Deliberately dependency-free (no cpal/ORT/HTTP/Tokio) so it compiles everywhere. |
| `boris-audio` | cpal capture/playback; resample to 16 kHz mono. RT audio callbacks only `try_send` — never block inside a cpal callback. |
| `boris-sense` | VAD (webrtc-vad) + wake-word (LiveKit open-wake-word ONNX via `ort`). `init_onnx_runtime()` must be called once before constructing wake models. |
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
- `ShellPolicy` (`Denied` / `Allowlist` / `OpenConfirm`): the bash deny-list is **best-effort only** — HITL confirmation is the real control. Never document or market the agent as fully sandboxed (this is called out explicitly in `SECURITY.md`/`CONTRIBUTING.md`).
- `NetworkPolicy::Open` still SSRF-blocks loopback/RFC1918/link-local/metadata hosts on `web_fetch`; residual DNS-rebinding risk is documented in code, not solved.
- HITL (human-in-the-loop) pauses dangerous tool calls for yes/no; after approval the runtime still enforces hard path/shell/network gates.
- **Wave scheduling** (default): one assistant turn can return many `tool_calls`; read-only tools run in parallel waves (`max_parallel_tools`, default 16), writes run sequentially. Falls back to legacy `join_all` or fully sequential HITL-safe mode depending on config/risk. Tunable via `BORIS_WAVE_SCHEDULING`, `BORIS_MAX_PARALLEL_TOOLS`, `BORIS_MAX_CONFIRMS`, `BORIS_TRUSTED`.

### `boris-pipeline` engine phases

`Off → Quiet → Armed → (wake) → Hearing → Reading → Thinking → Talking → AwaitingReply` (plus `AwaitingConfirm` for HITL yes/no). Wake scoring, VAD capture, STT, agent, and TTS all run inline on the single engine thread; status snapshots (`StatusPicture`) are pushed to the UI. Shutdown: prefer `Engine::shutdown_and_join`; `EngineHandle::shutdown` alone is fine if another owner joins later.

### `~/.boris` (product runtime data root, override with `BORIS_HOME`)

```
~/.boris/
  config.toml    # prefs: save_settings rewrites [models], [capability], [audio], [speech],
                 #   [agent], [ui] unconditionally, and [logging] when a filter is set;
                 #   unknown tables/keys outside those are preserved
  auth.json      # secrets: openrouter_api_key, exa_api_key (plaintext — never commit)
  models/        # parakeet/, supertone/onnx/, supertone/voices/
  sessions/      # transcripts
  memory/        # long-term markdown memory
  skills/        # skill playbooks
  logs/          # boris-desktop.*.log (packaged Windows builds have no console)
  workspace/     # sandboxed agent workspace
```

Product runtime prefers `~/.boris/models` (downloaded/bootstrapped), not the repo's gitignored `assets/`.

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
| `BORIS_MODEL_BASE_URL`, `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` | Model download |
| `BORIS_LOG` / `RUST_LOG` | Log filters |

Dev secrets go in `.env` (copy from `.env.example`), gitignored.

## Working conventions

- **Keep low-level crates thin.** `boris-core` has zero heavy deps by design; `boris-inference` has no `ort`/vendor SDKs/Tokio; adapters map failures to `boris_core::Error` at the trait edge. Don't add HTTP/ORT/Tokio deps to a crate whose README says it's deliberately kept small.
- **Import from crate roots**, not internal modules, unless the crate's own README says otherwise (e.g. `boris-agent` marks nested modules public for the pipeline but not a stability guarantee).
- **Never block inside a cpal RT callback** (`boris-audio`) — convert/`try_send` only; do real work on the worker thread.
- Each crate has its own `README.md` with a more detailed module map, public API, and design notes — read the relevant one before making non-trivial changes in that crate.
- Architecture rationale / refactor history: `docs/design/oss-collaboration-refactor.md`.
- PRs should stay single-concern (one crate, one feature slice, or docs) — see `CONTRIBUTING.md`.
