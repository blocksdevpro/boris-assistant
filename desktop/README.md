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

### ONNX Runtime (Windows)

Wake-word inference uses the `ort` crate. On Windows, `build.rs` stages
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
