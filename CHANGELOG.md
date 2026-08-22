# Changelog

All notable changes to Boris Assistant are documented in this file. This
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Further **1.2** work on `next` after [1.2.0-beta.1].

### Added

- Wake identity uses WeSpeaker **CAM++** embeddings (cosine vs enrolled takes)
  when `~/.boris/models/speaker/campplus_lm.onnx` is present. Brightness
  mismatch is no longer the identity test. Spectral playback scoring still
  rejects TV / Translate. Fbank matches WeSpeaker Hamming + per-frame DC
  removal; re-teach if you enrolled on an earlier build of this work.
- Capture **front-end** on the 16 kHz tap: high-pass, AGC2, and AEC3 (sonora /
  WebRTC APM). TTS is the echo reference, so leftover playback is subtracted
  before VAD / wake / STT. Quiet talk at laptop distance is gained up. Neural
  noise suppression is off (it hurts Parakeet). `BORIS_AUDIO_FRONTEND=0`
  bypasses the APM.
- Wake-word **barge-in** while Boris is talking. A lower wake threshold plus
  close-talk energy (not Armed’s playback gate — leftover TTS in the mic
  window looks like a speaker) pauses leftover speech. Silence, “continue”,
  or a false hit resumes from the cut; “stop” / a new request discards leftover
  and either re-arms or starts a turn. Settings → Speech; `BORIS_BARGE_IN=0`
  disables. Default on.
- On-screen **typed input** when voice is a bad channel. `collect_input`
  (exact / secret / blob) pauses the turn. The overlay unlocks a field (or
  textarea for a paste). Home shows the same field. Secrets are masked, never
  spoken, and redacted from session transcripts. Same submit from either
  window.
- Wake-word **barge-in** while Boris is working (Thinking, including tools
  after you confirm a dangerous call). The agent turn runs off the engine
  thread so the mic can still score wake. Armed live-mic liveness applies.
  Work keeps running until STT decides. Silence or a rejected speaker is a
  no-op; a bare “stop” / “wait” cancels; a new instruction (“no, do xyz
  instead”) replaces the turn. Same `voice_barge_in` toggle.
- Live **reasoning preview** on Home and the overlay while the strong / tool
  route thinks. Planning and complex stages now ask OpenRouter for reasoning
  text and stream the tail into `StatusPicture.thinking`. It is display-only
  — never spoken and never stored in agent context. The island grows to a
  thought stage (380 × 216) so four lines stay readable without scrollbars.
- **Presence grid** on splash, Home, and the overlay (3×3 cells). Engine
  Starting, Fault, and Off have their own copy so a slow boot is not labeled
  Off. Home and Settings are split out of the old MainWindow blob.
- `grep` takes Grok/Claude-style flags: `-A` / `-B` / `-C` / `-i`, `type`,
  `glob`, `output_mode` (`content` / `files_with_matches` / `count`), and
  `head_limit`.

### Changed

- Overlay activity is verb + object (`Reading src/lib.rs`) instead of
  `tool · bash`. Consecutive same-kind calls collapse (`Read 4 files`).
- Simple `bash cat` / `grep` / `ls` / `echo` is steered into `file_read`,
  `grep`, `list_dir`, or speech. Real pipelines stay on bash.
- Local file and code asks finish-gate if the turn never grepped, listed,
  or read. Same reminder style as weak research.
- Typed input has no Send / Paste / Cancel buttons. Enter or Ctrl+Enter
  submits (Settings → Overlay). Esc cancels. Paste with Ctrl+V.

### Fixed

- Once a voiceprint exists, a missing or failed CAM++ embedding is a
  mismatch, not Live. Teach rejects embed errors until two takes exist.
- AEC far-end overflow drops newest TTS, not the start already in the air,
  so echo cancel stays aligned with what you hear.
- Maintenance LLM timeouts are built inside the Tokio `block_on` future.
  Building `tokio::time::timeout` on the maintenance thread panicked it.
- A slower `get_status` invoke no longer overwrites a later pushed snapshot.
- Native overlay size stays on the card budget until React finishes the
  island exit, so scale does not snap while the card is still leaving.
- Overlay typed input no longer sits under a Thinking header, repeats the
  field label, or grows off the top of the screen. Short exact/secret
  fields stay thought-sized. Blob paste still uses the glance card. The
  HWND parks on-screen for either.
- Overlay and Home can actually submit typed input. `submit_input` /
  `cancel_input` were registered but missing from the Tauri allowlist.

## [1.2.0-beta.1] - 2026-08-17

First 1.2 beta. Stable **1.1.x** stays on `main`. NSIS only.

### Added

- Wake **live-mic teach**: Settings → Speech → Teach walks through four
  “Boris” takes. The profile lives at `~/.boris/speaker/live.json`.
- After a profile exists, Armed wake hits that look like a speaker (TV,
  Translate, TTS) or that do not match the taught takes stay Armed and do
  not start a turn. A live person still wakes Boris.
- Settings → Speech: **Ignore speakers and TV** (`[speech]
  ignore_speaker_playback`, default on). The gate is a no-op until teach
  finishes. Override with `BORIS_WAKE_LIVENESS=0`.

### Changed

- `wait_for_wake` returns the 2 s window so the engine can crop speech and
  score liveness on the same samples.
- Empty wake crops no longer fall back to the raw buffer (that enrolled
  room noise).

### Fixed

- ORT unit tests no longer deadlock: process-global `ort::init` finishes on
  the test thread before concurrent `init_onnx_runtime` calls.

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

[1.1.0]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0
[1.1.0-beta.5]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.5
[1.1.0-beta.4]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.4
[1.1.0-beta.3]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.3
[1.1.0-beta.2]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.2
[1.1.0-beta.1]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0-beta.1
[1.0.0]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.0.0
