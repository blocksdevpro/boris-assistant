# boris-pipeline

Desktop voice engine for Boris: sequential turns with **single-threaded turn
ordering** plus bounded helpers, UI status snapshots. Not a worker mesh and
not a Session FSM.

## Turn loop

```text
          Start ──► Armed ──► (wake word) ──► Hearing ──► Reading
                                       │                      │
                                       │                      ▼
                               AwaitingReply ◄── Talking ◄── Thinking
                                       │                      │
                                       └──────── (agent) ─────┘
                    (AwaitingConfirm / AwaitingInput can interrupt a turn)
```

| Phase | Meaning |
|-------|---------|
| `Off` | Idle; waiting for `Start` |
| `Quiet` | Brief init / soft idle (UI chrome) |
| `Armed` | Listening for wake |
| `AwaitingReply` | Freeform follow-up (no second wake) |
| `AwaitingConfirm` | Yes/no after a dangerous tool |
| `AwaitingInput` | Typed/pasted overlay/Home input |
| `Hearing` | Mic capture + VAD |
| `Reading` | STT |
| `Thinking` | Agent + tools, TTS preload, then accepted-answer synthesis |
| `Talking` | Playback started |

Wake scoring, VAD capture, and STT run inline on the engine thread. The agent
turn runs on a scoped thread during Thinking so the engine can still service
Stop and barge-in. TTS loading overlaps agent work, but reply synthesis starts
only after the complete final answer is accepted and tool markup is removed.
Streamed model content does not start synthesis or playback. Speculative
first-unit synthesis during answer generation is not implemented.

Sentence TTS synthesis runs on a dedicated `boris-tts-stream`
helper thread fed by one turn-scoped producer;
the engine remains the sole owner of phases and playback state. Bounded helpers
(a 2-worker Tokio runtime, a maintenance worker, two reusable model loaders)
stay off the speech-critical path. While Talking,
a lower wake threshold plus close-talk energy can pause leftover PCM (Armed
 liveness is not used — leftover TTS in the mic looks like a speaker); silence
 or “continue” resumes from the cut. While Thinking, the same toggle uses wake
 plus the Armed live-mic gate; work keeps running until STT decides. Silence
 or a rejected speaker is a no-op; “stop” / “wait” cancels the turn; a new
 request replaces it (`voice_barge_in` / `BORIS_BARGE_IN`). Confirm prompts
 and re-asks use the same barge watch: a wake word mid-prompt stops playback
 and listens for the yes/no instead of auto-denying. Voice confirms follow
 the agent HITL budget (`max_confirms_per_turn`, default 12 via
 `BORIS_MAX_CONFIRMS`); post-confirm tool rounds update the context meter,
 the turn trace, and the artifact peek. Mid-confirm device switches re-speak
 the prompt on the new device, wake-enroll yields the turn, and disconnects
 mark devices dead until the next Start. Reusable STT
 and TTS loader threads live for the engine lifetime instead of being recreated
 per turn. Status is pushed for the UI (latest-wins by monotonic `seq`;
activity is capped at 160 chars, the reasoning tail at 512, and the thinking
tail is kept across Hearing/Reading so the overlay does not flicker).
Engine and agent event publishers update the same locked snapshot before it is
sequenced. The desktop drops snapshots from old engine generations.

Detailed reports stay on screen while speech remains short. Failed card
presentation delivers `fallback_report` with the full body to Home and the
overlay, including when the catalog cannot be read. When storage is writable,
an independent `artifacts/recovery/` catalog keeps the report across restarts.
An initial presentation failure permits one corrected retry before the tool is
withheld for that turn.

The system prompt separates short speech from thorough tool work and detailed
screen reports. Shared rules are stated once; research and retry depth follows
unresolved evidence gaps rather than fixed query or tool-round quotas.

