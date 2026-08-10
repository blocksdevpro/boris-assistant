# boris-ai

LLM **provider plane** for Boris: HTTP clients, parsing helpers, no agent loop.

## Layout

```text
src/
  client.rs          LlmClient trait
  error.rs           LlmError / LlmErrorKind (+ HTTP status mapping)
  message.rs         content + tool_calls helpers
  model_pref.rs      model@provider / provider list
  usage.rs           TokenUsage + logging
  stream.rs          optional mpsc event helper (not crate-root re-exported)
  providers/
    openrouter/
      client.rs      construction / timeouts / base URL / session
      request.rs     chat completion JSON body
      complete.rs    stream-first complete + blocking fallback
      sse.rs         SSE line parse + StreamAssembler
```

## Public API (stable for hosts)

```rust
use boris_ai::{
    LlmClient, OpenRouterClient, LlmError, LlmErrorKind,
    TokenUsage, parse_provider_list, split_model_and_provider,
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT, DEFAULT_BASE_URL, DEFAULT_MODEL,
};
```

`boris-agent` re-exports the same core surface so desktop/pipeline can keep
`boris_agent::OpenRouterClient`.

## Behaviour notes

- `complete` tries **SSE first**, then falls back to a single JSON response if
  the stream fails or yields an empty (no text, no tools) payload.
- Assistant `content` is normalized to a **string** for the agent loop.
- `session_id` is sent as JSON and as `x-session-id` on **both** streaming and
  blocking requests (OpenRouter sticky routing / prompt-cache hits).
- Non-success HTTP statuses map to [`LlmErrorKind::Provider`] (or Timeout for
  408/504); transport failures stay [`LlmErrorKind::Http`].
- HTTP 200 bodies with a top-level `error` object are treated as provider errors.
- SSE assembly flushes a final unterminated line when the byte stream ends.
- Multi-line SSE events are **not** reassembled (single-line `data:` only).
- Default model (`DEFAULT_MODEL`) is owned by this crate as a last-resort
  fallback when the host passes `None`; product defaults should set a model
  explicitly.
- Base URL is injectable via `OpenRouterClient::with_base_url` (default OpenRouter).

## Tests

```bash
cargo test -p boris-ai
cargo check -p boris-ai
```

## Adding a provider

1. Implement `LlmClient` in `providers/<name>/`.
2. Re-export from `providers/mod.rs` and `lib.rs` if hosts need it.
3. Do not put provider HTTP inside `boris-agent`.
