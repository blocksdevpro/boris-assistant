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
Stop and barge-in. TTS loading overlaps agent work (`TtsSlot`, polled without
blocking), and the final answer streams into TTS while it generates: the agent
emits sanitized sentence chunks (`AgentEvent::Speech`), the event subscriber
forwards them into a channel, and `run_thinking` plays them through a
feed-mode `stream_reply` once the model is loaded. The first sentence is held
until no tool call has started for 900 ms past a boundary; a late tool call
cancels that response's speech (playback stops, Thinking resumes), and
acceptance commits the rest. A committed answer that played to the end skips
the normal speech path; anything else (cancelled, device loss, barge-in that
discards) falls back to speaking the accepted reply. Early speech uses the
Talking barge watch, so a barge-in pauses it at once, and the mic tail is
drained before Thinking's wake watch resumes. Only the first Thinking pass of a
turn streams; confirm and typed-input resumes speak after acceptance.
`BORIS_STREAM_SPEECH=0` turns streaming off.

On a long tool turn, once tool work has run for 7 s, Boris speaks one short
progress line: the model's latest progress note (the content it sent with its
tool calls), sanitized and cut to one sentence of at most 120 characters, or
"Still working on it." if no note is usable. It is spoken at most once per
turn, never after anything else was already audible, never once an approval
or typed-input prompt has been raised, and not during confirm/input resumes.
It plays through the same Talking barge watch as replies (AEC reference, no
Armed liveness), so Boris's own voice is not scored as a wake; a barge-in that
resumes drops the rest of the line. `[speech] progress_line` (default on, also
in the desktop settings) or `BORIS_PROGRESS_LINE=0` turns it off.

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
 request replaces it (`voice_barge_in` / `BORIS_BARGE_IN`). An abandoned turn
 keeps any tool work that already executed (its undelivered reply is dropped
 and a host note records the interruption); a turn where nothing ran is
 rolled back to its pre-turn checkpoint (`Agent::abandon_turn`). Before any
 model call the host answers simple intents itself: a fresh-turn "stop",
 "cancel", "never mind", "nothing", or "that's all" (closing phrases also in a
 follow-up window) re-arms silently, and "new session" / "start over" ends the
 session, starts a fresh one, and says so. Background or non-directed speech
 can be answered by the model with `[silent]`, which the host keeps quiet. A
 clip cut by the capture cap (`BORIS_MAX_UTTERANCE_SECS`) reaches the model
 with a host note that it may be incomplete. Confirm prompts
 and re-asks use the same barge watch: a wake word mid-prompt stops playback
 and listens for the yes/no instead of auto-denying. Voice confirms follow
 the agent HITL budget (`max_confirms_per_turn`, default 12 via
 `BORIS_MAX_CONFIRMS`); post-confirm tool rounds update the context meter,
 the turn trace, and the artifact peek. Mid-confirm device switches re-speak
 the prompt on the new device, wake-enroll yields the turn, and a lost audio
 worker (the capture channel itself closing) marks devices dead until the next
 Start. Reusable STT
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

Every built-in tool is listed on every model request in a stable order, so a
misheard or paraphrased request cannot hide a capability and the provider
prompt-cache prefix stays warm. `tool_search` appears only when optional
extension tools are registered (the Desktop host registers none).
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

### Reading first-audio latency

Each trace line carries three top-level fields for speech-end → first audio:

| Field | Meaning |
|-------|---------|
| `speech_end_to_first_audio_ms` | User stopped talking → Boris first audible (progress line, a streamed answer later cancelled, or the answer) |
| `speech_end_to_answer_audio_ms` | User stopped talking → the answer's own first audio (`audio_started`) |
| `answer_speech` | `streamed` (answer played while the agent was still running) or `after_accept` |

Matching events (`t_ms` from the turn start) are `speech_end`,
`progress_audio_started`, `early_audio_started`, `answer_streamed`, and
`audio_started`. Turns without a captured utterance (barge-in replacements,
typed input) have no `speech_end` and leave the fields out; old lines parse
with them absent. To compare before and after a change, take the p50 / p90 of
`speech_end_to_first_audio_ms` over the same kind of turns, and split by
`answer_speech` to see how often streaming applied, for example:

