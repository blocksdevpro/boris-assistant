# boris-sense

Local perception ports for the Boris voice pipeline: **wake-word scoring** and
**voice-activity detection**. No threads, no STT/TTS, no session policy.

## Features

| Feature   | Default | Dependencies                         |
|-----------|---------|--------------------------------------|
| `vad`     | on      | `webrtc-vad`                         |
| `wake`    | on      | `livekit-wakeword` (git), `ort`      |

Desktop and `boris-pipeline` use the default set (`vad` + `wake`). Disable
`wake` if you need a build without ONNX Runtime.

## Audio contract

- **Sample rate:** all adapters expect mono PCM at **16 kHz**
  (`boris_core::AUDIO_TARGET_RATE`).
- **Sample type:** `f32` in roughly `[-1.0, 1.0]`; converted to PCM16 with a
  **symmetric ±32767** clamp (`f32_to_pcm16_samples` / `_into`).
- **VAD frame sizes (WebRTC):** only **10 / 20 / 30 ms** at 16 kHz —
  **160 / 320 / 480** samples. The pipeline uses 10 ms (`VAD_WINDOW_SIZE = 160`).
- **Wake window:** ~2 s rolling buffer (`WAKEWORD_WINDOW_SIZE = 32_000` samples).

## Wake model bytes

`LivekitWakeWord::try_new(model_name, model_bytes, sample_rate)` loads a
LiveKit **open-wake-word classifier** ONNX blob from memory. Mel spectrogram
and speech-embedding graphs are bundled inside the `livekit-wakeword` crate;
you only embed the classifier weights (e.g. `boris.onnx` from
`assets/models/livekit/`).

`model_name` is the classifier key used when multi-label score maps are
returned. `select_wake_score` prefers an exact match on that key, then falls
back to a case-insensitive match on the key, and only then falls back to the
max score across all entries (so multi-label models never depend on
`HashMap` iteration order).

## ORT init

Call `init_onnx_runtime()` **once** at process start **before** constructing
wake models. It configures a process-global 1-thread pool with spin disabled
and returns `Result<()>` so hosts can log hard pool-setup failures.

```rust
use boris_sense::{
    init_onnx_runtime, f32_to_pcm16_samples,
    WakeWord, LivekitWakeWord, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
    Vad, WebRtcVad, VAD_WINDOW_SIZE,
};

fn main() -> boris_core::Result<()> {
    init_onnx_runtime()?;
    let mut wake = LivekitWakeWord::try_new("boris", &model_bytes, 16_000)?;
    let mut vad = WebRtcVad::new();
    // …
    Ok(())
}
```

## Git dependency note

`livekit-wakeword` comes from
[`blocksdevpro/rust-sdks`](https://github.com/blocksdevpro/rust-sdks) and is
**pinned by git `rev`** in `Cargo.toml` (matching the workspace `Cargo.lock`).
Bump the rev deliberately when upgrading; do not leave it floating on branch
HEAD.

## API surface

```rust
use boris_sense::{
    WakeWord, LivekitWakeWord, LiveKitWakeWord, // alias
    WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
    Vad, WebRtcVad, VAD_WINDOW_SIZE, WEBRTC_VAD_FRAME_SAMPLES_16K,
    init_onnx_runtime, f32_to_pcm16_samples, f32_to_pcm16_samples_into,
};
```

`LiveKitWakeWord` is a type alias of `LivekitWakeWord` (LiveKit adapter).
