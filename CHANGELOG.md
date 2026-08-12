# Changelog

All notable changes to Boris Assistant are documented in this file. This
project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- `web_search` no longer needs an Exa account. Downloads search via DuckDuckGo
  (Instant Answer + HTML) and Wikipedia with no API key. A configured Exa key
  is still used first as an optional upgrade.

### Added

- Session-backed visual artifacts: `present_artifact` / `list_artifacts` /
  `get_artifact` store markdown and code cards under
  `{session}/artifacts/{slug}-{id}.{ext}` with an `index.json` catalog.
  Spoken replies stay a short pointer; card bodies are not sent to TTS.
- Overlay glance + main-window session desk for those cards (no extra window).
  The island expands to a clipped preview; Home lists and renders the full
  session catalog.

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

[1.0.0]: https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.0.0
