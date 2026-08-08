# boris-audio

Real-time duplex audio for Boris: capture → 16 kHz mono, TTS mono → device playback.

## Public surface

```rust
use boris_audio::{
    AudioService, AUDIO_TARGET_RATE, OutputEvent, DeviceInfo, Direction,
};
use boris_audio::buffer::{SlidingBuffer, RecordingBuffer};
```

Historical paths still work:

- `boris_audio::service::{AudioService, DeviceInfo, Direction}`
- `boris_audio::output::OutputEvent`

## Threads

| Path | RT callback | Worker |
|------|-------------|--------|
| Input | f32 convert + `try_send` only | resample + fan-out |
| Output | pull samples + drain detect | command recv + oneshot resample |

Never block inside cpal callbacks.

## Tests

```bash
cargo test -p boris-audio --lib
```

Hardware integration tests are not run by default (headless CI).
