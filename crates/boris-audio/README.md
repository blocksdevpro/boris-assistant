# boris-audio

Real-time duplex audio for Boris: capture → 16 kHz mono → HPF/AGC/AEC, TTS mono → device playback.

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
| Input | f32 convert + `try_send` only | resample → HPF/AGC/AEC → fan-out |
| Output | pull samples + drain detect (`try_send` events) | command recv + oneshot resample + AEC far-end |

Never block inside cpal callbacks.

After resample, the input worker runs a WebRTC APM (sonora): high-pass, AGC2, and AEC3. TTS PCM is copied to 16 kHz and fed as the AEC far-end, in lockstep with capture frames. Noise suppression stays off — it tends to hurt Parakeet. `BORIS_AUDIO_FRONTEND=0` bypasses the APM for debugging.

`AudioService::play` returns `Result` via non-blocking `try_send` (queue full / worker gone).
Streamed `append` uses a short bounded enqueue wait. `finish_job` is a reliable,
bounded, worker-acknowledged control transition; event-loop hosts can use
`request_finish_job` to retry/poll without blocking command handling.
`pause` / `resume` are the same acknowledged control path: the device writes
silence while paused and keeps leftover PCM so speech can continue from the
cut. `stop` / `Flush` still discard the job.
`OutputEvent::Started` means samples are queued for the device callback, not that the first sample has hit the DAC.

## Tests

```bash
cargo test -p boris-audio --lib
```

Hardware integration tests are not run by default (no audio hardware required).
