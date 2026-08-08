# boris-inference

Object-safe **ports** (traits) for speech models used by Boris.

## Responsibility

| Provides | Does **not** provide |
|----------|----------------------|
| `SpeechToText` trait | Concrete STT models |
| `TextToSpeech` trait | Concrete TTS models |
| Shared `load` / `unload` defaults | Wake-word / VAD |
| | Agent / LLM clients |

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
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String> {
        let _ = audio;
        Ok(String::new())
    }
}
```

## Design notes

- **Keep this crate thin.** No `ort`, no vendor SDKs, no Tokio.
- **Object-safe on purpose** so the engine can erase backends.
- **`Send` only** (not `Sync`): models run on the single engine thread.

## Tests

```bash
cargo test -p boris-inference --lib
```
