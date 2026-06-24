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
const COMMANDCODE_BASE_URL: &str = "https://api.commandcode.ai";

const ANTHROPIC_VERSION: &str = "2023-06-01";
const OPENROUTER_REFERER: &str = "https://github.com/enrell/navi";
const OPENROUTER_TITLE: &str = "Navi";
const COPILOT_USER_AGENT: &str = "navi/0.1.0";
const COPILOT_INTENT: &str = "conversation-edits";
const COPILOT_INITIATOR: &str = "user";
const COMMANDCODE_CLI_VERSION: &str = "0.38.2";

/// Endpoint category — used to select auth headers per provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Endpoint {
    Responses,
    ChatCompletions,
    AnthropicMessages,
    Models,
}

/// Normalized usage data extracted from a provider-specific response.
#[derive(Debug, Clone, Default)]
pub(crate) struct NormalizedUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Tokens written to prompt cache (Anthropic).
    pub cache_creation_tokens: Option<u64>,
    /// Tokens read from prompt cache (Anthropic).
    pub cache_read_tokens: Option<u64>,
}

/// Per-provider behavior: auth, routing, headers, URL construction, usage parsing.
pub(crate) trait ProviderBehavior: Send + Sync {
    /// Default base URL when none is configured. None means the provider requires base_url in config.
    fn default_base_url(&self) -> Option<&str>;

    /// Route a model name to a stream API variant.
    fn stream_route(&self, model: &str, configured_kind: OpenAiApiKind) -> StreamRoute;

    /// Build auth + extra headers for a request to the given endpoint.
    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError>;

    /// Extract normalized usage from a provider-specific usage JSON object.
    ///
    /// Default implementation handles OpenAI Responses (`input_tokens`/`output_tokens`)
    /// and Chat Completions (`prompt_tokens`/`completion_tokens`) field names.
    /// Also extracts cached tokens from OpenAI's `input_tokens_details.cached_tokens`
    /// or `prompt_tokens_details.cached_tokens`.
    /// Providers with different field names should override this.
    fn parse_usage(&self, usage: &serde_json::Value) -> NormalizedUsage {
        let input_tokens = usage
            .get("input_tokens")
            .or_else(|| usage.get("prompt_tokens"))
            .and_then(serde_json::Value::as_u64);
        let output_tokens = usage
            .get("output_tokens")
            .or_else(|| usage.get("completion_tokens"))
            .and_then(serde_json::Value::as_u64);
        // OpenAI reports cached tokens in nested details objects:
        // Responses: usage.input_tokens_details.cached_tokens
        // Chat Completions: usage.prompt_tokens_details.cached_tokens
        let cache_read_tokens = usage
            .get("input_tokens_details")
            .or_else(|| usage.get("prompt_tokens_details"))
            .and_then(|details| details.get("cached_tokens"))
            .and_then(serde_json::Value::as_u64);
        NormalizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens: None,
            cache_read_tokens,
        }
    }
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

    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-key", HeaderValue::from_str(api_key)?);
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        if matches!(endpoint, Endpoint::Models) {
            headers.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {}", api_key))?,
            );
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }

    fn parse_usage(&self, usage: &serde_json::Value) -> NormalizedUsage {
        let input_tokens = usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64);
        let output_tokens = usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64);
        let cache_creation_tokens = usage
            .get("cache_creation_input_tokens")
            .and_then(serde_json::Value::as_u64);
        let cache_read_tokens = usage
            .get("cache_read_input_tokens")
            .and_then(serde_json::Value::as_u64);
        NormalizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        }
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

    fn parse_usage(&self, usage: &serde_json::Value) -> NormalizedUsage {
        let input_tokens = usage
            .get("promptTokenCount")
            .and_then(serde_json::Value::as_u64);
        let output_tokens = usage
            .get("candidatesTokenCount")
            .and_then(serde_json::Value::as_u64);
        NormalizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }
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

// ─── Command Code ─────────────────────────────────────────────────────────────

pub(crate) struct CommandCodeBehavior;

impl ProviderBehavior for CommandCodeBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(COMMANDCODE_BASE_URL)
    }

    fn stream_route(&self, _model: &str, _configured_kind: OpenAiApiKind) -> StreamRoute {
        StreamRoute::CommandCodeAlphaGenerate
    }

    fn build_headers(
        &self,
        api_key: &str,
        _endpoint: Endpoint,
    ) -> Result<HeaderMap, ProviderError> {
        let mut headers = standard_bearer_headers(api_key, true)?;
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("command-code/0.38.2 navi"),
        );
        headers.insert(
            "x-command-code-version",
            HeaderValue::from_static(COMMANDCODE_CLI_VERSION),
        );
        Ok(headers)
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
    match provider_id.as_str() {
        ProviderId::OPENAI => Box::new(OpenAiBehavior),
        ProviderId::ANTHROPIC
        | ProviderId::MIMO_ANTHROPIC_CN
        | ProviderId::MIMO_ANTHROPIC_SGP
        | ProviderId::MIMO_ANTHROPIC_AMS => Box::new(AnthropicBehavior),
        ProviderId::GOOGLE_GEMINI => Box::new(GeminiBehavior),
        ProviderId::OPENROUTER => Box::new(OpenRouterBehavior),
        ProviderId::GITHUB_COPILOT => Box::new(GitHubCopilotBehavior),
        ProviderId::OPENCODE => Box::new(OpencodeBehavior),
        ProviderId::OPENCODE_ZEN => Box::new(OpencodeZenBehavior),
        ProviderId::OPENCODE_GO => Box::new(OpencodeGoBehavior),
        ProviderId::COMMANDCODE => Box::new(CommandCodeBehavior),
        ProviderId::GROQ => Box::new(GroqBehavior),
        ProviderId::XAI => Box::new(XaiBehavior),
        _ => Box::new(CustomBehavior),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commandcode_routes_all_models_to_alpha_generate() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::COMMANDCODE));

        assert!(matches!(
            behavior.stream_route("claude-sonnet-4-6", OpenAiApiKind::ChatCompletions),
            StreamRoute::CommandCodeAlphaGenerate
        ));
        assert!(matches!(
            behavior.stream_route("xiaomi/mimo-v2.5-pro", OpenAiApiKind::ChatCompletions),
            StreamRoute::CommandCodeAlphaGenerate
        ));
        assert!(matches!(
            behavior.stream_route("deepseek/deepseek-v4-flash", OpenAiApiKind::ChatCompletions),
            StreamRoute::CommandCodeAlphaGenerate
        ));
    }
}
