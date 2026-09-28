# boris-tts-supertone

Product [`TextToSpeech`](boris_inference::TextToSpeech) adapter for
**Supertonic 3** via [`st-tts`](https://crates.io/crates/st-tts).

## Audio contract

| Property | Value |
|----------|--------|
| Sample rate | **44.1 kHz** ([`SUPERTONE_SAMPLE_RATE`]) |
| Channels | mono |
| Sample format | `f32` in roughly `[-1.0, 1.0]` |

Hosts must resample to the output device when needed
(the pipeline plays at 44_100 — see `boris-pipeline` play config).

## Paths

```rust
use boris_inference::TextToSpeech;
use boris_tts_supertone::SupertoneTts;

let mut tts = SupertoneTts::with_paths(
    "/path/to/supertone/onnx",
    "/path/to/supertone/voices",
    "M4",
)
.with_speed(1.05)
.with_total_step(8)
.with_silence_duration(0.15);
tts.load()?;
let pcm = tts.synthesize("Hello.")?;
```

Builder defaults: `total_step = 8`, `speed = 1.05`, `silence_duration = 0.0`,
language `"en"`. Clamps: steps floor at 1, non-finite / non-positive speed
falls back to `1.0`, non-finite / negative silence falls back to `0.0`.

- `model_dir` — ONNX graph + `tts.json` (Supertonic **3** multilingual).
- `voice_dir` — `voices/<id>.json` style packs.
- Voice ids must be simple basenames (no path separators / `..`).

English-only Supertonic 1 installs (`opensource-en` in `tts.json`) are
rejected at load with `Error::Config`.

## Silence between units

Long replies are split into [`speakable_units`]. **Inter-unit silence** is
owned by this adapter (`with_silence_duration`). We pass
`silence_duration = 0` into st-tts so its internal chunk padding does not
**double-pad** with our unit gaps.

## Tokio / threading

`st-tts` synthesize is `async`. This adapter keeps a **private 2-worker
Tokio runtime** and drives it with `block_on` on the **sync engine thread**.

- **Supported host:** a non-async (or non-entered) OS thread — e.g. the
  pipeline voice engine thread.
- **Not supported:** calling `synthesize` from inside an already-entered
  Tokio runtime. A plain nested `block_on` would panic, so the adapter
  checks `Handle::try_current()` first and returns `Error::Other` instead.

The runtime is built **lazily** on first `load` / `synthesize` (no panic in
`with_paths`).

## Load policy

- Prefer explicit `load()` (engine preloads while the agent thinks).
- `synthesize` lazy-loads if unloaded.
- Missing dirs / voice / wrong model family → `Error::Config`.
- Empty / whitespace text → `Ok(empty buffer)`.

## Trait methods

| Method | Value |
|--------|--------|
| `backend_id` | `"supertone"` |
| `sample_rate` | model rate after load, else `44_100` |
| `inter_unit_silence_samples` | `silence_duration × sample_rate` (rounded) |
| `is_loaded` | after successful load |

## Smoke example

```bash
# Requires installed models under ~/.boris/models/supertone
cargo run -p boris-tts-supertone --example smoke_synth
```

## Tests

```bash
cargo test -p boris-tts-supertone --lib
```
