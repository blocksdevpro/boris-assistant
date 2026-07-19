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
