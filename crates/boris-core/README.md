# boris-core

Shared foundation types for the Boris assistant workspace.

## Responsibility

| Provides | Does **not** provide |
|----------|----------------------|
| Audio sample / buffer aliases | Agent loop or tools |
| `AUDIO_TARGET_RATE` (16 kHz) | LLM HTTP clients |
| `TurnId`, `ServiceKind` | Engine phase / UI status |
| Shared `Error` / `Result` | Session FSM / worker buses |

## Public API (crate root)

Prefer importing from the crate root:

```rust
use boris_core::{
    AudioBuffer, AudioSample, ArcAudioBuffer, AUDIO_TARGET_RATE,
    TurnId, ServiceKind, Error, Result,
};
```

Modules `error` and `types` remain for organized source layout; re-exports at
the root are the stable surface for other crates.

## Design notes

- **Small on purpose.** Keep this crate free of `cpal`, ORT, HTTP, and Tokio so
  unit tests and CI can compile it everywhere.
- **Sequential engine.** The product voice path is a single engine thread, not
  a multi-worker event bus. Do not reintroduce session-FSM event types here.
- **Richer errors live higher.** `boris-agent` and `boris-ai` own domain errors;
  adapters map failures into `boris_core::Error` at the inference trait edge.

## Tests

```bash
cargo test -p boris-core --lib
```
