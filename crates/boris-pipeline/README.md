# boris-pipeline

Desktop voice engine for Boris: **one engine thread**, sequential turns, UI status
snapshots. Not a worker mesh and not a Session FSM.

## Turn loop

```text
                    ┌──────────────────────────────────────────┐
                    │              Engine thread               │
                    └──────────────────────────────────────────┘
                                      │
         Start ──► Armed ──► (wake word) ──► Hearing ──► Reading
                                      │                      │
                                      │                      ▼
                              AwaitingReply ◄── Talking ◄── Thinking
                                      │                      │
                                      └──────── (agent) ─────┘
```

| Phase | Meaning |
|-------|---------|
| `Off` | Idle; waiting for `Start` |
| `Quiet` | Brief init / soft idle (UI chrome) |
| `Armed` | Listening for wake |
| `AwaitingReply` | Freeform follow-up (no second wake) |
| `AwaitingConfirm` | Yes/no after a dangerous tool |
| `Hearing` | Mic capture + VAD |
| `Reading` | STT |
| `Thinking` | Agent + tools (+ TTS synth) |
| `Talking` | Playback started |

Wake scoring, VAD capture, STT, and agent orchestration run on the single
engine thread. Sentence TTS inference is handed to one turn-scoped producer so
the engine can continue servicing Stop/device-switch commands and audio events;
the engine remains the sole owner of phases and playback state. While Talking,
a lower wake threshold plus close-talk energy can pause leftover PCM (Armed
liveness is not used — leftover TTS in the mic looks like a speaker); silence
or “continue” resumes from the cut (`voice_barge_in` / `BORIS_BARGE_IN`). Reusable STT
and TTS loader threads live for the engine lifetime instead of being recreated
per turn. Status is pushed for the UI.

`low_memory` releases the outgoing model at each STT→TTS handoff. `balanced`
keeps models warm through an active turn or follow-up chain, then releases both
when Boris returns to idle. `low_latency` may keep both loaded for the powered-on
session.

Each voice turn is appended to `~/.boris/traces/turns.jsonl` on the durable
maintenance lane. Generation latency excludes audible playback. Summarize p50
and p95 locally with `cargo xtask trace-report` (or add `--json`).

## Public surface

| Type | Role |
|------|------|
| [`Engine`](src/engine/mod.rs) | Owns the engine thread join handle |
| [`EngineHandle`](src/engine/mod.rs) | Cloneable command sender (`Start` / `Stop` / `Shutdown` / device switch) |
| [`PipelineConfig`](src/config.rs) / [`LlmPrefs`](src/config.rs) | Host spawn configuration |
| [`StatusPicture`](src/status.rs) | UI DTO (mirrors desktop TS types; `thinking` is the live reasoning tail) |
| [`AppSettings`](src/settings.rs) | Prefs + API key (`config.toml` + `auth.json`) |
| [`PipelineError`](src/error.rs) | Typed errors for settings / install / init |

### Spawn

```rust
use boris_pipeline::{Engine, LlmPrefs, PipelineConfig};

let prefs = LlmPrefs::new(api_key)
    .model("google/gemini-2.5-flash-lite")
    .fast_model("google/gemini-2.5-flash-lite");
let config = PipelineConfig::with_llm(prefs, 44_100, wakeword_bytes, vad_bytes);
let (engine, handle, status_rx) = Engine::spawn(config)?;
handle.start()?;
// … mirror status_rx to UI …
engine.shutdown_and_join(); // preferred on host exit
```

### Shutdown contract

1. Prefer **`Engine::shutdown_and_join`** when the host exits (sends `Shutdown`, then joins).
2. **Dropping `Engine`** also sends `Shutdown` but does **not** join (avoids blocking `Drop`).
3. **`EngineHandle::shutdown`** alone is enough if another owner holds `Engine` and joins later.
4. All of the above are safe if the command channel is already closed.

## `~/.boris` layout

Override root with `BORIS_HOME`.

