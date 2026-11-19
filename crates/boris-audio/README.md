# boris-audio

Real-time duplex audio for Boris: capture → 16 kHz mono → HPF/AGC/AEC, TTS mono → device playback.

## Public surface

```rust
use boris_audio::{
    AudioHealth, AudioService, AUDIO_TARGET_RATE, OutputEvent, DeviceInfo, Direction,
    StreamHealth,
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
`OutputEvent::Started` means the first real sample was written in the device
callback (audible after driver buffer latency), not that it has hit the DAC.

## Devices

`AudioService::with_source_rate` opens the OS **default** mic and speaker
through cpal's default-device handles. On Windows, cpal 0.18 reroutes those
streams when the system default changes (`ErrorKind::DeviceChanged`, stream
keeps running), so an unpicked device follows Windows automatically.

- `switch_input` / `switch_output` open a specific device. The new stream
  opens before the old one closes; on failure the old stream keeps running.
  Switching to the device already open is a no-op unless its stream died or
  it was only open as the default.
- `use_default_input` / `use_default_output` go back to following the default.
- `health()` returns a cloneable `AudioHealth`. `input()` / `output()` /
  `stream(direction)` return a `StreamHealth` snapshot read under one lock:
  the open device id, whether it follows the default, and whether its
  stream's error callback reported loss (unplugged, invalidated). Reroutes,
  xruns, and refused real-time priority are not loss. Each open gets a fresh
  flag, so a late error from a replaced device never marks the new one failed.

Recovery policy (reopen, fall back, return to a re-plugged device) lives in
`boris-pipeline`'s device watcher, not here.

## Tests

```bash
cargo test -p boris-audio --lib
```

Hardware integration tests are not run by default (no audio hardware required).
