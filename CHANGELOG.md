# Changelog

All notable changes to Boris Assistant are documented in this file. This
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

No changes yet.

## [1.2.0-beta.3] - 2026-09-27

Third 1.2 beta. Stable **1.1.x** stays on `main`. Windows x64, NSIS only.

### Added

- A `bun run verify:release` check in `desktop/` validates product versions,
  installer targets, the updater public key, and release documentation before
  signing. `bun run release:manifest <UTC-timestamp>` creates the updater
  manifest from the built installer and its signature.
- Product website download and release pages, setup FAQs, canonical URLs,
  structured data, crawl files, and working social preview images. Published
  release listings remain pinned until the new beta is uploaded.
- A desktop architecture guide at `desktop/docs/ARCHITECTURE.md` maps the UI,
  IPC bridge, Tauri host, and paths for tracing common behavior.
- **Bundled agent playbooks** for code changes, root-cause debugging, code
  explanation, design, change review, technical writing, mentoring, and skill
  creation. The playbooks are installed under `~/.boris/skills` and are
  available through progressive skill discovery.
- Coverage that installs and loads every bundled playbook, including its
  frontmatter and full-body envelope.
- A phase-aware **presence orb** (`thinking-orbs`) on Home, the overlay
  island, startup, and Teach Voice. Fifteen states (off / starting / ready /
  hearing / reading / thinking / searching / working / weaving / shaping /
  talking / awaiting-reply / awaiting-input / confirm / fault) are derived
  from engine phase and activity text — never audio amplitude. It pauses
  off-screen, debounces thinking-state churn, falls back to a static ring
  when Canvas is unavailable, and stays static under reduced-motion.
- Shared overlay motion plus revision-keyed **streaming overlay text**:
  streamed words append without replaying, with instant swaps under
  reduced-motion.
- Cancellable LLM summary compaction that records provider usage and merges
  it into turn accounting (`TokenAccounting::merge`).
- A monotonic `seq` counter on every `StatusPicture` snapshot so the UI can
  order and dedupe latest-wins across the engine and per-turn publishers.
- Live capture feedback re-decodes the growing audio prefix when warm STT
  supports partials. The overlay shows advisory text during freeform capture;
  the final transcription still drives the turn. Stable partials can shorten
  the silence wait. Confirm captures stay short and skip partial decoding.
- The overlay shows the current tool operation and live progress notes during
  a turn.
- `collect_input` now supports numbered choices with up to four spoken options,
  alongside exact values, secrets, and large pastes.
- Truncated tool results can be reread without rerunning the tool. Session
  binding stores the pre-context-truncation result, up to 256K characters,
  under `tool_outputs/`; `get_tool_output` reads it by path and offset. The
  store removes old files on a best-effort basis after seven days.
- Optional host integration for stdio MCP servers. Read-suggestive tools
  register by default; writes require host opt-in, and every remote call still
  requires confirmation.

### Changed

- Desktop command handlers, bridge calls, status presentation helpers, and
  settings and update controllers now live in focused modules. The desktop
  README links to the architecture guide, and `@/lib/statusPresentation`
  remains the stable import path for status helpers.
- Voice HITL confirm budget now mirrors the agent policy
  (`max_confirms_per_turn`, default 12) instead of a hardcoded 8 rounds.
  Post-confirm tool rounds feed the overlay context meter, the per-turn trace
  (`agent_resume`), and the artifact peek instead of staying invisible.
- Confirm prompts and re-asks are wake barge-in aware: a wake word mid-prompt
  stops playback and listens for the yes/no instead of auto-denying. The
  second-capture reject path resumes through the same barge-aware thinking
  path as every other agent call (Stop / new-request barge-in applies).
- Mid-confirm device switches re-speak the prompt on the new device instead
  of capturing on a stale mic; wake-enroll requests yield the turn; audio
  disconnects mark devices dead and a fresh Start re-marks them alive.
