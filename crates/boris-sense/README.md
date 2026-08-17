# boris-sense

Local perception ports for the Boris voice pipeline: **wake-word scoring** and
**voice-activity detection**. No threads, no STT/TTS, no session policy.

## Features

| Feature   | Default | Dependencies                         |
|-----------|---------|--------------------------------------|
| `vad`     | on      | `ort` (Silero ONNX)                  |
| `wake`    | on      | `livekit-wakeword` (git), `ort`      |
| `speaker` | on      | `rustfft` (live-vs-loudspeaker cues) |

Desktop and `boris-pipeline` use the default set (`vad` + `wake`). `--features vad`
without `wake` still needs native ONNX Runtime. `--no-default-features` builds
only `pcm` / `time`.

## Audio contract

- **Sample rate:** all adapters expect mono PCM at **16 kHz**
  (`boris_core::AUDIO_TARGET_RATE`).
- **Sample type:** `f32` in roughly `[-1.0, 1.0]`. Wake still converts to PCM16
  with a **symmetric ±32767** clamp (`f32_to_pcm16_samples` / `_into`). Silero
  VAD consumes `f32` directly.
- **VAD hop (Silero):** **512 samples = 32 ms** at 16 kHz, plus 64 samples of
  rolling context kept inside the adapter. The pipeline must score **every** hop
  (`VAD_WINDOW_SIZE = 512`) so LSTM state stays aligned.
- **Wake window:** ~2 s rolling buffer (`WAKEWORD_WINDOW_SIZE = 32_000` samples).

## VAD model bytes

`SileroVad::try_new(model_bytes)` loads the official Silero streaming ONNX from
memory. Hosts embed the graph (e.g. `assets/models/silero/silero_vad.onnx`).
Call `init_onnx_runtime()` first. Call `Vad::reset()` at the start of each
independent utterance.

Default speech threshold is `0.5`. Hangover / endpointing stay in the pipeline
(`VAD_SILENCE_WINDOW` = 550 ms, matching LiveKit's Silero `min_silence_duration`;
confirm path uses 250 ms).

### WebRTC migration timing

The previous WebRTC/libfvad backend scored 160-sample (10 ms) frames every
40 ms and used 900 ms / 420 ms freeform/confirm hangover to tolerate GMM
flicker. Silero is stateful: the host must now score every 512-sample (32 ms)
hop without dropping or skipping samples. The endpoint windows are 550 ms for
freeform speech and 250 ms for short confirmations. These values are product
behavior, not model-internal padding.

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
wake or Silero VAD sessions. It configures a process-global 1-thread pool with
spin disabled and returns `Result<()>` so hosts can log hard pool-setup failures.

```rust
use boris_sense::{
    init_onnx_runtime,
    WakeWord, LivekitWakeWord, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
    Vad, SileroVad, VAD_WINDOW_SIZE,
};

fn main() -> boris_core::Result<()> {
    init_onnx_runtime()?;
    let mut wake = LivekitWakeWord::try_new("boris", &wake_bytes, 16_000)?;
    let mut vad = SileroVad::try_new(&vad_bytes)?;
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
    Vad, SileroVad, VAD_WINDOW_SIZE, SILERO_VAD_FRAME_SAMPLES_16K,
    init_onnx_runtime, f32_to_pcm16_samples, f32_to_pcm16_samples_into,
};
```

`LiveKitWakeWord` is a type alias of `LivekitWakeWord` (LiveKit adapter).

## Live vs loudspeaker

`compute_acoustic_feat` + [`AcousticModel::playback_z`] measure whether a wake
crop is darker / more band-limited than enrolled live takes (TV, Translate,
TTS out of a speaker). The pipeline owns enroll storage and the accept/reject
cutoff (`PLAYBACK_Z_REJECT` is only a suggested default).
