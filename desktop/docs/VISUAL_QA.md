# Visual QA

Run the desktop frontend in a regular browser:

```powershell
npm run dev
```

Use these development-only routes:

- `http://localhost:1420/?window=main` — main window without Tauri chrome APIs.
- `http://localhost:1420/?window=overlay&fixture=thinking-tool` — one deterministic overlay state.
- `http://localhost:1420/?preview=overlay-matrix` — every canonical overlay state on one page.

Fixture names are `off`, `ready`, `hearing`, `reading`, `thinking`,
`thinking-tool`, `confirm`, `talking`, `fault`, `long-caption`,
`device-faults`, and `artifact-card`.

For release QA, capture each overlay fixture at a 380 × 120 viewport and the
main window at 640 × 480. Check that no text, focus ring, island shadow, or
device warning is clipped. The matrix deliberately gives each overlay 160px
of vertical inspection space so unexpected overflow remains visible.

Run deterministic presentation checks with:

```powershell
npm test
```
