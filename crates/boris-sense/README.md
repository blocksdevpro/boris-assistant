# boris-sense

Wake-word + VAD perception ports (no threads, no STT/TTS).

## API

```rust
use boris_sense::{
    WakeWord, LivekitWakeWord, WAKEWORD_THRESHOLD, WAKEWORD_WINDOW_SIZE,
    Vad, WebRtcVad, VAD_WINDOW_SIZE,
    init_onnx_runtime, f32_to_pcm16_samples,
};
```

Call [`init_onnx_runtime`] once at process start before constructing wake models.
