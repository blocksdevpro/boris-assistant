# Boris Desktop

Tauri v2 + React + TypeScript + Vite + Bun + Tailwind CSS + shadcn/ui.

## Prerequisites

- [Bun](https://bun.sh)
- [Rust](https://rustup.rs) (stable)
- Platform deps for [Tauri](https://v2.tauri.app/start/prerequisites/)

## Develop

```bash
cd desktop
bun install
bun run tauri dev
```

## Build

```bash
bun run tauri build
```

Windows betas ship **NSIS only** (`Boris_*_x64-setup.exe`). MSI/WiX cannot encode
`1.1.0-beta.1` — the pre-release label must be numeric. The updater uses the NSIS
installer either way.

The current release-prep tree is **`1.1.0-beta.5`**.

## App updates (Tauri updater)

Release builds can self-update from GitHub Releases using signed installers.

| Piece | Location |
|-------|----------|
| Plugin (Rust) | `tauri-plugin-updater` + `tauri-plugin-process` in `src-tauri` |
| Plugin (JS) | `@tauri-apps/plugin-updater`, `@tauri-apps/plugin-process` |
| Version peek | GitHub Releases API (`/releases?per_page=20`). Stable = newest non-prerelease tag; Beta = newest `v*-beta.N` tag (skips the rolling `beta` tag) |
| Installer feed | Stable: `/releases/latest/download/latest.json`. Beta: `/releases/download/beta/latest.json`. Used only when the peek says a newer version exists |
| Public key | `plugins.updater.pubkey` (matches `.tauri/boris.key.pub`) |
| Private key | `.tauri/boris.key` — **gitignored**, required to sign builds |

The same `latest.json` URL is instant in a browser and used to hang in-process: a fresh `reqwest` client on Windows asks WinHTTP for the system proxy (WPAD/PAC) on every host, then does a cold TLS handshake and follows the asset-CDN redirect. The browser already has that PAC result and an HTTP/2 socket. The updater skips WinHTTP WPAD, caps connect at 3s, and never hits the CDN just to learn it is already current.

Signing and publishing steps: [`.tauri/README.md`](../.tauri/README.md).

```powershell
# Sign a release build (PowerShell)
# tauri build reads TAURI_SIGNING_PRIVATE_KEY (path or key contents), not _PATH
$env:TAURI_SIGNING_PRIVATE_KEY = (Resolve-Path ..\.tauri\boris.key).Path
# $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""   # if key has empty/no password prompt issues
bun run tauri build
```

In the UI: **Settings → General → Updates**. Default channel is **Stable**. **Beta** opts into the long-lived `beta` GitHub pre-release (see [`.tauri/README.md`](../.tauri/README.md)). A home banner appears when a newer version is published on the selected channel.

## Logs (debug packaged / release builds)

Release Windows builds have **no console window**, so **all** pipeline diagnostics
go to disk. On another PC after install:

```text
%USERPROFILE%\.boris\logs\boris.YYYY-MM-DD.log
```

What gets logged by default (**DEBUG** for all `boris_*` crates):

1. **Boot diagnostics** — exe path, sidecar DLLs (`onnxruntime.dll` / `DirectML.dll`),
   `~/.boris` layout, model dir listings, preflight, mic/speaker list
2. **Engine lifecycle** — audio open, wakeword ORT load, Start/Stop, every turn stage
3. **Wake loop** — heartbeat every ~5s with max score (proves mic + model alive)
4. **STT / agent / TTS** — timings, failures with directory dumps
5. **Panics** — full payload into the same file

### On a broken install (other computer)

1. Launch Boris, try Start (or just open the app).
2. Copy today’s log from `%USERPROFILE%\.boris\logs\`.
3. Search for: `DIAGNOSTICS`, `FAILED`, `PANIC`, `MISSING`, `error`.

### Control verbosity

```powershell
# Quieter
$env:RUST_LOG = "warn"
# Even louder
$env:RUST_LOG = "trace"
# or
$env:BORIS_LOG = "debug"
```

Override home with `BORIS_HOME` if you need logs elsewhere.

### ONNX Runtime (Windows)

Wake-word and Silero VAD inference use the `ort` crate. On Windows, `build.rs` stages
`onnxruntime.dll` / `DirectML.dll` (from `target/{profile}/` after ort's
`copy-dylibs`, or the pyke download cache) into `src-tauri/resources/ort/`.
Tauri `bundle.resources` then installs those DLLs **next to** `Boris.exe`
so a clean machine does not need a separate ORT install.

**Verify after install (or open the NSIS/MSI payload):** `onnxruntime.dll`
and/or `DirectML.dll` sit beside the app executable.

## Add UI components

```bash
bunx shadcn@latest add <component>
```

## Layout

```text
desktop/
├── src/                 # React frontend
│   ├── components/ui/   # shadcn components
│   ├── lib/utils.ts
│   └── App.tsx
└── src-tauri/           # Tauri / Rust host
```

Workspace member: `desktop/src-tauri` (see root `Cargo.toml`).