```text
~/.boris/
  config.toml          # prefs: [models], [capability], [audio], [speech], [agent], [ui]; optional [logging]
  auth.json            # secrets: openrouter_api_key, exa_api_key
  models/
    parakeet/          # STT
    supertone/onnx/    # TTS graphs
    supertone/voices/  # M4.json
    silero/            # optional seed of embedded Silero VAD ONNX
  sessions/desktop/    # voice session transcripts + artifacts/
  memory/              # long-term markdown memory
  skills/              # skill playbooks
  logs/                # boris-desktop.*.log
  workspace/           # sandboxed agent workspace
```

`save_settings` unconditionally rewrites `[models]`, `[capability]`, `[audio]`, `[speech]`,
`[agent]`, and `[ui]`, and conditionally writes `[logging]` (only when a non-empty logging
filter is set, so fresh installs don't get a stray empty section). All other root tables and
unknown keys outside those managed sections are preserved on save (see `save_settings` /
`apply_config_file` in `src/settings.rs`). There is currently no genuinely hand-edit-only
managed table in `config.toml` — anything the desktop settings UI can persist ends up in one
of the sections above.

## Model downloads (`download.rs`)

Each catalog entry enforces a `min_bytes` floor and a mandatory pinned SHA-256
digest. Downloads and existing model files are hashed before being accepted; a
mismatch is discarded or reinstalled. Default Hugging Face sources use pinned
commit revisions.

`BORIS_MODEL_BASE_URL` accepts only `https://` mirrors. Mirror responses must
still match the catalog hash.

## Environment variables

| Var | Purpose |
|-----|---------|
| `BORIS_HOME` | Override data root |
| `OPENROUTER_API_KEY` | LLM key (also `auth.json`) |
| `OPENROUTER_MODEL` / `BORIS_STRONG_MODEL` | Strong model id |
| `BORIS_FAST_MODEL` | Fast model id |
| `BORIS_MODEL_PROVIDER` / `BORIS_STRONG_PROVIDER` | Strong OpenRouter host order |
| `BORIS_FAST_PROVIDER` | Fast host order |
| `BORIS_PIN_PROVIDER` | `1` = no host fallback |
| `BORIS_CAPABILITY` | `voice_safe` \| `local_power` \| `full` |
| `BORIS_MEMORY` | `0` disables long-term memory |
| `BORIS_TRUSTED` | `0` disables auto-allow for moderate tools |
| `BORIS_MODEL_BASE_URL` | Mirror base for `install_models` |
| `BORIS_PROGRESSIVE_TOOLS` / `BORIS_WAVE_SCHEDULING` / `BORIS_MAX_PARALLEL_TOOLS` | Tool runtime |
| `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` | Hugging Face auth for downloads |
| `BORIS_BARGE_IN` | `0` disables wake-word barge-in while Talking |
| `BORIS_AUDIO_FRONTEND` | `0` bypasses capture HPF/AGC/AEC |
| `BORIS_LOG` / `RUST_LOG` | Logging filters (host) |

## Features

| Feature | Default | Effect |
|---------|---------|--------|
| `stt-parakeet` | on | Parakeet STT adapter |
| `tts-supertone` | on | Supertone TTS adapter |

## How to test

```bash
# Typecheck
cargo check -p boris-pipeline

# Unit tests (confirm matching, settings merge, pure helpers)
cargo test -p boris-pipeline

# Single module
cargo test -p boris-pipeline confirm
cargo test -p boris-pipeline settings
```

Integration that needs a mic / ORT DLLs lives in the desktop app (`boris-desktop`),
not in this crate’s unit suite.

## Crate map

| Module | Responsibility |
|--------|----------------|
| `engine` | Turn loop, spawn, shutdown |
| `hear` | Wake wait, VAD capture |
| `status` | UI phase/engine DTOs |
| `paths` | `~/.boris` layout + preflight |
| `settings` | Load/save merge for prefs |
| `download` | Model HTTP install |
| `config` | `PipelineConfig` / `LlmPrefs` |
| `devices` | Device list DTOs |
| `diagnostics` | Startup environment dump |
| `error` | `PipelineError` |
| `prompt` | System prompt text |