```bash
jq -s '[.[] | .speech_end_to_first_audio_ms // empty] | sort
       | {n: length, p50: .[length/2|floor], p90: .[length*0.9|floor]}'    ~/.boris/traces/turns.jsonl
```

`boris_agent::summarize_traces` returns the same p50 / p90 as
`first_audio_p50_ms` / `first_audio_p90_ms`.

## Timers and reminders

The agent's `timer_set`, `reminder_set`, `reminders_list`, and
`reminder_cancel` tools (low risk, no confirmation, every preset) keep a
schedule in `~/.boris/state/reminders.json`, which survives restarts. The
engine shares the same in-process `ReminderStore`. While Armed,
`wait_for_wake` checks it once a second on the engine thread and returns
`HearBreak::RemindersDue`; the engine speaks every due item (earliest first)
as one host line, shows it as `said` with a `reminder` activity chip, removes
it, and re-arms. Items that come due during a turn are spoken at the next gap:
right after Talking when the follow-up window (AwaitingReply) opens, before the
reply mic starts, or otherwise at Armed. `reminder_gap_open` is the gate: never
Hearing/Reading/Thinking/Talking, AwaitingConfirm, AwaitingInput, a pending
agent confirmation, or a queued barge-in turn. Playback is `speak_host_line`
in both places (no barge watch; Stop ends it). The spoken line is then added to
the conversation with `Agent::record_host_event` (a persisted `[Host event:
...]` note, not a human turn) and synced to the transcript, so "what was that
reminder?" works. While Boris is off items wait for the next Start. An item
more than two minutes late says when it was due ("Reminder from 3:15 PM: …").

## Devices

A `boris-device-watch` helper thread checks the open streams and the OS device
list once a second (about 8 ms per check) and sends the engine the same
`SwitchInput` / `SwitchOutput` commands the settings picker uses, so every
phase applies recovery the way it already applies a user switch:

| Situation | What happens |
|-----------|--------------|
| Nothing picked (`"default"`) | Follow the Windows default; cpal reroutes the stream itself |
| Default stream died (no default left, driver reset) | Reopen on the default when one exists |
| Picked device unplugged | Fall back to the default within about a second |
| Picked device plugged back in | Switch back to it |
| Picked device keeps failing to open | Use the default for 30 s, then try again |
| Speaker lost mid-reply | Reply playback aborts instead of waiting out its drain deadline |

`EngineHandle::send` records host `SwitchInput` / `SwitchOutput` requests as
the user's pick (`"default"` or empty = follow the default); watcher-sent
switches do not change the pick. A pick that is not connected is kept without
an error and used once it appears. The engine thread owns the watcher guard,
so the watcher is stopped and joined whenever the engine loop exits (including
by panic); engine shutdown still goes through `Shutdown` as before.

The watcher reads each stream through `AudioHealth::stream(direction)`, one
consistent snapshot (device id, follows-default, failed), so a switch landing
mid-check is never seen half-applied.

## Wake chime

When a wake is accepted (after the liveness gate, before capture), the engine
plays a 190 ms two-note chime synthesized in code (`engine/chime`), rendered
once at the output rate. It goes through the normal output pipeline, so AEC
removes it from the capture that follows, and the reply path drains stale
output events before playing. Barge-in and follow-up turns get no chime.
`[speech] wake_chime` (default on) or `BORIS_WAKE_CHIME=0` turns it off.

## Self-check

`EngineHandle::run_self_check` (or `EngineCommand::RunSelfCheck`) proves the
voice chain on this PC without anyone speaking. Steps, published as
`StatusPicture.self_check` after each one finishes:

| Step id | Checks |
|---------|--------|
| `microphone` | 1.5 s live listen delivers audio above digital silence |
| `speaker` | Playback stream is open and has not reported loss |
| `voice` | TTS synthesizes "Hey Boris, what time is it?" (in memory, never played) |
| `wake` | Wake model fires on that audio, scored like `wait_for_wake` |
| `speech_detect` | Silero finds at least 320 ms of speech in it |
| `transcribe` | STT returns the phrase's words ("Boris" itself may be misheard) |