Before each initial model request, the agent selects common tool bundles from
the human objective. The prompt directs the model to use listed tools and
discover a capability only when needed tools are absent. Short follow-ups such
as "continue" retain the preceding objective's selection and discoveries.
See [agent tool listing](../boris-agent/README.md#tool-listing) for details.

Independent tools can use the explicit `parallel` API or native multi-tool calls.
The activity chip shows the live parallel batch and concurrency ceiling; child
completion events remain visible in developer capture. Writes and approval/input
boundaries preserve ordering, and completed calls are never replayed on a late pause.

When Parakeet is warm, freeform capture can re-decode the recorded audio prefix
and show interim text in the overlay. Those partials are advisory; the final
decode remains authoritative. Stable partials can shorten the silence wait.
Confirmation captures skip partial decoding. Set `BORIS_STT_PARTIALS=0` to
disable this behavior. See the environment variable table for cadence and
capture-length controls.

 `low_memory` releases the outgoing model at each STT→TTS handoff (including
 confirm captures). `balanced`
keeps models warm through an active turn or follow-up chain, then releases both
when Boris returns to idle. `low_latency` may keep both loaded for the powered-on
session.

Each voice turn is appended to `~/.boris/traces/turns.jsonl` on the durable
maintenance lane. Generation latency excludes audible playback. Read the JSONL
directly for local p50/p95 summaries.

## Public surface

| Type | Role |
|------|------|
| [`Engine`](src/engine/mod.rs) | Owns the engine thread join handle |
| [`EngineHandle`](src/engine/mod.rs) | Cloneable command sender (`Start` / `Stop` / `Shutdown` / device switch / wake enroll / typed input) |
| [`PipelineConfig`](src/config.rs) / [`LlmPrefs`](src/config.rs) | Host spawn configuration |
| [`StatusPicture`](src/status.rs) | UI DTO (mirrors desktop TS types; `thinking` is the live reasoning tail; `seq` is the latest-wins order key) |
| [`AppSettings`](src/settings.rs) | Prefs + API key (`config.toml` + `auth.json`) |
| [`PipelineError`](src/error.rs) | Typed errors (settings / install / init / IO / other) |

### Spawn

```rust
use boris_pipeline::{Engine, LlmPrefs, PipelineConfig};

let prefs = LlmPrefs::new(api_key)
    .model("google/gemini-2.5-flash-lite")
    .fast_model("google/gemini-2.5-flash-lite");
let config = PipelineConfig::with_llm(prefs, 44_100, wakeword_bytes.to_vec(), vad_bytes.to_vec());
let (engine, handle, status_rx) = Engine::spawn(config)?;
handle.start().expect("engine command channel open");
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
    livekit/           # optional seed of embedded wake classifier
    speaker/           # optional CAM++ speaker model
  sessions/desktop/    # transcripts, artifacts/, and tool_outputs/
  memory/              # memory.sqlite canonical store + optional notes.jsonl
  skills/              # skill playbooks
  logs/                # boris.YYYY-MM-DD.log
  traces/              # turns.jsonl per-turn latency traces
  speaker/             # live.json — wake liveness enroll (acoustic takes)
  state/workspace/     # sandboxed agent workspace
```

`save_settings` unconditionally rewrites `[models]`, `[capability]`, `[audio]`, `[speech]`,
`[agent]`, and `[ui]`, and conditionally writes `[logging]` (only when a non-empty logging
filter is set, so fresh installs don't get a stray empty section). All other root tables and
unknown keys outside those managed sections are preserved on save (see `save_settings` /
`apply_config_file` in `src/settings.rs`). There is currently no genuinely hand-edit-only
managed table in `config.toml` — anything the desktop settings UI can persist ends up in one
of the sections above.

## Boris memory

When `BORIS_MEMORY` is enabled, the engine opens
`~/.boris/memory/memory.sqlite` before it registers personal-memory tools.
Every completed turn is durably ingested as evidence, and proactive recall plus
memory tools use active records from that store.

On beta.2's first launch, the engine queues a background import of legacy
`profile.json`, `MEMORY.md`, workspace memory, and session `memory.md` files.
The configured LLM refines each source into canonical records. Legacy files are
deleted only after all sources are refined and verified; a failed migration
leaves them in place for a later retry.

## Model downloads (`download.rs`)

Each catalog entry enforces a `min_bytes` floor and a mandatory pinned SHA-256
digest. Fresh downloads are hashed before being accepted; a mismatch is
discarded or reinstalled. Already-installed files are skipped by `min_bytes`
size only (no re-hash, to avoid freezing the UI). Default Hugging Face
sources use pinned commit revisions.

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
| `BORIS_MEMORY` | `0` disables canonical Boris memory and legacy migration |
| `BORIS_TRUSTED` | `0` disables auto-allow for moderate tools |
| `BORIS_MAX_CONFIRMS` | HITL confirm budget per turn (default 12) |
| `BORIS_VAD_THRESHOLD` | Silero speech-probability threshold in `(0, 1]` (default `0.5`) |
| `BORIS_WAKE_LIVENESS` | `0` disables the taught wake filter |
| `BORIS_MODEL_RESIDENCY` | `low_memory` \| `balanced` \| `low_latency` model residency |
| `BORIS_TTS_VOICE` | TTS voice id |
| `BORIS_CONTEXT_WINDOW_TOKENS` | LLM context window override |
| `BORIS_MODEL_PROVIDER` / `BORIS_STRONG_PROVIDER` / `BORIS_FAST_PROVIDER` | OpenRouter host order |
| `BORIS_PIN_PROVIDER` | `1` = no host fallback |
| `EXA_API_KEY` / `BORIS_EXA_API_KEY` | Exa search upgrade key |
| `BORIS_MODEL_BASE_URL` | Mirror base for `install_models` |
| `BORIS_PROGRESSIVE_TOOLS` / `BORIS_WAVE_SCHEDULING` / `BORIS_MAX_PARALLEL_TOOLS` | Tool runtime |
| `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` | Hugging Face auth for downloads |
| `BORIS_BARGE_IN` | `0` disables wake-word barge-in while Talking, Thinking, or confirming |
| `BORIS_AUDIO_FRONTEND` | `0` bypasses capture HPF/AGC/AEC |
| `BORIS_STT_PARTIALS` | `0` disables live STT partials during capture (default on when STT is warm) |
| `BORIS_STT_PARTIAL_INTERVAL_MS` | Prefix re-decode cadence in ms, 200–5000 (default 900) |
| `BORIS_STT_PARTIAL_MIN_MS` | Recorded audio before the first partial, 500–10000 ms (default 1500) |
| `BORIS_STT_PARTIAL_MAX` | Prefix re-decodes per turn, 1–24 (default 3) |
| `BORIS_MAX_UTTERANCE_SECS` | Freeform capture cap in seconds, 15–180 (default 30; confirms stay 8) |
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
| `artifacts` | Session artifact store |
| `liveness` | Taught-wake liveness enroll + scoring |
| `env_util` | Env-var parsing helpers |
| `error` | `PipelineError` |
| `prompt` | System prompt text |
