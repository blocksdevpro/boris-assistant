# boris-ai

LLM **provider plane** for Boris: HTTP clients, parsing helpers, no agent loop.

## Layout

```text
src/
  client.rs          LlmClient trait (private module; re-exported at crate root)
  error.rs           LlmError / LlmErrorKind (+ HTTP status mapping) — public module
  message.rs         content + tool_calls helpers (private, internal use only)
  model_pref.rs      model@provider / provider list (private module; re-exported fns)
  usage.rs           TokenUsage + logging (private module; TokenUsage re-exported)
  stream.rs          optional mpsc event helper — public module, not crate-root re-exported
  providers/
    openrouter/
      client.rs      construction / timeouts / base URL / session
      reasoning.rs    ReasoningConfig / ReasoningEffort (OpenRouter `reasoning` object)
      request.rs     chat completion JSON body
      complete.rs    stream-first complete + blocking fallback
      sse.rs         SSE line parse + StreamAssembler
```

Only `error` and `stream` are `pub mod`; everything else is a private module
whose public items are re-exported from the crate root. Prefer
`boris_ai::Thing` over reaching into a submodule path.

## Public API (stable for hosts)

```rust
use boris_ai::{
    LlmClient, OpenRouterClient, LlmError, LlmErrorKind,
    TokenUsage, parse_provider_list, split_model_and_provider,
    ReasoningConfig, ReasoningEffort,
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT, DEFAULT_BASE_URL, DEFAULT_MODEL,
    DEFAULT_MAX_TOKENS,
};
```

`boris-agent` re-exports the same core surface so desktop/pipeline can keep
`boris_agent::OpenRouterClient`.

## Behaviour notes

- `complete` tries **SSE first**, then falls back to a single JSON response if
  the stream fails or yields an empty (no text, no tools) payload.
- Tool-planning and complex stages send `reasoning.exclude = false` so thinking
  tokens arrive as `LlmStreamEvent::ReasoningDelta`. Simple voice still excludes
  them. Reasoning is never assembled into `content`.
- Assistant `content` is normalized to a **string** for the agent loop.
- `session_id` is sent as JSON and as `x-session-id` on **both** streaming and
  blocking requests (OpenRouter sticky routing / prompt-cache hits).
- Non-success HTTP statuses map to [`LlmErrorKind::Provider`] (or Timeout for
  408/504); transport failures stay [`LlmErrorKind::Http`].
- HTTP 200 bodies with a top-level `error` object are treated as provider errors.
  A top-level `error` object arriving **mid-stream** as an SSE `data:` payload
  is also detected and aborts assembly with the same `LlmErrorKind::Provider`
  error, instead of silently producing an empty message.
- SSE bytes are buffered **raw** (`Vec<u8>`) across network chunks and only
  decoded once a complete line has accumulated — a multi-byte UTF-8 character
  split across two TCP chunks decodes correctly instead of corrupting into
  replacement characters on both halves.
- SSE assembly flushes a final unterminated line when the byte stream ends.
- Multi-line SSE events are **not** reassembled (single-line `data:` only).
- Default model (`DEFAULT_MODEL`) is owned by this crate as a last-resort
  fallback when the host passes `None`; product defaults should set a model
  explicitly.
- Base URL is injectable via `OpenRouterClient::with_base_url` (default OpenRouter).
- Reasoning defaults to [`ReasoningConfig::default`] (`High` effort, reasoning
  text excluded from the response, `DEFAULT_MAX_TOKENS` = 24,576 completion
  headroom). `ReasoningEffort::None` explicitly sends `"enabled": false`
  rather than omitting the field.

## Tests

```bash
cargo test -p boris-ai
cargo check -p boris-ai
```

## Adding a provider

1. Implement `LlmClient` in `providers/<name>/`.
2. Re-export from `providers/mod.rs` and `lib.rs` if hosts need it.
3. Do not put provider HTTP inside `boris-agent`.
