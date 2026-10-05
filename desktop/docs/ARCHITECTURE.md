# Desktop architecture

The desktop app has three code areas. The React app owns screens and display
rules. The Tauri host owns windows, IPC, engine lifecycle, and operating-system
integration. `boris-pipeline` owns voice turns, models, and settings persistence.

```text
React window
  -> src/bridge (invoke names, events, wire types)
    -> src-tauri/src/commands (Tauri boundary)
      -> orchestrator / host services
        -> crates/boris-pipeline (voice behavior and persistence)
```

Keep changes in `desktop/` focused on the UI, the Tauri boundary, and desktop
integration. Change pipeline behavior in the crates when the rule belongs to
the voice engine.

## Find a behavior

| Change or bug | Start here | Follow the call to |
|---|---|---|
| Main screen, settings, or voice setup UI | `src/windows/main/` | `HomeView.tsx`, `settings/`, or `TeachVoiceView.tsx` |
| Floating overlay wording or content | `src/lib/status/` | `src/windows/overlay/OverlayWindow.tsx` |
| Overlay window size, placement, visibility, or click-through | `src-tauri/src/overlay_win.rs` | Matching layout rules in `src/lib/status/overlay.ts` |
| Status appears stale or out of order | `src/bridge/useStatus.ts` | `src-tauri/src/orchestrator.rs`, then the pipeline status producer |
| Successful tool output is reported as failure | `crates/boris-agent/src/tool/observation.rs` | Runtime invocation, loop batching, context history, and `routing.rs`; see the [tool-result contract](../../crates/boris-agent/README.md#tool-execution-status) |
| Boris waits before speaking | `crates/boris-pipeline/src/engine/mod.rs` | `engine/speech.rs` and `crates/boris-agent/src/loop_/round.rs`; see the [current speech flow](../../crates/boris-pipeline/README.md#turn-loop) |
| A button invokes the wrong behavior or reports an IPC error | `src/bridge/` | The matching module in `src-tauri/src/commands/` |
| Settings save or autostart behavior | `src/windows/main/settings/` | `src/bridge/commands/settings.ts`, `src-tauri/src/commands/settings.rs`, then pipeline settings |
| Model installation or progress | `src/windows/main/MainWindow.tsx` | `src/bridge/commands/models.ts`, `src-tauri/src/commands/models.rs`, then pipeline install code |
| Session artifact content | `src/components/artifacts/` | `src/bridge/commands/artifacts.ts`, `src-tauri/src/commands/artifacts.rs`, then pipeline artifact code |
| Tray, startup, updater, or app logging | `src-tauri/src/` | `tray.rs`, `autostart.rs`, `updater.rs`, or `logging.rs` |

When a problem starts in the voice turn itself, follow the call into
`crates/boris-pipeline`; the desktop host forwards it and mirrors status, but it
does not decide wake, STT, agent, or TTS policy.

## Frontend map

```text
src/
  App.tsx                 Selects main vs overlay window and preview modes
  bridge/                 Typed Tauri commands, events, and status hook
    commands/             IPC calls grouped by engine, devices, models, settings, and artifacts
  windows/main/           Main screen, settings, and voice setup
  windows/overlay/        Floating overlay React view
  components/             Shared UI and artifact rendering
  lib/status/             Pure status-to-copy and overlay display rules
  lib/                    Logging, updater, markdown, and other helpers
  preview/                Browser fixtures and overlay debug harness
```

`src/bridge/ipc.ts` and the Rust command names form one IPC contract. Add or
change a command in both the typed bridge and the matching Rust command module.
Normalize host values in `bridge/types.ts`; keep display-only decisions in
`lib/` so the components that render them stay small.

The stable import `@/lib/statusPresentation` re-exports the status helpers.
Edit the narrower module directly:

- `status/activity.ts` humanizes activity strings from the pipeline.
- `status/conversation.ts` chooses speech captions, subtitles, and chat rows.
- `status/overlay.ts` chooses overlay presence, card size, and idle behavior.
- `status/labels.ts` removes repeated or echoed display text.
- `status/types.ts` owns the view types used by those helpers.

`windows/main/MainWindow.tsx` wires the views and engine actions. Settings
loading and debounced saves live in `windows/main/settings/useSettingsController.ts`.
Update checks and install progress live in `windows/main/useUpdateController.ts`.

## Tauri host map

```text
src-tauri/src/
  lib.rs                  App setup and registered IPC command list
  commands/
    engine.rs             Start/Stop, device changes, and wake enrollment
    status.rs             Snapshot and model-readiness commands
    models.rs             Model status and download progress
    settings.rs           Settings persistence and autostart
    artifacts.rs          Session artifact reads
    diagnostics.rs        Log path and frontend log intake
  orchestrator.rs         AppState, engine thread, Start/Stop, status ordering
  overlay_win.rs          Native overlay window, geometry, and visibility
  tray.rs                 Tray actions
  autostart.rs            Windows logon integration
  artifacts.rs            Thin host adapter for session artifact reads
  logging.rs              Rust and frontend logs
  updater.rs              Update feeds and install checks
```

`commands/` is an adapter layer. Put Tauri argument handling and blocking-task
dispatch there. Keep shared engine state and sequencing in `orchestrator.rs`.
Keep window geometry and visibility in `overlay_win.rs`. Do not copy voice
policy into either file.

## Trace status end to end

```text
pipeline StatusPicture
  -> orchestrator status mirror (drops old generations and orders snapshots)
    -> Tauri "status" event
      -> bridge/useStatus.ts (subscribes, pulls initial snapshot, rejects old sequence)
        -> lib/status/* (derives visible copy and layout mode)
          -> main or overlay React view
```

The React overlay and native host both decide when an artifact needs the larger
overlay window. Keep `shouldShowOverlayCard` in `status/overlay.ts` aligned with
`wants_card_layout` in `src-tauri/src/overlay_win.rs`. If the React content is
clipped or floats in empty space, inspect both decisions.

## Design choices

The desktop keeps separate UI, IPC, and host layers because each owns a
different boundary. Within those layers, related rules are grouped by topic.
Moving every feature into one vertical folder would mix React state, Tauri
commands, and operating-system code; keeping one large file per layer made
specific rules hard to locate. This layout keeps the call path visible while
letting each topic fit in a small module.
