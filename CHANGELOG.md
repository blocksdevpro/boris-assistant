# Changelog

All notable changes to Boris Assistant are documented in this file. This
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