- Overlay snapshot contract: latest-wins by `seq`, activity truncated to 160
  chars and the reasoning tail to 512, the thinking tail is kept across
  Hearing/Reading so the overlay does not flicker, a fresh turn reports zero
  context (not 1), transient hints use `activity` instead of error `detail`,
  `go_off` clears stale captions, a TTS-load failure clears unsaid text, and
  a missed `present_artifact` peek only logs instead of blanking the card.
- Confirm captures follow model residency (`LowMemory` evicts STT, otherwise
  models stay warm across re-prompt rounds); a stuck TTS inference is parked
  on a reaper thread instead of leaking model ownership.
- System prompt gains an `<interaction>` block (host owns the turn, trailing
  `?` opens the mic for one freeform reply, HITL prompts stay one short
  sentence, `collect_input` is pointer-only and never reads secrets back,
  cards go through `present_artifact` first), treats `<personal_context>` as
  ground-truth data rather than instructions, and allows exactly one verified
  profile URL on the spoken line.
- OpenRouter clients for voice turns enable reasoning `include_text` so
  `Reasoning` events reach the overlay thinking chip; intentionally unwired
  client knobs (max tokens, timeouts, referer/title, base URL) stay on
  defaults until the product needs them.
- Engine setup applies the capability preset to the sandbox at the type level
  before tool registration, pins `ToolRuntimeFeatures` listing knobs
  (`force_list_all: false`, `core_tools: None`), and logs legacy-memory
  discovery before queueing migration. Session handoff flushes durably with a
  timeout so two sessions' snapshots cannot interleave.
- Internal cleanup in `boris-agent` / `boris-ai`: dead compaction preview and
  search-limit helpers removed, `MemoryStatus::parse` deduped,
  `tool_observation_json` is test-only, plus `cargo fmt` normalization.

- Existing task-execution and research playbooks now follow the user's intent
  and scope. Todos, extra research, artifacts, and fixed search quotas are
  optional and driven by the work required.
- Skill catalogs, load results, and post-tool reminders now reinforce matching
  a skill by its full description and stopping when the requested result is
  complete.
- **Prompt hardening**: trusted system content is now separate from untrusted
  user-role data. Personal context, the skills catalog, memory, and task
  evidence are sent as user-role data; the skills catalog is a bounded JSON
  envelope (`<skills_catalog_data>`) paired with a static
  `SKILLS_SYSTEM_POLICY`, and user info is length-bounded with markup
  escaping. Turn abort/reset paths refresh the system prompt and resolve
  pending tool calls as cancelled.
- **Context accounting and compaction**: token estimates run over serialized
  message JSON; summary compaction is lossless (only complete oldest turns
  are folded into a single `Summary` message, the unsummarized tail is never
  deleted), with hysteresis before triggering LLM compaction and smaller
  Tier-2/Tier-3 fallbacks.
- Oversized requests now fail fast with `InputTooLarge` before any provider
  call (no billing); summary-maintenance turns are forced onto the fast
  tier with an explicit cap.
- Silero freeform endpointing is now 700 ms of trailing silence (was
  550 ms); yes/no confirm stays at 250 ms.
- Tools that collect typed input now force the sequential HITL-safe path so
  remaining calls are preserved across the `NeedsInput` pause.
- Confirmation and input prompts use short, speakable tool descriptions. File
  tools suggest close sibling names when a spoken path does not match. Image
  and PDF reads return a bounded file description for on-screen opening; they
  do not extract pixels or PDF text. `web_fetch` retries likely bot-blocked
  403 or 503 responses once.
- Freeform capture length is configurable from 15 to 180 seconds with
  `BORIS_MAX_UTTERANCE_SECS` (default 30); confirmation capture remains capped
  at 8 seconds. Live partial decoding can be disabled with
  `BORIS_STT_PARTIALS=0` and tuned with the other `BORIS_STT_PARTIAL_*` values.
