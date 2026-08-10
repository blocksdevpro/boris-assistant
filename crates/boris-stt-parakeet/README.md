# boris-stt-parakeet

[`SpeechToText`](https://docs.rs) adapter wrapping **NVIDIA Parakeet** via
[`transcribe-rs`](https://crates.io/crates/transcribe-rs) ONNX.

## Model contract (v2 English / TDT int8 layout)

Point [`ParakeetStt::with_model_dir`] at a directory that contains:

| File | Required | Notes |
|------|----------|--------|
| `encoder-model.int8.onnx` | yes (int8) | Or `encoder-model.onnx` for FP32 |
| `decoder_joint-model.int8.onnx` | yes (int8) | Or FP32 sibling |
| `nemo128.onnx` | yes | Mel preprocessor |
| `vocab.txt` | yes | Must include `<blk>` blank token |

Product install path: `~/.boris/models/parakeet` (or workspace
`assets/models/parakeet` for local dev).

Quantization defaults to **Int8** (matches shipped product weights). Use
[`ParakeetStt::with_quantization`] for FP32 installs.

## Usage

```rust
use boris_inference::SpeechToText;
use boris_stt_parakeet::ParakeetStt;

let mut stt = ParakeetStt::with_model_dir("/path/to/parakeet");
// Optional: stt = stt.with_language("en");
stt.load()?;
let text = stt.transcribe(&pcm_mono_f32_16khz)?;
```

### Load policy

- Prefer explicit `load()` (engine preloads while capturing).
- `transcribe` also lazy-loads if unloaded.
- Missing / incomplete model dir → `Error::Config`.
- Empty audio → `Ok("")` without calling the model.

### Trait methods

| Method | Value |
|--------|--------|
| `backend_id` | `"parakeet"` |
| `is_loaded` | `true` after successful `load` |
| empty audio | `Ok("")` |

## Tests

Unit tests do **not** require ONNX weights:

```bash
cargo test -p boris-stt-parakeet --lib
```
