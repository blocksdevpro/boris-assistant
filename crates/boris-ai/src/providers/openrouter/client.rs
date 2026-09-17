//! [`OpenRouterClient`] construction and configuration.

use std::time::Duration;

use reqwest::Client;

use crate::error::LlmError;
use crate::model_pref::parse_provider_list;

use super::reasoning::{ReasoningConfig, DEFAULT_MAX_TOKENS};

/// Default TCP connect timeout for OpenRouter requests.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default overall request timeout (connect + TTFB + body).
///
/// Reasoning models (DeepSeek / Gemini thinking / o-series) often need more
/// than 60s for a single tool-planning step; 180s avoids mid-think timeouts.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);

/// Default OpenRouter API base URL (`…/api/v1`). Chat completions is
/// `{base}/chat/completions`.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Default model when the host does not pass one (tool-capable preferred).
///
/// **Ownership:** this crate owns the fallback string for unconfigured hosts.
/// Product defaults (voice pipeline, desktop settings) may override via
/// `OpenRouterClient::new(..., Some(model))` / [`OpenRouterClient::with_model`].
/// Changing this constant only affects callers that pass `None`.
pub const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash-0731";

/// Conservative fallback when a host has not supplied provider model metadata.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 128_000;

/// OpenRouter Chat Completions client.
///
/// # Model-provider routing
///
/// Supports optional `provider.order` so hosts can pin inference endpoints
/// (CoreWeave / Baseten / SiliconFlow, …) — the **host** of the weights, not
/// the model author (Google/OpenAI).
///
/// # Prompt cache stickiness
///
/// When a `session_id` is set, OpenRouter sticky-routes turns to the same
/// endpoint to maximize cache hits (`usage.prompt_tokens_details.cached_tokens`).
/// The id is sent both as JSON `session_id` and as the `x-session-id` header
/// on streaming **and** blocking requests.
pub struct OpenRouterClient {
    pub(super) api_key: String,
    pub(super) model: String,
    /// OpenRouter provider slugs tried in order (e.g. `["coreweave", "baseten"]`).
    pub(super) provider_order: Vec<String>,
    /// When `provider_order` is set: whether to fall back to other providers.
    pub(super) allow_fallbacks: bool,
    /// Sticky routing key for cache-friendly multi-turn sessions.
    pub(super) session_id: Option<String>,
    /// API base URL without trailing slash (default OpenRouter).
    pub(super) base_url: String,
    /// Thinking / reasoning budget (OpenRouter unified `reasoning` object).
    pub(super) reasoning: ReasoningConfig,
    /// Completion token cap (must exceed reasoning allocation on some models).
    pub(super) max_tokens: u32,
    /// Provider-advertised combined prompt + completion window.
    pub(super) context_window_tokens: u32,
    /// Configured TCP connect timeout (also applied to the HTTP client).
    pub(super) connect_timeout: Duration,
    /// Configured overall request timeout: connect + time-to-first-byte +
    /// full SSE body. There is **no separate idle/stream-stall timeout** —
    /// a slow-but-trickling stream can hold the request open until this
    /// total deadline. See `idle_timeout` for the planned stall detector.
    pub(super) timeout: Duration,
    /// Optional per-request override of [`Self::timeout`], applied on every
    /// request via `RequestBuilder::timeout`. `None` keeps the client-level
    /// total. Lets future `CompleteOptions` stage budgets shrink/extend the
    /// deadline without rebuilding the client.
    pub(super) request_timeout: Option<Duration>,
    /// Optional stall detector budget (accepted but **not yet enforced** —
    /// TODO: wrap SSE chunk reads in `complete.rs` with this idle deadline
    /// and surface `LlmErrorKind::Timeout`). Stored now so hosts can already
    /// configure the intended policy.
    pub(super) idle_timeout: Option<Duration>,
    /// Optional `HTTP-Referer` header (OpenRouter app attribution).
    pub(super) referer: Option<String>,
    /// Optional `X-Title` header (OpenRouter app attribution).
    pub(super) title: Option<String>,
    pub(super) http: Client,
}

