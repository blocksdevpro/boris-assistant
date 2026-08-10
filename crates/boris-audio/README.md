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
| Output | pull samples + drain detect (`try_send` events) | command recv + oneshot resample |

Never block inside cpal callbacks.

`AudioService::play` returns `Result` via non-blocking `try_send` (queue full / worker gone).
`OutputEvent::Started` means samples are queued for the device callback, not that the first sample has hit the DAC.

## Tests

```bash
cargo test -p boris-audio --lib
```

Hardware integration tests are not run by default (headless CI).
