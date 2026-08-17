<p align="center">
  <img src="desktop/src-tauri/icons/128x128.png" width="88" alt="Boris">
</p>

<h1 align="center">Boris</h1>

<p align="center">
  A Windows-first desktop voice assistant that can actually help.<br>
  <strong>Wake → listen → transcribe → tools → speak.</strong>
</p>

<p align="center">
  <a href="https://github.com/blocksdevpro/boris-assistant/releases/latest"><img src="https://img.shields.io/github/v/release/blocksdevpro/boris-assistant?label=stable&color=3fb950" alt="Stable release"></a>
  <a href="https://github.com/blocksdevpro/boris-assistant/releases"><img src="https://img.shields.io/github/v/release/blocksdevpro/boris-assistant?include_prereleases&label=latest&color=d29922" alt="Latest including pre-releases"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache-2.0"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-0078D4?logo=windows&logoColor=white" alt="Windows 10 and 11">
  <img src="https://img.shields.io/badge/rust-1.97-dea584?logo=rust&logoColor=white" alt="Rust 1.97">
</p>

<p align="center">
  <a href="https://github.com/blocksdevpro/boris-assistant/releases/latest"><strong>Download for Windows</strong></a>
  ·
  <a href="CHANGELOG.md">Changelog</a>
  ·
  <a href="desktop/README.md">Desktop notes</a>
  ·
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

<p align="center">
  <img src="website/public/boris-screenshot.png" alt="Boris desktop: a live voice turn with the overlay island and current conversation" width="920">
</p>

Local wake, speech-to-text, and speech output. Your choice of cloud model. Tools that ask before they do something risky.

The product is **Boris Desktop** (`desktop/` → `boris-desktop`). Voice and agent logic live in the Rust workspace under `crates/`.

