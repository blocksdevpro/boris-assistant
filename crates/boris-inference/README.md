# boris-inference

Object-safe **ports** (traits) for speech models used by Boris.

## Responsibility

| Provides | Does **not** provide |
|----------|----------------------|
| `SpeechToText` trait | Concrete STT models |
| `TextToSpeech` trait | Concrete TTS models |
| Shared `load` / `unload` / `is_loaded` / `backend_id` defaults | Wake-word / VAD |
| `TextToSpeech::sample_rate` | Agent / LLM clients |

Adapters live in sibling crates:

- `boris-stt-parakeet` → `SpeechToText`
- `boris-tts-supertone` / `boris-tts-kokoro` → `TextToSpeech`

The product engine (`boris-pipeline`) stores `Box<dyn …>` and feature-gates which
adapter is constructed.

## Public API

```rust
use boris_inference::{SpeechToText, TextToSpeech};
use boris_core::{AudioSample, AudioBuffer, Result};

struct MyStt;
impl SpeechToText for MyStt {
    fn backend_id(&self) -> &str { "my-stt" }
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
        if audio.is_empty() {
            return Ok(String::new());
        }
        Ok(String::new())
    }
}

struct MyTts;
impl TextToSpeech for MyTts {
    fn backend_id(&self) -> &str { "my-tts" }
    fn sample_rate(&self) -> u32 { 24_000 }
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        Ok(Vec::new())
    }
}
```

## Implementor checklist

### STT (`SpeechToText`)

1. Override `load` / `unload` when holding weights; keep them idempotent.
2. Override `is_loaded` to match real state (default is `false`).
3. Set `backend_id` to a stable short name (`"parakeet"`, …).
4. Map missing/invalid model paths → `Error::config(...)`.
5. Empty audio → `Ok("")` (never panic).
6. Prefer lazy-load *or* clear error if unloaded — document which.

### TTS (`TextToSpeech`)

1. Same lifecycle / `is_loaded` / `backend_id` rules as STT.
2. **Always** implement `sample_rate()` with the native mono PCM rate (Hz).
   Hosts use this for playback resampling. Return a fixed rate before load when
   the model rate is known a priori.
3. Output is mono `f32` in roughly `[-1.0, 1.0]`.
4. Empty / whitespace text → `Ok(empty buffer)` so playback can be skipped.
5. Missing model/voice paths → `Error::config(...)`.
6. Prefer lazy-load *or* clear error if unloaded — document which.

### Error mapping

| Kind | Use |
|------|-----|
| Missing dir, bad voice id, invalid settings | `Error::Config` |
| Inference / vendor runtime failure | `Error::Other` |
| Capture / device issues (rare in adapters) | `Error::Audio` |

## Design notes

- **Keep this crate thin.** No `ort`, no vendor SDKs, no Tokio.
- **Object-safe on purpose** so the engine can erase backends.
- **`Send` only** (not `Sync`): models run on the single engine thread.

## Tests

```bash
cargo test -p boris-inference --lib
```
