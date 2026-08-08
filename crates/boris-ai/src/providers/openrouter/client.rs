//! [`OpenRouterClient`] construction and configuration.

use std::time::Duration;

use reqwest::Client;

use crate::model_pref::parse_provider_list;

use super::request::CHAT_COMPLETIONS_URL;

/// Default TCP connect timeout for OpenRouter requests.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default overall request timeout (connect + TTFB + body).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Default model when the host does not pass one.
pub const DEFAULT_MODEL: &str = "google/gemini-2.5-flash-lite";

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
pub struct OpenRouterClient {
    pub(super) api_key: String,
    pub(super) model: String,
    /// OpenRouter provider slugs tried in order (e.g. `["coreweave", "baseten"]`).
    pub(super) provider_order: Vec<String>,
    /// When `provider_order` is set: whether to fall back to other providers.
    pub(super) allow_fallbacks: bool,
    /// Sticky routing key for cache-friendly multi-turn sessions.
    pub(super) session_id: Option<String>,
    pub(super) http: Client,
}

impl OpenRouterClient {
    /// Create a client with default timeouts and model.
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self::build(api_key, model, DEFAULT_CONNECT_TIMEOUT, DEFAULT_TIMEOUT)
    }

    /// Override connect and overall request timeouts (builder-style).
    pub fn with_timeouts(mut self, connect: Duration, total: Duration) -> Self {
        self.http = build_http_client(connect, total);
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
        self.session_id = if s.trim().is_empty() {
            None
        } else {
            Some(s)
        };
        self
    }

    /// Model id configured for this client.
    pub fn model(&self) -> &str {
        &self.model
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

    /// Chat completions endpoint URL (test/diag helper).
    pub fn endpoint_url() -> &'static str {
        CHAT_COMPLETIONS_URL
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
            http: build_http_client(connect_timeout, timeout),
        }
    }

    pub(super) fn authorization_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }
}

fn build_http_client(connect: Duration, total: Duration) -> Client {
    Client::builder()
        .connect_timeout(connect)
        .timeout(total)
        .build()
        .unwrap_or_else(|_| Client::new())
}