impl OpenRouterClient {
    /// Create a client with default timeouts, base URL, and model.
    ///
    /// Never panics: if the `reqwest::Client` cannot be built with the
    /// configured timeouts (e.g. malformed proxy env vars), construction
    /// falls back to a default client and logs a warning. See
    /// [`try_build_http_client`].
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self::build(api_key, model, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT)
    }

    /// Override connect and overall request timeouts (builder-style).
    ///
    /// Rebuilds the underlying `reqwest::Client` with the given timeouts and
    /// records them for [`Self::timeout`] / [`Self::connect_timeout`].
    /// Never panics — falls back like [`Self::new`] on build failure.
    /// A per-request [`Self::with_request_timeout`] override, if set, is kept
    /// and still wins over `total` on each request.
    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.connect_timeout = connect;
        self.timeout = total;
        self.http = http_client_or_default(connect, total);
        self
    }

    /// Override the overall request timeout for every request without
    /// rebuilding the client (builder-style).
    ///
    /// Applied per request on top of the client-level [`Self::timeout`];
    /// `None` (default) keeps the client-level total. Intended for stage
    /// budgets (e.g. short SimpleVoice turns) set by the host.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    /// Accept (but not yet enforce) an SSE idle/stall budget (builder-style).
    ///
    /// Stub: stored for future `complete.rs` enforcement (currently the 180s
    /// [`Self::timeout`] total is the only deadline, so a stalled stream
    /// waits until then). Pass `None` to clear.
    pub fn with_idle_timeout(mut self, idle: Option<Duration>) -> Self {
        self.idle_timeout = idle;
        self
    }

    /// Override the API base URL (builder-style).
    ///
    /// Default: [`DEFAULT_BASE_URL`]. Leading/trailing whitespace and
    /// trailing slashes are stripped; a trailing `/chat/completions` suffix
    /// is removed so callers can paste a full endpoint URL without causing
    /// a doubled `…/chat/completions/chat/completions` path.
    /// Chat completions are requested at `{base}/chat/completions`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = normalize_base_url(base_url.into());
        self
    }

    /// Override the default model (builder-style).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Prefer specific OpenRouter **model-providers** (inference hosts) in order.
    ///
    /// Entries are trimmed, lowercased, and empties dropped (same
    /// normalization as [`parse_provider_list`]), so `"Baseten "` and
    /// `"baseten"` pin the same host.
    /// Empty list → OpenRouter default load-balancing / sticky routing.
    pub fn with_provider_order(
        mut self,
        order: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.provider_order = order
            .into_iter()
            .map(|s| s.into())
            .map(|s: String| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// Parse a free-form provider string (comma/space separated) into `provider.order`.
    pub fn with_provider_pref(mut self, raw: impl AsRef<str>) -> Self {
        self.provider_order = parse_provider_list(raw.as_ref());
        self
    }

    /// Whether OpenRouter may try other providers when the preferred list fails.
    ///
    /// Default `true`. Set `false` to hard-pin to `provider_order` only.
    ///
    /// Note: this flag is only sent when `provider_order` is non-empty (see
    /// `request_body_with`); with an empty order `allow_fallbacks: false` is
    /// silently a no-op because there is no `provider` object to attach it
    /// to. Set the order first (or together) when pinning.
    pub fn with_allow_fallbacks(mut self, allow: bool) -> Self {
        self.allow_fallbacks = allow;
        self
    }

    /// Session id for OpenRouter sticky routing (prompt-cache hits across turns).
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        let s = session_id.into();
        self.session_id = if s.trim().is_empty() { None } else { Some(s) };
        self
    }

    /// Set OpenRouter reasoning effort (thinking tokens).
    ///
    /// Defaults to [`ReasoningConfig::default`] (`high`, exclude body).
    pub fn with_reasoning(mut self, reasoning: ReasoningConfig) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// Cap on completion tokens (reasoning + visible answer share this pool on
    /// some providers). Default [`DEFAULT_MAX_TOKENS`].
    ///
    /// Floored at 1 (not 1024) so small stage budgets such as
    /// `RequestStage::SimpleVoice` (768 tokens) pass through unchanged;
    /// per-request `CompleteOptions` values are likewise only floored at 1.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens.max(1);
        self
    }

    /// Set the configured model's combined prompt + completion window.
    ///
    /// Floored at 1024 to reject obvious misconfiguration (e.g. unset/zero
    /// metadata) while still allowing small-context local models.
    pub fn with_context_window_tokens(mut self, tokens: u32) -> Self {
        self.context_window_tokens = tokens.max(1_024);
        self
    }

    /// Optional `HTTP-Referer` header for OpenRouter app attribution
    /// (builder-style). Blank values clear the header.
    pub fn with_referer(mut self, referer: impl Into<String>) -> Self {
        self.referer = sanitize_optional_header("referer", referer.into());
        self
    }

    /// Optional `X-Title` header for OpenRouter app attribution
    /// (builder-style). Blank values clear the header.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = sanitize_optional_header("title", title.into());
        self
    }

    /// Model id configured for this client.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Current reasoning config.
    pub fn reasoning(&self) -> &ReasoningConfig {
        &self.reasoning
    }

    /// Current max_tokens.
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    /// Configured combined prompt + completion window.
    pub fn context_window_tokens(&self) -> u32 {
        self.context_window_tokens
    }

    /// Configured OpenRouter model-provider order (may be empty).
    pub fn provider_order(&self) -> &[String] {
        &self.provider_order
    }

    /// Whether fallbacks outside `provider_order` are allowed.
    pub fn allow_fallbacks(&self) -> bool {
        self.allow_fallbacks
    }

    /// Configured TCP connect timeout.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }

    /// Configured overall request timeout (connect + TTFB + full SSE body).
    ///
    /// This deadline covers the **entire** SSE stream; there is no separate
    /// idle timeout. Use [`Self::request_timeout`] for the effective
    /// per-request deadline when an override is set.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Effective per-request deadline: the [`Self::with_request_timeout`]
    /// override when set, otherwise [`Self::timeout`].
    pub fn effective_timeout(&self) -> Duration {
        self.request_timeout.unwrap_or(self.timeout)
    }

    /// Per-request timeout override, if any.
    pub fn request_timeout(&self) -> Option<Duration> {
        self.request_timeout
    }

    /// Accepted-but-not-yet-enforced SSE idle budget, if any.
    pub fn idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }

    /// Configured `HTTP-Referer` header value, if any.
    pub fn referer(&self) -> Option<&str> {
        self.referer.as_deref()
    }

    /// Configured `X-Title` header value, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Sticky session id, if any.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// API base URL (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Chat completions endpoint URL for this client (test/diag helper).
    pub fn endpoint_url(&self) -> String {
        self.chat_completions_url()
    }

    pub(super) fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn build(
        api_key: String,
        model: Option<String>,
        connect_timeout: Duration,
        timeout: Duration,
    ) -> Self {
        Self {
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            provider_order: Vec::new(),
            allow_fallbacks: true,
            session_id: None,
            base_url: DEFAULT_BASE_URL.to_string(),
            reasoning: ReasoningConfig::default(),
            max_tokens: DEFAULT_MAX_TOKENS,
            context_window_tokens: DEFAULT_CONTEXT_WINDOW_TOKENS,
            connect_timeout,
            timeout,
            request_timeout: None,
            idle_timeout: None,
            referer: None,
            title: None,
            http: http_client_or_default(connect_timeout, timeout),
        }
    }

    pub(super) fn authorization_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    /// Shared auth + session headers for streaming and blocking requests.
    ///
    /// Also applies the per-request [`Self::with_request_timeout`] override
    /// when set (`RequestBuilder::timeout` wins over the client-level total
    /// for that request only), and attaches `HTTP-Referer` / `X-Title` when
    /// configured. Called by both the streaming and blocking paths, so the
    /// override covers the whole SSE stream as well.
    pub(super) fn apply_common_headers(
        &self,
        mut req: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        req = req.header("Authorization", self.authorization_header());
        if let Some(sid) = self.session_id.as_deref() {
            // Header form is also supported; body session_id takes precedence if both set.
            req = req.header("x-session-id", sid);
        }
        if let Some(referer) = self.referer.as_deref() {
            req = req.header("HTTP-Referer", referer);
        }
        if let Some(title) = self.title.as_deref() {
            req = req.header("X-Title", title);
        }
        if let Some(per_request) = self.request_timeout {
            req = req.timeout(per_request);
        }
        req
    }
}

