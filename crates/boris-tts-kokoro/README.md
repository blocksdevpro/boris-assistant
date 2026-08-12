# boris-tts-kokoro

**Experimental** [`TextToSpeech`](boris_inference::TextToSpeech) adapter for
[Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) via
[`any-tts`](https://crates.io/crates/any-tts) (Candle).

## Status

This backend is available for local experiments and A/B comparison with
Supertone. The product voice path currently prefers Supertone. API and
defaults may change.

## Model files

Point [`KokoroTts::with_model_path`] at a directory that contains at least:

| Path | Required |
|------|----------|
| `config.json` | yes |
| `kokoro-v1_0.pth` **or** `model.safetensors` / similar weights | yes |
| `voices/<voice>.pt` | yes (for the configured voice) |

**No automatic HuggingFace download.** The `any-tts` dependency is built with
`default-features = false, features = ["kokoro"]` only.

## Usage

```rust
use boris_inference::TextToSpeech;
use boris_tts_kokoro::KokoroTts;

let mut tts = KokoroTts::with_model_path("/path/to/kokoro")
    .with_voice("bm_lewis")
    .with_language("English");
tts.load()?;
let pcm = tts.synthesize("Hello from Kokoro.")?;
assert_eq!(tts.sample_rate(), 24_000);
```

### Load policy

- Prefer explicit `load()`.
- `synthesize` lazy-loads if unloaded.
- Missing / incomplete model path → `Error::Config`.
- Empty / whitespace text → `Ok(empty buffer)`.

### Trait methods

| Method | Value |
|--------|--------|
| `backend_id` | `"kokoro"` |
| `sample_rate` | `24_000` (mono f32) |
| `is_loaded` | after successful load |

### Long text

Kokoro works best on short-to-medium phrases. Very long monologues may need
host-side splitting (the product engine does not split for Kokoro today).

## Tests

```bash
cargo test -p boris-tts-kokoro --lib
```

Unit tests do not require full weights (path storage, empty text, missing dir).