- The OpenRouter client normalizes assistant content and uses a blocking
  completion fallback when a stream has no usable text or tool call. The agent
  loop also reminds the model of available tools after repeated unknown-tool
  rounds.
- Overlay window parking uses one stable top edge so the island grows
  downward instead of jumping; startup splash, Home, Teach Voice, Settings,
  and overlay transitions are faster and share one motion helper.
  Settings toggles use a compact switch style.

### Fixed

- OpenRouter SSE assembly no longer glues duplicate text when a stream sends
  both incremental `delta` and canonical `message` payloads; tool calls are
  rebuilt by position and snapshots only cover content with no prior delta.
- Overlay captions, thought tails, and artifact cards share one motion path
  with `inert`/`aria-hidden` hiding and reduced-motion instant paths.
- Bare "please" no longer counts as a yes in voice confirmation (polite
  filler re-asks, then denies by default); "yes please" and "please go ahead"
  still approve.
- Barge-in `TakeTurn` now carries the interrupted turn's `heard` text so the
  next turn quotes the real request.
- Confirm playback interrupts and missed-utterance hints no longer surface as
  error `detail`; confirm TTS synth failures eagerly unload the model, and
  confirm capture/STT/settle failures are logged with deny-by-default
  instead of silent empty strings.
- Engine and agent events now update one shared status snapshot before the
  host assigns the public sequence number. The host drops snapshots from old
  engine generations, so late tool activity cannot overwrite newer phase,
  device, input, artifact, or fault state. Overlay text and card transitions
  also keep their layout through status changes.

### Removed

- The `cargo xtask trace-report` helper and the `xtask` workspace member are
  gone (including the `cargo xtask` alias). Per-turn traces are still
  appended to `~/.boris/traces/turns.jsonl` — read the JSONL directly.
- The 3×3 presence-grid components (`GridPresence`, presence state/play
  helpers, and their CSS/tests) are replaced by the orb (`BorisOrb`,
  `orbStateFromStatus`, `overlayMotion`).

## [1.2.0-beta.2] - 2026-08-28

Second 1.2 beta. Stable **1.1.x** stays on `main`. NSIS only.

### Added

- **Canonical Boris memory**: `~/.boris/memory/memory.sqlite` is now the
  local source of truth for conversation evidence, durable facts,
  preferences, projects, lifecycle state, and full-text retrieval.
- `memory_search`, `memory_get`, and `forget_memory` now operate on
  evidence-backed records. Completed turns are durably ingested without
  writing a Markdown session-memory log.
- A verified legacy-memory migration imports `profile.json`, global and
  workspace `MEMORY.md`, and old session `memory.md` files. Each archived
  excerpt is refined by Boris's configured LLM before it becomes a canonical
  record.

### Changed

- **Boris memory** replaces the old Markdown memory runtime in Desktop. The
  Settings toggle keeps its existing saved config key for compatibility, but
  now controls the canonical store.
- Personal-context extraction state is stored in `memory.sqlite`; it no
  longer creates or updates `profile.json` after migration.
- Personal context injected into the model prompt now comes from canonical
  active records, so corrections, expiry, and explicit forgetting affect both
  retrieval and prompt context.

### Migration and privacy

- Legacy files are deleted only after every discovered source has been staged,
  successfully AI-refined, and verified in the canonical store. If the LLM
  call fails, times out, or the import cannot be verified, the old files stay
  untouched and Boris retries on a later launch.
- A successful migration retires the obsolete `profile.json`, `MEMORY.md`,
  session `memory.md`, and derived `search.sqlite` files. Historical content
  is sent only to the LLM already configured for Boris during this one-time
  refinement pass.

## [1.2.0-beta.1] - 2026-08-25

First 1.2 beta. Stable **1.1.x** stays on `main`. NSIS only.

### Added