A failed model step can be fixed with `repair_models(on_progress)`: it hashes
every installed model against its pinned SHA-256 (about 1 GB read), deletes
the ones that fail, then runs `install_models` to fetch them again. Stop the
engine first; loaded models can hold their files open on Windows.

It runs on the engine thread with the engine's own model instances, only while
idle or Armed. Mid-turn (hearing, thinking, talking, confirming, typed input)
the request is dropped and the turn continues; the desktop UI times out and
asks the user to retry. Models are released afterwards per residency policy.

## API key check

`check_api_key(key).await` asks OpenRouter's `GET /key` whether the key works
and returns an `ApiKeyStatus { state, detail }`. `ApiKeyState` serializes as
`ok` | `no_credit` | `rejected` | `unreachable` | `missing`. Rate limits (429),
timeouts (408), and 5xx are `unreachable`, never blamed on the key. A blank key
is `missing` without a network call.

## Public surface

| Type | Role |
|------|------|
| [`Engine`](src/engine/mod.rs) | Owns the engine thread join handle |
| [`EngineHandle`](src/engine/mod.rs) | Cloneable command sender (`Start` / `Stop` / `Shutdown` / device switch / wake enroll / self-check / typed input) |
| [`PipelineConfig`](src/config.rs) / [`LlmPrefs`](src/config.rs) | Host spawn configuration |
| [`StatusPicture`](src/status.rs) | UI DTO (mirrors desktop TS types; `thinking` is the live reasoning tail; `seq` is the latest-wins order key; `listen_hint` / `self_check` explain idle problems) |
| [`check_api_key`](src/api_key.rs) / `ApiKeyStatus` | OpenRouter key verdict for setup screens |
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
  config.toml          # prefs: [models], [capability], [audio], [speech], [agent], [ui]; optional [logging], [workspace]
  auth.json            # secrets: openrouter_api_key, exa_api_key
  BORIS.md             # optional user instructions, injected into the system prompt
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
  state/projects.json  # daily cache of discovered git repositories
```

`save_settings` unconditionally rewrites `[models]`, `[capability]`, `[audio]`, `[speech]`,
`[agent]`, and `[ui]`, and conditionally writes `[logging]` (only when a non-empty logging
filter is set, so fresh installs don't get a stray empty section). All other root tables and
unknown keys outside those managed sections are preserved on save (see `save_settings` /
`apply_config_file` in `src/settings.rs`). There is currently no genuinely hand-edit-only
managed table in `config.toml` — anything the desktop settings UI can persist ends up in one
of the sections above. The optional, hand-edited `[workspace]` table is not managed, so it is
preserved:

```toml
[workspace]
projects = ['C:\Users\me\code\app']  # known project roots, added as read roots
discover_projects = true                # bounded git-repo scan of read roots (BORIS_DISCOVER_PROJECTS=0 disables)
```

Optional, hand-edited `[[hooks]]` entries (also preserved on save) run user
commands around tool calls and at turn end; they are off unless present and
**run arbitrary commands** (see the agent README, "User hooks"):

```toml
[[hooks]]
event = "pre_tool"                 # pre_tool | post_tool | turn_end
tool = "bash"                      # optional tool-name glob
command = "python C:/hooks/check.py"
timeout_secs = 10                  # optional, 1-120
```

Hook commands start in `state/workspace/`.

Known projects are rendered into the system prompt with the sandbox, the real `bash` shell,
and the readable/writable roots. Configured projects become read roots only; write roots are
never widened.

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
| `BORIS_WAKE_CHIME` | `0` disables the chime played when a wake is accepted |
| `BORIS_STREAM_SPEECH` | `0` speaks only after the final answer is accepted (no sentence streaming) |
| `BORIS_PROGRESS_LINE` | `0` disables the one spoken progress line on long tool turns |
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
| `engine/device_watch` | Unplug / re-plug recovery (helper thread + pure policy) |
| `engine/self_check` | On-device voice-chain self-check |
| `engine/chime` | Synthesized wake chime |
| `api_key` | OpenRouter key check DTO |
| `diagnostics` | Startup environment dump |
| `artifacts` | Session artifact store |
| `liveness` | Taught-wake liveness enroll + scoring |
| `env_util` | Env-var parsing helpers |
| `error` | `PipelineError` |
| `prompt` | System prompt text |