| Channel | Version | Get it |
|---|---|---|
| **Stable** | [1.1.0](https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0) | [Latest release](https://github.com/blocksdevpro/boris-assistant/releases/latest) — NSIS or MSI |
| **Beta** | **1.2.0-beta.1** (this tree) | Source / signed `bun run tauri build` from `next` |
| **This tree** | **1.2.0-beta.1** | `next` branch — NSIS only |

Workspace crates are `publish = false`. They ship inside the desktop app, not on crates.io.

---

## Install

Windows 10 or 11, x64, with a working mic and speakers.

1. Download **`Boris_*_x64-setup.exe`** (or the MSI) from [Releases](https://github.com/blocksdevpro/boris-assistant/releases).
2. Run the installer (you can install 1.1.0 over 1.0.0 or a 1.1 beta).
3. On first launch, finish **model install** and set an [OpenRouter](https://openrouter.ai/) API key in Settings.

Signed in-app updates poll GitHub Releases. **Stable** follows the latest non-prerelease. **Beta** follows versioned `v*-beta.N` pre-releases (the rolling [`beta`](https://github.com/blocksdevpro/boris-assistant/releases/tag/beta) tag still holds `latest.json` for the installer download). Pick the channel in **Settings → Updates → Channel**. The check reads the Releases API first so it stays fast; the asset CDN is only used when a newer build is listed.

Windows **1.1.0 ships NSIS and MSI**. Pre-release betas ship NSIS only because WiX/MSI cannot encode a label like `1.1.0-beta.1`.

Packaged builds have no console. Logs land at `%USERPROFILE%\.boris\logs\boris.YYYY-MM-DD.log`.

---

## Features

- **Hands-free loop** — wake word, VAD capture, local STT, agent turn, local TTS playback
- **Taught wake filter** — four “Boris” takes so TV / Translate / TTS from a speaker do not start a turn *(1.2 beta)*
- **Responsive speech** — Silero VAD, sentence-streamed TTS, and configurable model residency
- **Voice island** — always-on-top overlay for listening / thinking / speaking, plus live captions
- **Tool-using agent** — files, glob/grep, shell (HITL), web search and fetch, clipboard, memory, skills, sessions, todos
- **Async research** — background subagents with poll/join/cancel and read-only tool isolation
- **Capability presets** — `voice_safe` / `local_power` / `full` plus path policy and human approval for risky work
- **Local models** — LiveKit-style wake, Silero VAD, NVIDIA Parakeet STT, and Supertone TTS
- **Your model** — OpenRouter (OpenAI-compatible) via `boris-ai`; audio stays on the machine
- **Web search without a key** — DuckDuckGo + Wikipedia by default; an Exa key is an optional upgrade *(1.1)*
- **Session artifacts** — markdown and code cards on the overlay and Home desk; spoken replies stay short *(1.1)*
- **Local diagnostics** — durable turn traces with `cargo xtask trace-report` p50/p95 summaries
- **User home** — `%USERPROFILE%\.boris` for config, keys, models, logs, sessions, memory, skills, workspace, speaker teach

---

## How a turn works

```text
Armed  →  wake  →  Hearing  →  Reading  →  Thinking  →  Talking  →  AwaitingReply
   │                    │           │            │            │
   │                    VAD        Parakeet     agent +      Supertone
   │                                           tools         playback
   └──────── AwaitingConfirm (HITL yes / no) ─────────────────┘
```

The engine thread owns voice state and turn ordering. Reusable loader threads
preload STT/TTS, final speech is produced sentence-by-sentence while playback
continues, and durable transcript/memory/trace work runs on maintenance lanes.
This keeps Stop and device-switch commands responsive without turning the
pipeline into an unbounded worker mesh.

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│  desktop/  (React + Vite + Bun + Tauri v2)                  │
│    UI  ↔  IPC  ↔  boris-desktop (src-tauri)                 │
│    main window · overlay island · tray · updater            │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│  boris-pipeline  — ordered voice loop + bounded workers     │
│    Off → Quiet → Armed → Hearing → Reading → Thinking       │
│         → Talking → AwaitingReply / AwaitingConfirm         │
└───────┬───────────────┬───────────────┬─────────────────────┘
        │               │               │
        ▼               ▼               ▼
  boris-audio     boris-sense     boris-agent
  (cpal I/O)      (VAD + wake)    (ReAct + tools)
        │               │               │
        └───────┬───────┘               ▼
                │                 boris-ai (OpenRouter)
                ▼
        boris-inference (STT/TTS ports)
                │
     ┌──────────┴──────────┐
     ▼                     ▼
 boris-stt-parakeet   boris-tts-supertone
                      (boris-tts-kokoro experimental)
                │
                ▼
           boris-core  (shared types / errors)
```

| Crate | Role |
|-------|------|
| [`boris-core`](crates/boris-core) | Shared audio aliases, `TurnId`, foundation errors |
| [`boris-audio`](crates/boris-audio) | Capture / playback, resample to 16 kHz mono |
| [`boris-sense`](crates/boris-sense) | VAD + wake-word adapters, ORT init |
| [`boris-inference`](crates/boris-inference) | Object-safe STT / TTS traits |
| [`boris-stt-parakeet`](crates/boris-stt-parakeet) | Parakeet ONNX speech-to-text |
| [`boris-tts-supertone`](crates/boris-tts-supertone) | Product TTS (default) |
| [`boris-tts-kokoro`](crates/boris-tts-kokoro) | Experimental Kokoro adapter |
| [`boris-ai`](crates/boris-ai) | LLM client plane (OpenRouter) |
| [`boris-agent`](crates/boris-agent) | Tool runtime, policy, memory, sessions, artifacts |
| [`boris-pipeline`](crates/boris-pipeline) | Desktop voice engine + `~/.boris` + model install |
| [`boris-desktop`](desktop/src-tauri) | Tauri shell, tray, overlay, updater, packaging |

---

## Build from source

**Prerequisites**

- Windows (primary target — mic/speaker, ORT / DirectML packaging)
- [Rust](https://rustup.rs) — [`rust-toolchain.toml`](rust-toolchain.toml) pins **1.97.1** (MSRV 1.97)
- [Bun](https://bun.sh)
- [Tauri v2 platform deps](https://v2.tauri.app/start/prerequisites/)
- An [OpenRouter](https://openrouter.ai/) API key (or compatible setup)

```bash
git clone https://github.com/blocksdevpro/boris-assistant.git
cd boris-assistant

cp .env.example .env
# edit .env — OPENROUTER_API_KEY=...  (also settable in the app)

cd desktop
bun install
bun run tauri dev
```

Signed Windows release build (PowerShell):

```powershell
# Tauri reads TAURI_SIGNING_PRIVATE_KEY as a path or the key contents.
$env:TAURI_SIGNING_PRIVATE_KEY = (Resolve-Path .\.tauri\boris.key).Path
# Set this only when the key is password protected:
# $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "..."

Set-Location desktop
bun run tauri build
bun run check:bundle-size
```

The NSIS installer and updater signature are written under
`target/release/bundle/nsis/`; the MSI is under `target/release/bundle/msi/`.
A plain `cargo build --release` does not create or sign the Tauri updater
artifacts.

More packaging, updater signing, and log detail: [`desktop/README.md`](desktop/README.md).

> The root package `boris-assistant` is a **retired CLI stub**.  
> `cargo run` at the repo root prints a message and exits — it does **not** start the voice app.

### Rust workspace

```bash
# Library crates (no Tauri UI)
cargo test -p boris-core -p boris-ai -p boris-agent --lib
cargo test -p boris-audio -p boris-sense -p boris-inference --lib
cargo test -p boris-pipeline --lib
cargo check -p boris-pipeline --features stt-parakeet,tts-supertone

# Full product (needs the tracked wake ONNX + frontend toolchain)
cargo check -p boris-desktop
```

Crate-level docs live in each `crates/*/README.md`.

---

## Models

| Model | When needed | How |
|-------|-------------|-----|
| **Wake** (`boris-large.onnx`) | **Compile** of `boris-desktop` | Tracked release asset, embedded via `include_bytes!` from `assets/models/livekit/` |
| **Silero VAD** (`silero_vad.onnx`) | **Compile** of `boris-desktop` | Tracked MIT-licensed asset, embedded from `assets/models/silero/` |
| **Parakeet STT** | Runtime | App install / HF download into `~/.boris/models/parakeet` |
| **Supertone TTS** | Runtime | App install into `~/.boris/models/supertone` |

- `/assets` is ignored except for the versioned wake classifier and Silero VAD
  graph. Their provenance and SHA-256 values are recorded in
  [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES).
- Product runtime prefers **`~/.boris/models`** (download / bootstrap), not repo `assets/`.
- Override data root: `BORIS_HOME`.
- Override download base: `BORIS_MODEL_BASE_URL` (HTTPS only; downloaded models are verified against pinned SHA-256 digests).
- Hugging Face token (if required): `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN`.

A bare clone is enough to build `boris-desktop`.

---

## Configuration

### Environment (dev)

See [`.env.example`](.env.example):

```text
OPENROUTER_API_KEY=...
OPENROUTER_MODEL=...
```

Common runtime vars (full list in [`boris-pipeline`](crates/boris-pipeline/README.md)):

| Variable | Purpose |
|----------|---------|
| `BORIS_HOME` | Override `~/.boris` |
| `OPENROUTER_API_KEY` | LLM key |
| `OPENROUTER_MODEL` / `BORIS_STRONG_MODEL` | Strong model id |
| `BORIS_FAST_MODEL` | Fast model id |
| `BORIS_CAPABILITY` | `voice_safe` \| `local_power` \| `full` |
| `BORIS_TRUSTED` | `0` disables auto-allow for moderate-risk tools |
| `BORIS_MEMORY` | `0` disables long-term memory |
| `BORIS_LOG` / `RUST_LOG` | Log filters |

### User data (`~/.boris`)

```text
~/.boris/
  config.toml      # prefs
  auth.json        # secrets (plaintext)
  models/          # STT / TTS weights
  sessions/        # transcripts + per-session artifacts/
  memory/          # long-term notes
  skills/          # skill playbooks
  logs/            # boris-desktop.*.log
  workspace/       # sandboxed agent workspace
```

---

## Security

- Agent tools run under **capability presets** and **path sandboxes** (`~/.boris/workspace` and configured roots).
- **Shell** and higher-risk actions use **HITL** confirmation. Deny lists are best-effort — this is **not** a full sandbox. Do not market it as one.
- **Web fetch** blocks common private / link-local / metadata hosts; residual DNS-rebinding risk remains.
- Keep API keys in `.env` (gitignored) or `~/.boris/auth.json`. Never commit secrets. `auth.json` is **plaintext**.
- Full policy and private reporting: [SECURITY.md](SECURITY.md).

---

## License

Licensed under the **Apache License, Version 2.0**. See [LICENSE](LICENSE).

Copyright 2026 BlocksDevPro.

Third-party models and native libraries (ONNX Runtime, Parakeet, Supertone, wake weights, and so on) carry their **own** licenses. Apache-2.0 covers the Boris source in this repository, not every binary weight downloaded at runtime. See [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES).

---

## Status

Public product versions follow [semver](https://semver.org/). See [CHANGELOG.md](CHANGELOG.md).

| | |
|---|---|
| First stable | [1.0.0](https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.0.0) — 2026-08-12 |
| Current stable | [1.1.0](https://github.com/blocksdevpro/boris-assistant/releases/tag/v1.1.0) — faster routing/tools, streamed speech, async research, Silero VAD, and durable traces |
| Git `main` | Stable line (`1.1.x`) |
| Git `next` | Beta line — this tree is `1.2.0-beta.1` |