- **Taught wake filtering**: Settings → Speech → Teach saves four “Boris”
  takes at `~/.boris/speaker/live.json`. With the optional CAM++ model,
  embeddings reject unmatched TV, Translate, and TTS wakes; re-teach profiles
  created before CAM++ support.
  The filter is on by default and can be disabled with `BORIS_WAKE_LIVENESS=0`.
- A 16 kHz **audio front end** adds high-pass filtering, AGC2, and AEC3 before
  VAD, wake, and STT. TTS is the echo reference; `BORIS_AUDIO_FRONTEND=0`
  bypasses the processing.
- Wake-word **barge-in** now works while Boris is speaking or thinking. A new
  request or “stop” interrupts the turn; a false hit or “continue” resumes
  speech. It is on by default and can be disabled with `BORIS_BARGE_IN=0`.
- On-screen **typed input** for exact values, secrets, and large pastes.
  `collect_input` pauses the turn and works in Home and the overlay; secrets
  stay masked, silent, and out of session transcripts.
- A live **reasoning preview** appears in Home and the overlay for planning
  and complex work. It is display-only: never spoken or stored as context.
- A phase-aware 3×3 **presence grid** now shows engine state on Home and the
  overlay: framed chases signal boot/work, audio meters cover listening and
  speech, a scan indicates transcription, and an ellipsis prompts for input.
  Ready, Confirm, Fault, and Off remain distinct; the launch splash keeps its
  own dedicated signal mark.
- `grep` supports `-A` / `-B` / `-C` / `-i`, `type`, `glob`, `output_mode`,
  and `head_limit`.

### Changed

- Overlay activity uses concise verb-object labels, such as `Reading src/lib.rs`,
  and combines consecutive calls.
- Simple `bash` reads and searches are routed to native file, directory, grep,
  or speech tools. Real pipelines still use bash.
- File and code requests are prompted to inspect local evidence before replying.
- Typed input submits with Enter or Ctrl+Enter; Esc cancels.

### Fixed

- Failed CAM++ embeddings now reject a taught wake instead of treating it as
  live; teaching also requires valid embeddings.
- AEC preserves the already-playing TTS reference, keeping echo cancellation
  aligned with audible audio.
- Maintenance LLM timeouts no longer panic their Tokio worker.
- Older `get_status` responses cannot overwrite newer pushed status.
- Overlay input now submits correctly and remains correctly sized, positioned,
  and labeled during Thinking and exit transitions.
- ONNX Runtime tests no longer deadlock during concurrent initialization.

## [1.1.0] - 2026-08-15

Stable release of the 1.1 line. Product-equivalent to [1.1.0-beta.5];
Windows ships both NSIS and MSI again. See the beta.1–beta.5 notes below
for the day-by-day 1.1 history.

### Added

- Settings → General: **Start with Windows**. Registers a current-user Run
  key so Boris launches at sign-in. That launch stays in the tray (no main
  window) and turns the engine on.
- Asynchronous research subagents return immediately and support real
  `poll`, `join`, and `cancel` actions with session-bound cancellation.
- Durable per-turn latency traces under `~/.boris/traces/turns.jsonl`, plus
  `cargo xtask trace-report` for local p50/p95 reports.
- SQLite FTS5-backed memory search that is rebuilt only when its schema needs
  migration and stays current as session memory is appended.
- Session-backed visual artifacts: `present_artifact` / `list_artifacts` /
  `get_artifact` store markdown and code cards under
  `{session}/artifacts/{slug}-{id}.{ext}` with an `index.json` catalog.
  Spoken replies stay a short pointer; card bodies are not sent to TTS.
- Overlay glance + main-window session desk for those cards. The island
  expands to a clipped preview; Home lists and renders the full session
  catalog.
- Settings → General update channel (Stable / Beta). Beta polls versioned
  `v*-beta.N` pre-releases; Stable uses GitHub Latest.
- Branded launch splash from first HTML paint through React mount so the
  main window does not flash an empty shell.

### Changed

