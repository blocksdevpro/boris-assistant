# Boris Assistant

**Boris** is a Windows-first desktop voice assistant: wake word → listen →
transcribe → tool-using agent → speak. The product host is a **Tauri v2** app
(`boris-desktop`); speech and agent logic live in a Rust Cargo workspace under
`crates/`.

| | |
|---|---|
| **Product entrypoint** | [`desktop/`](desktop/) → `boris-desktop` |
| **Voice engine** | [`crates/boris-pipeline`](crates/boris-pipeline) |
| **Agent / tools** | [`crates/boris-agent`](crates/boris-agent) |
| **License** | [Apache-2.0](LICENSE) |
| **Contributing** | [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md) · [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) |
| **Repository** | https://github.com/blocksdevpro/boris-assistant |

> **Note:** The root package `boris-assistant` is a **retired CLI stub**.  
> `cargo run` at the repo root is intentional and does **not** start the voice app.  
> Always run the desktop host (below).

---

## Features

- **Hands-free loop** — wake word, VAD capture, STT, agent turn, TTS playback
- **Tool-using agent** — files, shell (HITL), web fetch, memory, skills, sessions
- **Capability presets** — `voice_safe` / `local_power` / `full` sandbox policy
- **Local models** — Parakeet STT, Supertone TTS, LiveKit-style wake (ONNX)
- **Cloud LLM** — OpenRouter (OpenAI-compatible) via `boris-ai`
- **User home** — `~/.boris` for config, keys, models, logs, sessions, memory

---

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│  desktop/  (React + Vite + Bun + Tauri)                     │
│    UI  ↔  IPC  ↔  boris-desktop (src-tauri)                 │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│  boris-pipeline  — single engine thread, sequential turns   │
│    Armed → wake → hear → STT → agent → TTS → play           │
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
| [`boris-agent`](crates/boris-agent) | Tool runtime, policy, memory, sessions |
| [`boris-pipeline`](crates/boris-pipeline) | Desktop voice engine + `~/.boris` + model install |
| [`boris-desktop`](desktop/src-tauri) | Tauri shell, tray, overlay, packaging |

---

## Prerequisites

- **Windows** (primary target) — mic/speaker, ORT / DirectML packaging documented for desktop
- [Rust](https://rustup.rs) (stable, edition 2021)
- [Bun](https://bun.sh) (desktop frontend)
- [Tauri v2 platform deps](https://v2.tauri.app/start/prerequisites/)
- An [OpenRouter](https://openrouter.ai/) API key (or compatible setup)
- **Wake model at build time** (see [Models](#models))

---

## Quick start (desktop)

```bash
# 1. Clone
git clone https://github.com/blocksdevpro/boris-assistant.git
cd boris-assistant

# 2. API key (optional; can also be set in the app / ~/.boris/auth.json)
cp .env.example .env
# edit .env — OPENROUTER_API_KEY=...

# 3. Wake ONNX required for desktop compile (path is gitignored under assets/)
#    Place: assets/models/livekit/boris-large.onnx
#    See "Models" below.

# 4. Run
cd desktop
bun install
bun run tauri dev
```

Release build:

```bash
cd desktop
bun run tauri build
```

More desktop/ops detail: [`desktop/README.md`](desktop/README.md).

---

## Models

| Model | When needed | How |
|-------|-------------|-----|
| **Wake** (`boris-large.onnx`) | **Compile** of `boris-desktop` | Embedded via `include_bytes!` from `assets/models/livekit/` (directory is **gitignored** — you must supply the file locally) |
| **Parakeet STT** | Runtime | App install / HF download into `~/.boris/models/parakeet` |
| **Supertone TTS** | Runtime | App install into `~/.boris/models/supertone` |

- `/assets` is listed in `.gitignore` (large weights and local-only trees).
- Product runtime prefers **`~/.boris/models`** (download / bootstrap), not repo `assets/`.
- Override data root: `BORIS_HOME`.
- Override download base: `BORIS_MODEL_BASE_URL`.
- Hugging Face token (if required): `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN`.

Until the wake ONNX is relocated into a tracked resource path, **a bare git clone cannot build `boris-desktop` without placing that file**.

---

## Configuration

### Environment (dev)

See [`.env.example`](.env.example):

```text
OPENROUTER_API_KEY=...
OPENROUTER_MODEL=...
```

Common runtime vars (also documented in [`boris-pipeline` README](crates/boris-pipeline/README.md)):

| Variable | Purpose |
|----------|---------|
| `BORIS_HOME` | Override `~/.boris` |
| `OPENROUTER_API_KEY` | LLM key |
| `OPENROUTER_MODEL` / `BORIS_STRONG_MODEL` | Strong model id |
| `BORIS_FAST_MODEL` | Fast model id |
| `BORIS_CAPABILITY` | `voice_safe` \| `local_power` \| `full` |
| `BORIS_LOG` / `RUST_LOG` | Log filters |
| `BORIS_MEMORY` | `0` disables long-term memory |

### User data (`~/.boris`)

```text
~/.boris/
  config.toml      # prefs
  auth.json        # secrets (API key)
  models/          # STT / TTS weights
  sessions/        # transcripts
  memory/          # long-term notes
  skills/          # skill playbooks
  logs/            # boris-desktop.*.log
  workspace/       # sandboxed agent workspace
```

---

## Develop the Rust workspace

```bash
# Library crates (no Tauri UI)
cargo test -p boris-core -p boris-ai -p boris-agent --lib
cargo test -p boris-audio -p boris-sense -p boris-inference --lib
cargo test -p boris-pipeline --lib
cargo check -p boris-pipeline --features stt-parakeet,tts-supertone

# Full product (needs wake ONNX + frontend toolchain)
cargo check -p boris-desktop
```

Crate-level docs live in each `crates/*/README.md`.

---

## Security notes (high level)

- Agent tools run under **capability presets** and **path sandboxes** (`~/.boris/workspace` and configured roots).
- **Shell** and higher-risk actions use **HITL** confirmation; deny lists are best-effort, not a full sandbox.
- **Web fetch** blocks common private/link-local/metadata hosts; residual DNS-rebinding risk remains (documented in agent web tools).
- Keep API keys in `.env` (gitignored) or `~/.boris/auth.json` — never commit secrets. `auth.json` is **plaintext** secret storage.
- Full policy and private reporting: [SECURITY.md](SECURITY.md).

---

## Logs

Packaged Windows builds have no console. Diagnostics go to:

```text
%USERPROFILE%\.boris\logs\boris-desktop.YYYY-MM-DD.log
```

See [`desktop/README.md`](desktop/README.md) for ORT/DirectML packaging and log filters.

---

## License

Licensed under the **Apache License, Version 2.0**. See [LICENSE](LICENSE).

Copyright 2026 BlocksDevPro.

Third-party models and native libraries (ONNX Runtime, Parakeet, Supertone, wake weights, etc.)
carry their **own** licenses; this Apache-2.0 grant covers the Boris source in this repository,
not every binary weight you download at runtime. See [THIRD-PARTY-NOTICES](THIRD-PARTY-NOTICES).

---

## Status

Early **0.1.x** — actively developed. APIs and crate surfaces may change. Crates are
marked `publish = false` until intentionally released to crates.io.
