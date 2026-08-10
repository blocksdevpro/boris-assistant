# boris-core

Shared foundation types for the Boris assistant workspace.

## Responsibility

| Provides | Does **not** provide |
|----------|----------------------|
| Audio sample / buffer aliases | Agent loop or tools |
| `AUDIO_TARGET_RATE` (16 kHz mono f32) | LLM HTTP clients |
| `TurnId` | Engine phase / UI status |
| Shared `Error` / `Result` | Session FSM / worker buses |

## Public API (crate root)

Prefer importing from the crate root. Modules are private; only re-exports are public:

```rust
use boris_core::{
    AudioBuffer, AudioSample, ArcAudioBuffer, AUDIO_TARGET_RATE,
    TurnId, Error, Result,
};
```

## Audio layout

- **Sample type:** mono interleaved `f32` (`AudioSample`)
- **Amplitude:** approximately `[-1.0, 1.0]` (soft full-scale)
- **Rate:** [`AUDIO_TARGET_RATE`] = 16 kHz after input resampling
- **Shared buffers:** prefer `Arc::from(owned_vec)` for `ArcAudioBuffer` (no sample copy); `Arc::from(slice)` copies

## Error taxonomy

| Domain | Variant | Examples |
|--------|---------|----------|
| Paths, settings, env | `Error::Config` | missing model path, bad settings |
| Device I/O, capture, playback, resample | `Error::Audio` | mic open failed, device busy |
| Model load, runtime, unclassified | `Error::Other` | ONNX load, adapter glue |

`From<String>` / `From<&str>` **only** create `Error::Other`. Classified failures should use `Error::config`, `Error::audio`, or `Error::other`. A future `Model` variant may absorb model/runtime errors; until then use `Other`.

Display prefixes are stable contracts: `config error: …`, `audio error: …`; `Other` displays the raw message.

## Design notes

- **Small on purpose.** Keep this crate free of `cpal`, ORT, HTTP, and Tokio so
  unit tests and CI can compile it everywhere.
- **Sequential engine.** The product voice path is a single engine thread, not
  a multi-worker event bus. Do not reintroduce session-FSM event types here.
- **Richer errors live higher.** `boris-agent` and `boris-ai` own domain errors;
  adapters map failures into `boris_core::Error` at the inference trait edge.
- **`TurnId` is opaque.** Construct with `TurnId::new` / `From<u64>`; `next()` saturates at `u64::MAX`.

## Tests

```bash
cargo test -p boris-core --lib
```