- Hearing now uses **Silero VAD** (official streaming ONNX) instead of the
  deprecated WebRTC/libfvad GMM. Every 32 ms hop is scored so LSTM state
  stays aligned. Threshold override: `BORIS_VAD_THRESHOLD`.
- Silero endpointing: freeform trailing silence is **550 ms** (LiveKit Agents
  Silero default) and yes/no confirm silence is **250 ms**. The old 900 / 420
  ms windows were hangover for WebRTC flicker.
- Model routing is request-local and trait-driven. Greetings and time/date
  turns stay on the fast tier even when tools are advertised; research,
  coding, side effects, and failed tool evidence receive stronger budgets.
- Progressive tool listing is enabled by default with bounded LRU activation,
  a hard 64 KiB schema ceiling, central argument validation, per-tool
  concurrency limits, and parallel read-only calls before the first approval.
- Session transcripts, long-term-memory appends, personal extraction, and turn
  traces run on maintenance lanes instead of the speech-critical engine path.
- Final speech is synthesized in sentence units while earlier audio plays.
  Stop/device-switch commands remain responsive, and `low_memory`, `balanced`,
  and `low_latency` now provide distinct model-residency policies.
- The desktop loads only its selected window and dynamically imports syntax
  grammars. The production entry chunk is now about 226 KiB and has a 500 KiB
  release gate.
- Default chat model is `deepseek/deepseek-v4-flash-0731` via DigitalOcean
  (`digitalocean`). Existing saved model/provider choices are unchanged.
- Desktop process and log files are named `boris` (not `boris-desktop`).
  The taskbar, installer binary, and `~/.boris/logs` all use `boris`.
- Completion logs now include wall-clock `ms` for the thing that just finished:
  each tool call, LLM complete (stream / blocking), memory search / get /
  personal extract, status phase transitions, wake wait, capture, session
  begin/end, and engine init steps. STT / TTS / agent-turn timings were
  already present.
- `web_search` no longer needs an Exa account. Downloads search via DuckDuckGo
  (Instant Answer + HTML) and Wikipedia with no API key. A configured Exa key
  is still used first as an optional upgrade.
- `grep` stays in-process for keyword / single-file searches. Ripgrep is only
  spawned when the pattern looks like a regex.
- `bash` uses `bash -c` instead of a login shell (`-lc`), prefers Git Bash by
  absolute path, and never launches WSL/`WindowsApps` `bash.exe`.

### Fixed

- First overlay show no longer flashes a solid rectangle. The island stays
  off-screen until React paints, the shared launch splash is stripped from
  the overlay document, and the island CSS has a radius before Motion runs.
- Malformed, missing, or schema-invalid tool arguments are returned to the
  model as repairable observations and can never reach tool execution.
- Transcript compaction now rewrites changed prefixes even when the resulting
  snapshot grows, preventing mixed pre/post-compaction JSONL history.
- Playback lifecycle events cannot deadlock the audio worker, `Started` means
  the first real device sample, and a stuck native TTS call is detached after
  a bounded cancellation grace instead of freezing shutdown.
- Research finish gating counts useful current-turn evidence rather than stale
  or failed search observations.
- Saving Settings while the overlay is already up no longer flashes a large
  decorated transparent window. Packaged Windows builds no longer pop that
  frame on launch (or on the automatic Start-on-open save).
- The empty window that flashed on every Settings save in the packaged app
  was a console for `icacls` (ACL lock on `auth.json`). `CREATE_NO_WINDOW`
  is set so the GUI exe does not allocate a terminal.
- Settings → Updates no longer appends a second `beta` onto versions that
  already include a pre-release label.
- Update checks peek the GitHub Releases API (no asset download) and only
  hit `/releases/download` when a newer version is listed. On Windows the
  HTTP client skips WinHTTP WPAD so a URL that is instant in the browser
  is not a 10–20s Rust hang.
- Settings save and engine Start no longer freeze the Windows UI thread.

### Packaging

