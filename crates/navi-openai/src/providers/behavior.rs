use crate::errors::ProviderError;
use crate::types::{OpenAiApiKind, StreamRoute};
use navi_core::ProviderId;
use reqwest::header::USER_AGENT;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

// ─── Provider base URLs ───────────────────────────────────────────────────────

const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const OPENCODE_ZEN_BASE_URL: &str = "https://opencode.ai/zen/v1";
const OPENCODE_GO_BASE_URL: &str = "https://opencode.ai/zen/go/v1";

const ANTHROPIC_VERSION: &str = "2023-06-01";
const OPENROUTER_REFERER: &str = "https://github.com/enrell/navi";
const OPENROUTER_TITLE: &str = "Navi";
const COPILOT_USER_AGENT: &str = "navi/0.1.0";
const COPILOT_INTENT: &str = "conversation-edits";
const COPILOT_INITIATOR: &str = "user";

/// Endpoint category — used to select auth headers per provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Responses,
    ChatCompletions,
    AnthropicMessages,
    Models,
}

/// Per-provider behavior: auth, routing, headers, URL construction.
pub(crate) trait ProviderBehavior: Send + Sync {
    /// Default base URL when none is configured. None means the provider requires base_url in config.
    fn default_base_url(&self) -> Option<&str>;

    /// Route a model name to a stream API variant.
    fn stream_route(&self, model: &str, configured_kind: OpenAiApiKind) -> StreamRoute;

    /// Build auth + extra headers for a request to the given endpoint.
    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError>;
}

/// Helper: create a `Bearer` authorization header value from an API key.
fn bearer_value(api_key: &str) -> Result<HeaderValue, ProviderError> {
    Ok(HeaderValue::from_str(&format!("Bearer {api_key}"))?)
}

/// Helper: build standard Bearer + optional Content-Type headers.
///
/// Most providers follow this pattern. Pass `content_type = true` for
/// endpoints that send a JSON body (ChatCompletions, AnthropicMessages).
fn standard_bearer_headers(api_key: &str, content_type: bool) -> Result<HeaderMap, ProviderError> {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, bearer_value(api_key)?);
    if content_type {
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    Ok(headers)
}

// ─── OpenAI ───────────────────────────────────────────────────────────────────

pub(crate) struct OpenAiBehavior;

impl ProviderBehavior for OpenAiBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(OPENAI_BASE_URL)
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError> {
        let content_type = matches!(
            endpoint,
            Endpoint::ChatCompletions | Endpoint::AnthropicMessages
        );
        standard_bearer_headers(api_key, content_type)
    }
}

// ─── Anthropic ────────────────────────────────────────────────────────────────

pub(crate) struct AnthropicBehavior;

impl ProviderBehavior for AnthropicBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None // requires base_url in config
    }

    fn stream_route(&self, _model: &str, _configured_kind: OpenAiApiKind) -> StreamRoute {
        StreamRoute::AnthropicMessages
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(api_key)?);
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

// ─── Google Gemini ────────────────────────────────────────────────────────────

pub(crate) struct GeminiBehavior;

impl ProviderBehavior for GeminiBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(GEMINI_BASE_URL)
    }

    fn stream_route(&self, _model: &str, _configured_kind: OpenAiApiKind) -> StreamRoute {
        StreamRoute::GeminiGenerateContent
    }

    fn build_headers(
        &self,
        _api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        // Gemini uses API key in URL query param, not in headers
        Ok(HeaderMap::new())
    }
}

// ─── OpenRouter ───────────────────────────────────────────────────────────────

pub(crate) struct OpenRouterBehavior;

impl ProviderBehavior for OpenRouterBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = standard_bearer_headers(api_key, true)?;
        headers.insert("HTTP-Referer", HeaderValue::from_static(OPENROUTER_REFERER));
        headers.insert("X-Title", HeaderValue::from_static(OPENROUTER_TITLE));
        Ok(headers)
    }
}

// ─── GitHub Copilot ───────────────────────────────────────────────────────────

pub(crate) struct GitHubCopilotBehavior;

impl ProviderBehavior for GitHubCopilotBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = standard_bearer_headers(api_key, true)?;
        headers.insert(USER_AGENT, HeaderValue::from_static(COPILOT_USER_AGENT));
        headers.insert("Openai-Intent", HeaderValue::from_static(COPILOT_INTENT));
        headers.insert("x-initiator", HeaderValue::from_static(COPILOT_INITIATOR));
        Ok(headers)
    }
}

// ─── Opencode ─────────────────────────────────────────────────────────────────

pub(crate) struct OpencodeBehavior;

impl ProviderBehavior for OpencodeBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(OPENCODE_ZEN_BASE_URL)
    }

    fn stream_route(&self, model: &str, _configured_kind: OpenAiApiKind) -> StreamRoute {
        let model = model
            .trim()
            .trim_start_matches("opencode/")
            .to_ascii_lowercase();
        if model.starts_with("gpt-") {
            StreamRoute::Responses
        } else if model.starts_with("claude-") {
            StreamRoute::AnthropicMessages
        } else {
            StreamRoute::ChatCompletions
        }
    }

    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError> {
        let content_type = matches!(
            endpoint,
            Endpoint::ChatCompletions | Endpoint::AnthropicMessages
        );
        standard_bearer_headers(api_key, content_type)
    }
}

// ─── Opencode Zen ─────────────────────────────────────────────────────────────

pub(crate) struct OpencodeZenBehavior;

impl ProviderBehavior for OpencodeZenBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(OPENCODE_ZEN_BASE_URL)
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        standard_bearer_headers(api_key, true)
    }
}

// ─── Opencode Go ──────────────────────────────────────────────────────────────

pub(crate) struct OpencodeGoBehavior;

impl ProviderBehavior for OpencodeGoBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(OPENCODE_GO_BASE_URL)
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        standard_bearer_headers(api_key, true)
    }
}

// ─── Groq ─────────────────────────────────────────────────────────────────────

pub(crate) struct GroqBehavior;

impl ProviderBehavior for GroqBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        standard_bearer_headers(api_key, true)
    }
}

// ─── Xai ──────────────────────────────────────────────────────────────────────

pub(crate) struct XaiBehavior;

impl ProviderBehavior for XaiBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        standard_bearer_headers(api_key, true)
    }
}

// ─── Custom (fallback) ────────────────────────────────────────────────────────

pub(crate) struct CustomBehavior;

impl ProviderBehavior for CustomBehavior {
    fn default_base_url(&self) -> Option<&str> {
        None
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        standard_bearer_headers(api_key, true)
    }
}

// ─── Factory ──────────────────────────────────────────────────────────────────

pub(crate) fn behavior_for_provider(provider_id: &ProviderId) -> Box<dyn ProviderBehavior> {
    match provider_id {
        ProviderId::OpenAi => Box::new(OpenAiBehavior),
        ProviderId::Anthropic => Box::new(AnthropicBehavior),
        ProviderId::GoogleGemini => Box::new(GeminiBehavior),
        ProviderId::OpenRouter => Box::new(OpenRouterBehavior),
        ProviderId::GitHubCopilot => Box::new(GitHubCopilotBehavior),
        ProviderId::Opencode => Box::new(OpencodeBehavior),
        ProviderId::OpencodeZen => Box::new(OpencodeZenBehavior),
        ProviderId::OpencodeGo => Box::new(OpencodeGoBehavior),
        ProviderId::Groq => Box::new(GroqBehavior),
        ProviderId::Xai => Box::new(XaiBehavior),
        ProviderId::Custom(_) => Box::new(CustomBehavior),
    }
}