/// Trim whitespace, strip trailing slashes, and drop a trailing
/// `/chat/completions` path suffix (callers may paste a full endpoint URL).
fn normalize_base_url(base: String) -> String {
    let mut base = base.trim().to_string();
    loop {
        while base.ends_with('/') {
            base.pop();
        }
        if let Some(stripped) = base.strip_suffix("/chat/completions") {
            base = stripped.to_string();
        } else {
            break;
        }
    }
    base
}

/// Trim an optional attribution header value; blank/invalid values become
/// `None` so a misconfigured referer/title can never panic request building
/// (reqwest rejects control characters in header values).
fn sanitize_optional_header(field: &'static str, raw: String) -> Option<String> {
    let value = raw.trim().to_string();
    if value.is_empty() {
        return None;
    }
    match reqwest::header::HeaderValue::from_str(&value) {
        Ok(_) => Some(value),
        Err(e) => {
            tracing::warn!(field, error = %e, "ignoring invalid attribution header value");
            None
        }
    }
}

/// Build a reqwest client that **always** has connect + total timeouts.
///
/// Returns [`LlmError`] instead of panicking so constructors can degrade
/// gracefully (notably on malformed proxy env vars, which reqwest surfaces
/// as a builder error).
pub(super) fn try_build_http_client(connect: Duration, total: Duration) -> Result<Client, LlmError> {
    Client::builder()
        .connect_timeout(connect)
        .timeout(total)
        .build()
        .map_err(|e| {
            LlmError::config(format!(
                "failed to build HTTP client (connect={connect:?}, timeout={total:?}): {e}"
            ))
        })
}