- Windows ships **NSIS and MSI**. WiX can encode the numeric `1.1.0`
  version; 1.1 betas remain NSIS-only.

## [1.1.0-beta.5] - 2026-08-15

### Added

- Settings → General: **Start with Windows**. Registers a current-user Run
  key so Boris launches at sign-in. That launch stays in the tray (no main
  window) and turns the engine on.
- Asynchronous research subagents return immediately and support real
  `poll`, `join`, and `cancel` actions with session-bound cancellation.
- Durable per-turn latency traces under `~/.boris/traces/turns.jsonl`, plus
  `cargo xtask trace-report` for local p50/p95 reports.
- SQLite FTS5-backed memory search that is rebuilt only when its schema needs
  migration and stays current as session memory is appended.

### Changed

- Hearing now uses **Silero VAD** (official streaming ONNX) instead of the
  deprecated WebRTC/libfvad GMM. Every 32 ms hop is scored so LSTM state
  stays aligned. Threshold override: `BORIS_VAD_THRESHOLD`.
- Silero endpointing: freeform trailing silence is **550 ms** (LiveKit Agents
  Silero default) and yes/no confirm silence is **250 ms**. The old 900 / 420
  ms windows were hangover for WebRTC flicker.
- Model routing is request-local and trait-driven. Greetings and time/date
  turns stay on the fast tier even when tools are advertised; research,
  coding, side effects, and failed tool evidence receive stronger budgets.
- Progressive tool listing is enabled by default with bounded LRU activation,
  a hard 64 KiB schema ceiling, central argument validation, per-tool
  concurrency limits, and parallel read-only calls before the first approval.
- Session transcripts, long-term-memory appends, personal extraction, and turn
  traces run on maintenance lanes instead of the speech-critical engine path.
- Final speech is synthesized in sentence units while earlier audio plays.
  Stop/device-switch commands remain responsive, and `low_memory`, `balanced`,
  and `low_latency` now provide distinct model-residency policies.
- The desktop loads only its selected window and dynamically imports syntax
  grammars. The production entry chunk is now about 226 KiB and has a 500 KiB
  release gate.
- Default chat model is `deepseek/deepseek-v4-flash-0731` via DigitalOcean
  (`digitalocean`). Existing saved model/provider choices are unchanged.
- Desktop process and log files are named `boris` (not `boris-desktop`).
  The taskbar, installer binary, and `~/.boris/logs` all use `boris`.
- Completion logs now include wall-clock `ms` for the thing that just finished:
  each tool call, LLM complete (stream / blocking), memory search / get /
  personal extract, status phase transitions, wake wait, capture, session
  begin/end, and engine init steps. STT / TTS / agent-turn timings were
  already present.

### Fixed

- First overlay show no longer flashes a solid rectangle. The island stays
  off-screen until React paints, the shared launch splash is stripped from
  the overlay document, and the island CSS has a radius before Motion runs.
- Malformed, missing, or schema-invalid tool arguments are returned to the
  model as repairable observations and can never reach tool execution.
- Transcript compaction now rewrites changed prefixes even when the resulting
  snapshot grows, preventing mixed pre/post-compaction JSONL history.
- Playback lifecycle events cannot deadlock the audio worker, `Started` means
  the first real device sample, and a stuck native TTS call is detached after
  a bounded cancellation grace instead of freezing shutdown.
- Research finish gating counts useful current-turn evidence rather than stale
  or failed search observations.

## [1.1.0-beta.4] - 2026-08-13

### Fixed

- Saving Settings while the overlay is already up (Ready / mid-turn) no
  longer flashes a large decorated transparent window. Unrelated toggles
  only update the prefs cache; `set_size` / `set_max_size` / `set_position`
  run when overlay scale or position actually change.
- Packaged Windows builds no longer pop that decorated transparent frame
  on launch (or on the automatic Start-on-open save). The overlay HWND is
  created only when the island must appear; `hide()` is skipped until it
  has actually been shown.
