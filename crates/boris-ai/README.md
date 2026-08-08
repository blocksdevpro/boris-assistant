# boris-ai

LLM **provider plane** for Boris: HTTP clients, parsing helpers, no agent loop.

## Layout

```text
src/
  client.rs          LlmClient trait
  error.rs           LlmError / LlmErrorKind
  message.rs         content + tool_calls helpers
  model_pref.rs      model@provider / provider list
  usage.rs           TokenUsage + logging
  stream.rs          optional mpsc event helper
  providers/
    openrouter/
      client.rs      construction / timeouts / session
      request.rs     chat completion JSON body
      complete.rs    stream-first complete + blocking fallback
      sse.rs         SSE line parse + StreamAssembler
```

## Public API (stable for hosts)

```rust
use boris_ai::{
    LlmClient, OpenRouterClient, LlmError, LlmErrorKind,
    TokenUsage, parse_provider_list, split_model_and_provider,
    DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT,
};
```

`boris-agent` re-exports the same surface so desktop/pipeline can keep
`boris_agent::OpenRouterClient`.

## Behaviour notes

- `complete` tries **SSE first**, then falls back to a single JSON response if
  the stream fails or yields an empty (no text, no tools) payload.
- Assistant `content` is normalized to a **string** for the agent loop.
- `session_id` enables OpenRouter sticky routing for prompt-cache hits.

## Tests

```bash
cargo test -p boris-ai --lib
```

## Adding a provider

1. Implement `LlmClient` in `providers/<name>/`.
2. Re-export from `providers/mod.rs` and `lib.rs` if hosts need it.
3. Do not put provider HTTP inside `boris-agent`.