/// Build the timed client, falling back to a default (timeout-less) client —
/// with a warning — when the timed build fails. Never panics, so
/// [`OpenRouterClient::new`] / [`OpenRouterClient::with_timeouts`] are safe
/// to call during startup even with a broken proxy/TLS environment.
fn http_client_or_default(connect: Duration, total: Duration) -> Client {
    match try_build_http_client(connect, total) {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "timed HTTP client build failed; falling back to a default client without explicit timeouts"
            );
            Client::builder().build().unwrap_or_else(|fallback_err| {
                tracing::warn!(
                    error = %fallback_err,
                    "default HTTP client build also failed; using Client::new()"
                );
                Client::new()
            })
        }
    }
}

/// Build a reqwest client that **always** has connect + total timeouts.
///
/// Kept for backward compatibility (and the unit test below): prefer
/// [`try_build_http_client`] in new code. Panics with a clear message if the
/// TLS/backend stack cannot construct a client.
#[allow(dead_code)] // Constructors use the fallible path; kept as a panicking wrapper.
fn build_http_client(connect: Duration, total: Duration) -> Client {
    try_build_http_client(connect, total).expect(
        "failed to build reqwest Client with timeouts; check TLS backend / system configuration",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_http_client_succeeds_with_timeouts() {
        let _ = build_http_client(Duration::from_secs(1), Duration::from_secs(5));
    }

    #[test]
    fn with_base_url_strips_trailing_slash() {
        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("http://127.0.0.1:9/v1/");
        assert_eq!(c.base_url(), "http://127.0.0.1:9/v1");
        assert_eq!(c.endpoint_url(), "http://127.0.0.1:9/v1/chat/completions");
    }

    #[test]
    fn default_base_url_is_openrouter() {
        let c = OpenRouterClient::new("k".into(), None);
        assert_eq!(c.base_url(), DEFAULT_BASE_URL);
        assert!(c.endpoint_url().ends_with("/chat/completions"));
        assert_eq!(c.model(), DEFAULT_MODEL);
        assert_eq!(c.context_window_tokens(), DEFAULT_CONTEXT_WINDOW_TOKENS);
    }

    #[test]
    fn context_window_is_configurable_and_safely_clamped() {
        let configured =
            OpenRouterClient::new("k".into(), Some("m".into())).with_context_window_tokens(96_000);
        assert_eq!(configured.context_window_tokens(), 96_000);

        let clamped =
            OpenRouterClient::new("k".into(), Some("m".into())).with_context_window_tokens(10);
        assert_eq!(clamped.context_window_tokens(), 1_024);
    }

    #[test]
    fn max_tokens_floor_allows_small_voice_budgets() {
        // SimpleVoice stage budget (768) must survive the client-side floor.
        let c = OpenRouterClient::new("k".into(), Some("m".into())).with_max_tokens(768);
        assert_eq!(c.max_tokens(), 768);
        let c = OpenRouterClient::new("k".into(), Some("m".into())).with_max_tokens(0);
        assert_eq!(c.max_tokens(), 1);
    }

    #[test]
    fn normalize_base_url_trims_and_strips_endpoint_suffix() {
        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("  http://127.0.0.1:9/v1///  ");
        assert_eq!(c.base_url(), "http://127.0.0.1:9/v1");
        assert_eq!(c.endpoint_url(), "http://127.0.0.1:9/v1/chat/completions");

        // Pasting a full endpoint URL must not double-append the path.
        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(c.base_url(), "https://openrouter.ai/api/v1");
        assert_eq!(
            c.endpoint_url(),
            "https://openrouter.ai/api/v1/chat/completions"
        );

        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_base_url("https://example.com/v1/chat/completions///");
        assert_eq!(
            c.endpoint_url(),
            "https://example.com/v1/chat/completions"
        );
    }

    #[test]
    fn provider_order_is_trimmed_and_lowercased() {
        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_provider_order(["Baseten ", "  COREWEAVE", "", "   "]);
        assert_eq!(c.provider_order(), &["baseten".to_string(), "coreweave".to_string()]);
    }

    #[test]
    fn timeout_getters_and_request_override() {
        let c = OpenRouterClient::new("k".into(), Some("m".into()));
        assert_eq!(c.connect_timeout(), DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(c.timeout(), DEFAULT_TIMEOUT);
        assert_eq!(c.effective_timeout(), DEFAULT_TIMEOUT);
        assert_eq!(c.request_timeout(), None);
        assert_eq!(c.idle_timeout(), None);

        let c = c
            .with_timeouts(Duration::from_secs(3), Duration::from_secs(30))
            .with_request_timeout(Duration::from_secs(5))
            .with_idle_timeout(Some(Duration::from_secs(7)));
        assert_eq!(c.connect_timeout(), Duration::from_secs(3));
        assert_eq!(c.timeout(), Duration::from_secs(30));
        assert_eq!(c.effective_timeout(), Duration::from_secs(5));
        assert_eq!(c.idle_timeout(), Some(Duration::from_secs(7)));

        let c = c.with_idle_timeout(None);
        assert_eq!(c.idle_timeout(), None);
    }

    #[test]
    fn referer_and_title_builders_roundtrip_and_clear() {
        let c = OpenRouterClient::new("k".into(), Some("m".into()))
            .with_referer("https://example.com/app")
            .with_title("Boris");
        assert_eq!(c.referer(), Some("https://example.com/app"));
        assert_eq!(c.title(), Some("Boris"));

        // Blank clears; invalid header values are dropped, never panic.
        let c = c.with_referer("   ").with_title("bad\nvalue");
        assert_eq!(c.referer(), None);
        assert_eq!(c.title(), None);
    }

    #[test]
    fn constructor_never_panics_with_invalid_proxy_env() {
        // Malformed proxy env vars must never panic construction: the timed
        // build reports a `Result` (see `try_build_http_client`) and the
        // constructors degrade to a fallback client with a warning.
        // (reqwest 0.13 evaluates env proxies per request, so the timed
        // build itself may still succeed — the invariant under test is only
        // that construction cannot panic.)
        const KEYS: [&str; 6] = [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ];
        let saved: Vec<(String, Option<std::ffi::OsString>)> = KEYS
            .iter()
            .map(|k| (k.to_string(), std::env::var_os(k)))
            .collect();
        for k in KEYS {
            std::env::set_var(k, "http://[::1-namedport");
        }
        let result = std::panic::catch_unwind(|| {
            let c = OpenRouterClient::new("k".into(), Some("m".into()))
                .with_timeouts(Duration::from_secs(1), Duration::from_secs(2));
            assert_eq!(c.model(), "m");
            assert_eq!(c.connect_timeout(), Duration::from_secs(1));
            assert_eq!(c.timeout(), Duration::from_secs(2));
            // Timed build helper always yields a usable client or a typed error.
            let _ = try_build_http_client(Duration::from_secs(1), Duration::from_secs(2));
        });
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        assert!(result.is_ok(), "constructor panicked on invalid proxy env");
    }
}