- The empty window that flashed on every Settings save in the packaged
  app was a console for `icacls` (ACL lock on `auth.json`), not the
  overlay. `CREATE_NO_WINDOW` is set so the GUI exe does not allocate a
  terminal. Same as tauri-apps/discussions#11446.

## [1.1.0-beta.3] - 2026-08-13

### Fixed

- Settings → Updates no longer appends a second `beta` onto versions that
  already include a pre-release label (`v1.1.0-beta.2 · beta`).
- Update checks peek the GitHub Releases API (no asset download) and only
  hit `/releases/download` when a newer version is listed. On Windows the
  HTTP client skips WinHTTP WPAD so a URL that is instant in the browser
  is not a 10–20s Rust hang. CDN send failures show as a short
  "could not reach GitHub" message.
- Saving settings no longer flashes a blank always-on-top window. Overlay
  resize/move is skipped while the island is hidden.

## [1.1.0-beta.2] - 2026-08-13

### Added

- Branded launch splash from first HTML paint through React mount so the
  main window does not flash an empty shell.

### Fixed

- Settings save and engine Start no longer freeze the Windows UI thread.
  `get_settings` / `save_app_settings` (and device switch) run off the
  message pump; boot `load_settings` is deferred until after first paint.

## [1.1.0-beta.1] - 2026-08-13

### Changed

- `web_search` no longer needs an Exa account. Downloads search via DuckDuckGo
  (Instant Answer + HTML) and Wikipedia with no API key. A configured Exa key
  is still used first as an optional upgrade.
- `grep` stays in-process for keyword / single-file searches. Ripgrep is only
  spawned when the pattern looks like a regex (a lone `.` in `file.rs` is not
  treated as regex). Avoids a ~30–60ms process tax on every voice-turn search.
- `bash` uses `bash -c` instead of a login shell (`-lc`), prefers Git Bash by
  absolute path, and never launches WSL/`WindowsApps` `bash.exe`. Login-profile
  sourcing was ~10× the spawn cost of `echo`.

### Added

- Session-backed visual artifacts: `present_artifact` / `list_artifacts` /
  `get_artifact` store markdown and code cards under
  `{session}/artifacts/{slug}-{id}.{ext}` with an `index.json` catalog.
  Spoken replies stay a short pointer; card bodies are not sent to TTS.
- Overlay glance + main-window session desk for those cards (no extra window).
  The island expands to a clipped preview; Home lists and renders the full
  session catalog.
- Settings → General update channel (Stable / Beta). Beta polls a long-lived
  GitHub pre-release tagged `beta`; Stable still uses `/releases/latest`.

### Packaging

- Windows beta bundles NSIS only. WiX/MSI rejects non-numeric pre-release
  labels such as `1.1.0-beta.1`.

## [1.0.0] - 2026-08-12

### Added

- Boris Desktop for Windows: a Tauri/React desktop host for the local voice
  pipeline.
- Local voice components for wake-word detection, Parakeet speech-to-text, and
  Supertone text-to-speech.
- An agent runtime with capability presets, path policy, confirmations for
  higher-risk actions, memory, sessions, and browser/file/shell tools.
- Runtime diagnostics and model-install workflows under `%USERPROFILE%\\.boris`.

### Security

- Documented the product's capability policy, path sandbox boundaries, and
  private vulnerability-reporting process in [SECURITY.md](SECURITY.md).

### Packaging

- Windows MSI and NSIS installer targets for the Boris Desktop host.

[1.2.0-beta.3]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.2.0-beta.3
[1.2.0-beta.2]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.2.0-beta.2
[1.2.0-beta.1]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.2.0-beta.1
[1.1.0]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0
[1.1.0-beta.5]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.5
[1.1.0-beta.4]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.4
[1.1.0-beta.3]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.3
[1.1.0-beta.2]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.2
[1.1.0-beta.1]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.1
[1.0.0]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.0.0
