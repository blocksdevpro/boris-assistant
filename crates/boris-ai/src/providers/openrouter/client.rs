//! [`OpenRouterClient`] construction and configuration.

use std::time::Duration;

use reqwest::Client;

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
    pub(super) http: Client,
}

impl OpenRouterClient {
    /// Create a client with default timeouts, base URL, and model.
    ///
    /// Panics if the underlying `reqwest::Client` cannot be constructed
    /// (TLS backend misconfigured / system configuration). See
    /// [`Self::with_timeouts`], which shares the same client-construction path.
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self::build(api_key, model, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT)
    }

    /// Override connect and overall request timeouts (builder-style).
    ///
    /// Rebuilds the underlying `reqwest::Client` with the given timeouts.
    /// Panics if the client cannot be constructed (TLS backend misconfigured);
    /// timeouts are never silently dropped.
    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.http = build_http_client(connect, total);
        self
    }

    /// Override the API base URL (builder-style).
    ///
    /// Default: [`DEFAULT_BASE_URL`]. Trailing slashes are stripped.
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
    /// Empty list → OpenRouter default load-balancing / sticky routing.
    pub fn with_provider_order(
        mut self,
        order: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.provider_order = order.into_iter().map(|s| s.into()).collect();
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
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens.max(1_024);
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

    /// Configured OpenRouter model-provider order (may be empty).
    pub fn provider_order(&self) -> &[String] {
        &self.provider_order
    }

    /// Whether fallbacks outside `provider_order` are allowed.
    pub fn allow_fallbacks(&self) -> bool {
        self.allow_fallbacks
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
            http: build_http_client(connect_timeout, timeout),
        }
    }

    pub(super) fn authorization_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    /// Shared auth + session headers for streaming and blocking requests.
    pub(super) fn apply_common_headers(
        &self,
        mut req: reqwest::RequestBuilder,
    ) -> reqwest::RequestBuilder {
        req = req.header("Authorization", self.authorization_header());
        if let Some(sid) = self.session_id.as_deref() {
            // Header form is also supported; body session_id takes precedence if both set.
            req = req.header("x-session-id", sid);
        }
        req
    }
}

fn normalize_base_url(mut base: String) -> String {
    while base.ends_with('/') {
        base.pop();
    }
    base
}

/// Build a reqwest client that **always** has connect + total timeouts.
///
/// Does not fall back to `Client::new()` (which has no timeouts). Panics with a
/// clear message if the TLS/backend stack cannot construct a client.
fn build_http_client(connect: Duration, total: Duration) -> Client {
    Client::builder()
        .connect_timeout(connect)
        .timeout(total)
        .build()
        .expect(
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
    }
}
