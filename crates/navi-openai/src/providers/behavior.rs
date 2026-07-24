use crate::errors::ProviderError;
use crate::types::{OpenAiApiKind, StreamRoute};
use navi_core::{ApiMeta, ModelRequest, ModelStreamEvent, ProviderId, RateLimits};
use reqwest::header::USER_AGENT;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use sha2::{Digest, Sha256};

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

/// Normalized usage data extracted from a provider-specific response.
#[derive(Debug, Clone, Default)]
pub(crate) struct NormalizedUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Tokens written to prompt cache (Anthropic).
    pub cache_creation_tokens: Option<u64>,
    /// Tokens read from prompt cache (Anthropic / some OpenAI-compat aggregators).
    pub cache_read_tokens: Option<u64>,
}

/// Parse a JSON number/string as u64. Aggregators often emit floats (`430.0`).
pub(crate) fn json_u64(value: Option<&serde_json::Value>) -> Option<u64> {
    value.and_then(json_u64_value)
}

pub(crate) fn json_u64_value(value: &serde_json::Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    if let Some(n) = value.as_i64() {
        return u64::try_from(n).ok();
    }
    if let Some(f) = value.as_f64() {
        if f.is_finite() && f >= 0.0 {
            return Some(f.round() as u64);
        }
    }
    value.as_str().and_then(|s| s.trim().parse().ok())
}

/// Per-provider behavior: auth, routing, headers, URL construction, usage parsing.
pub(crate) trait ProviderBehavior: Send + Sync {
    /// Default base URL when none is configured. None means the provider requires base_url in config.
    fn default_base_url(&self) -> Option<&str>;

    /// Route a model name to a stream API variant.
    fn stream_route(&self, model: &str, configured_kind: OpenAiApiKind) -> StreamRoute;

    /// Build auth + extra headers for a request to the given endpoint.
    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError>;

    /// Apply per-request headers (session affinity, etc.) after [`build_headers`].
    ///
    /// Default is a no-op. Charm Hyper sets `x-session-id` /
    /// `x-session-affinity` so turns of the same NAVI session share KV-cache.
    fn apply_request_headers(
        &self,
        _headers: &mut HeaderMap,
        _request: &ModelRequest,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Extract API metadata and rate-limit headers from a successful response.
    ///
    /// Default implementation understands OpenAI-compatible response headers
    /// (`x-request-id`, `openai-organization`, `openai-processing-ms`,
    /// `openai-version`, `x-ratelimit-*`, `x-ratelimit-*-project-tokens`).
    /// Providers that emit differently-named headers can override this.
    fn parse_response_headers(&self, headers: &HeaderMap) -> Option<ModelStreamEvent> {
        parse_openai_response_headers(headers)
    }

    /// Extract normalized usage from a provider-specific usage JSON object.
    ///
    /// Default implementation handles OpenAI Responses (`input_tokens`/`output_tokens`)
    /// and Chat Completions (`prompt_tokens`/`completion_tokens`) field names.
    /// Also extracts cached tokens from OpenAI's `input_tokens_details.cached_tokens`
    /// or `prompt_tokens_details.cached_tokens`.
    /// Providers with different field names should override this.
    fn parse_usage(&self, usage: &serde_json::Value) -> NormalizedUsage {
        let mut input_tokens = json_u64(
            usage
                .get("input_tokens")
                .or_else(|| usage.get("prompt_tokens"))
                .or_else(|| usage.get("promptTokenCount")),
        );
        let output_tokens = json_u64(
            usage
                .get("output_tokens")
                .or_else(|| usage.get("completion_tokens"))
                .or_else(|| usage.get("candidatesTokenCount")),
        );
        // OpenAI reports cached tokens in nested details objects:
        // Responses: usage.input_tokens_details.cached_tokens
        // Chat Completions: usage.prompt_tokens_details.cached_tokens
        // OpenCode Zen: usage.prompt_cache_hit_tokens / prompt_cache_miss_tokens
        let cache_read_tokens = usage
            .get("input_tokens_details")
            .or_else(|| usage.get("prompt_tokens_details"))
            .and_then(|details| details.get("cached_tokens"))
            .and_then(json_u64_value)
            .or_else(|| json_u64(usage.get("cache_read_input_tokens")))
            .or_else(|| json_u64(usage.get("prompt_cache_hit_tokens")));
        let cache_creation_tokens = json_u64(usage.get("cache_creation_input_tokens"))
            .or_else(|| json_u64(usage.get("prompt_cache_miss_tokens")));

        // Some aggregators only put a usable total in `total_tokens`.
        if input_tokens.is_none() {
            if let Some(total) = json_u64(usage.get("total_tokens")) {
                let out = output_tokens.unwrap_or(0);
                input_tokens = Some(total.saturating_sub(out));
            }
        }

        NormalizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        }
    }

    /// Whether this provider endpoint accepts OpenAI-compatible `parallel_tool_calls`.
    fn supports_parallel_tool_calls(&self, _endpoint: Endpoint) -> bool {
        false
    }
}

/// Read a response header as a UTF-8 string.
fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(String::from)
}

/// Parse OpenAI-compatible response headers into a normalized [`ApiMeta`] event.
///
/// Returns `None` if none of the recognized headers are present, so providers
/// that do not emit these headers stay silent.
pub(crate) fn parse_openai_response_headers(headers: &HeaderMap) -> Option<ModelStreamEvent> {
    let request_id = header_str(headers, "x-request-id");
    let organization = header_str(headers, "openai-organization");
    let processing_ms = headers
        .get("openai-processing-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok());
    let api_version = header_str(headers, "openai-version");

    let mut rate_limits = RateLimits::default();
    rate_limits.limit_requests = header_str(headers, "x-ratelimit-limit-requests");
    rate_limits.limit_tokens = header_str(headers, "x-ratelimit-limit-tokens");
    rate_limits.remaining_requests = header_str(headers, "x-ratelimit-remaining-requests");
    rate_limits.remaining_tokens = header_str(headers, "x-ratelimit-remaining-tokens");
    rate_limits.reset_requests = header_str(headers, "x-ratelimit-reset-requests");
    rate_limits.reset_tokens = header_str(headers, "x-ratelimit-reset-tokens");
    rate_limits.limit_project_tokens = header_str(headers, "x-ratelimit-limit-project-tokens");
    rate_limits.remaining_project_tokens =
        header_str(headers, "x-ratelimit-remaining-project-tokens");
    rate_limits.reset_project_tokens = header_str(headers, "x-ratelimit-reset-project-tokens");

    let has_meta = request_id.is_some()
        || organization.is_some()
        || processing_ms.is_some()
        || api_version.is_some();
    let has_rate_limits = rate_limits.limit_requests.is_some()
        || rate_limits.limit_tokens.is_some()
        || rate_limits.remaining_requests.is_some()
        || rate_limits.remaining_tokens.is_some()
        || rate_limits.reset_requests.is_some()
        || rate_limits.reset_tokens.is_some()
        || rate_limits.limit_project_tokens.is_some()
        || rate_limits.remaining_project_tokens.is_some()
        || rate_limits.reset_project_tokens.is_some();

    if !has_meta && !has_rate_limits {
        return None;
    }

    Some(ModelStreamEvent::ApiMeta(ApiMeta {
        request_id,
        organization,
        processing_ms,
        api_version,
        rate_limits,
    }))
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
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
        NormalizedUsage {
            input_tokens: json_u64(usage.get("input_tokens")),
            output_tokens: json_u64(usage.get("output_tokens")),
            cache_creation_tokens: json_u64(usage.get("cache_creation_input_tokens")),
            cache_read_tokens: json_u64(usage.get("cache_read_input_tokens")),
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
        let input_tokens = json_u64(usage.get("promptTokenCount"));
        let output_tokens = json_u64(usage.get("candidatesTokenCount"));
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
    }
}

// ─── Command Code ─────────────────────────────────────────────────────────────

pub(crate) struct CommandCodeBehavior;

const COMMANDCODE_BASE_URL: &str = "https://api.commandcode.ai/provider/v1";

impl ProviderBehavior for CommandCodeBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(COMMANDCODE_BASE_URL)
    }

    fn stream_route(&self, model: &str, _configured_kind: OpenAiApiKind) -> StreamRoute {
        // Command Code's Provider API exposes an OpenAI-compatible /chat/completions
        // endpoint and an Anthropic-compatible /messages endpoint. Claude-family
        // models must use /messages; everything else uses /chat/completions.
        if model.to_ascii_lowercase().starts_with("claude-") {
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(
            endpoint,
            Endpoint::ChatCompletions | Endpoint::AnthropicMessages
        )
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

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
    }
}

// ─── Xai ──────────────────────────────────────────────────────────────────────

/// Header that tells xAI auth middleware to treat the Bearer as a Grok CLI
/// OAuth session token (not a Platform API key).
const XAI_TOKEN_AUTH_HEADER: &str = "X-XAI-Token-Auth";
const XAI_TOKEN_AUTH_VALUE: &str = "xai-grok-cli";
const XAI_CLIENT_VERSION_HEADER: &str = "x-grok-client-version";
const XAI_CLIENT_IDENTIFIER_HEADER: &str = "x-grok-client-identifier";
const XAI_CLIENT_MODE_HEADER: &str = "x-grok-client-mode";
const XAI_CLIENT_SURFACE_HEADER: &str = "x-grok-client-surface";
const XAI_AGENT_ID_HEADER: &str = "x-grok-agent-id";
const XAI_MODEL_OVERRIDE_HEADER: &str = "x-grok-model-override";
const XAI_SESSION_ID_HEADER: &str = "x-grok-session-id";
const XAI_CONV_ID_HEADER: &str = "x-grok-conv-id";
const XAI_REQ_ID_HEADER: &str = "x-grok-req-id";
const XAI_TURN_ID_HEADER: &str = "x-grok-turn-id";

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
        let mut headers = standard_bearer_headers(api_key, true)?;
        // OAuth access JWTs need Grok CLI / Grok Build headers so the proxy
        // bills the subscription plan (not Platform API). Without
        // `x-grok-client-version`, cli-chat-proxy returns HTTP 426.
        // Platform keys start with `xai-` and use normal Bearer only.
        if crate::oauth::is_xai_oauth_access_token(api_key) {
            insert_xai_cli_identity_headers(&mut headers)?;
        }
        Ok(headers)
    }

    fn apply_request_headers(
        &self,
        headers: &mut HeaderMap,
        request: &ModelRequest,
    ) -> Result<(), ProviderError> {
        // Only decorate Grok CLI / subscription proxy traffic. Platform keys
        // (or non-CLI headers) skip model routing + session correlation.
        if headers.get(XAI_TOKEN_AUTH_HEADER).is_none() {
            return Ok(());
        }

        // Required by cli-chat-proxy to route the correct inference cluster.
        // Official docs: proxy uses this header, not only the JSON body model.
        insert_header(
            headers,
            XAI_MODEL_OVERRIDE_HEADER,
            &request.model,
            "x-grok-model-override",
        )?;

        // Per-conversation stickiness + request correlation (Grok sampler
        // headers). Auxiliary model calls have no parent NAVI session, so they
        // get a fresh id instead of sharing a process-wide `navi` bucket.
        let req_id = crate::oauth::xai_new_request_id();
        let session = request
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&req_id);
        insert_header(headers, XAI_SESSION_ID_HEADER, session, "x-grok-session-id")?;
        insert_header(headers, XAI_CONV_ID_HEADER, session, "x-grok-conv-id")?;
        insert_header(headers, XAI_TURN_ID_HEADER, session, "x-grok-turn-id")?;

        insert_header(headers, XAI_REQ_ID_HEADER, &req_id, "x-grok-req-id")?;
        Ok(())
    }

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
    }
}

fn insert_xai_cli_identity_headers(headers: &mut HeaderMap) -> Result<(), ProviderError> {
    headers.insert(
        XAI_TOKEN_AUTH_HEADER,
        HeaderValue::from_static(XAI_TOKEN_AUTH_VALUE),
    );

    let version = crate::oauth::xai_grok_cli_client_version();
    insert_header(
        headers,
        XAI_CLIENT_VERSION_HEADER,
        &version,
        "x-grok-client-version",
    )?;

    // Match official grok User-Agent fingerprint (`grok/<version>`).
    let ua = format!("grok/{version}");
    insert_header(headers, USER_AGENT.as_str(), &ua, "User-Agent")?;

    headers.insert(
        XAI_CLIENT_MODE_HEADER,
        HeaderValue::from_static(crate::oauth::XAI_GROK_CLI_CLIENT_MODE),
    );
    headers.insert(
        XAI_CLIENT_SURFACE_HEADER,
        HeaderValue::from_static(crate::oauth::XAI_GROK_CLI_CLIENT_SURFACE),
    );

    let client_id = crate::oauth::xai_client_identifier();
    insert_header(
        headers,
        XAI_CLIENT_IDENTIFIER_HEADER,
        &client_id,
        "x-grok-client-identifier",
    )?;

    let agent_id = crate::oauth::xai_agent_id();
    insert_header(headers, XAI_AGENT_ID_HEADER, &agent_id, "x-grok-agent-id")?;
    Ok(())
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &str,
    value: &str,
    label: &str,
) -> Result<(), ProviderError> {
    let header_name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|err| ProviderError::Other(format!("invalid {label} header name: {err}")))?;
    let hv = HeaderValue::from_str(value)
        .map_err(|err| ProviderError::Other(format!("invalid {label} header value: {err}")))?;
    headers.insert(header_name, hv);
    Ok(())
}

// ─── Charm Hyper ──────────────────────────────────────────────────────────────

/// Charm Hyper (`https://hyper.charm.land/v1`) — OpenAI-compatible chat
/// completions with session-affinity headers for provider-side prompt cache.
///
/// Matches Crush: both `x-session-id` and `x-session-affinity` carry the same
/// deterministic opaque token derived from the NAVI session id.
pub(crate) struct CharmHyperBehavior;

const HYPER_BASE_URL: &str = "https://hyper.charm.land/v1";

/// Opaque, process-stable token for Hyper session affinity.
///
/// Crush uses XXH3 of the session UUID. We use SHA-256 of the session id and
/// emit the first 16 bytes as hex so the value is deterministic across NAVI
/// restarts (unlike `DefaultHasher`, which is not stable across processes).
pub(crate) fn hyper_session_affinity_token(session_id: &str) -> String {
    let digest = Sha256::digest(session_id.as_bytes());
    let mut out = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn insert_hyper_session_headers(
    headers: &mut HeaderMap,
    session_id: &str,
) -> Result<(), ProviderError> {
    let token = hyper_session_affinity_token(session_id);
    let value = HeaderValue::from_str(&token).map_err(|err| {
        ProviderError::Other(format!(
            "invalid charm-hyper session affinity header: {err}"
        ))
    })?;
    headers.insert("x-session-id", value.clone());
    headers.insert("x-session-affinity", value);
    Ok(())
}

impl ProviderBehavior for CharmHyperBehavior {
    fn default_base_url(&self) -> Option<&str> {
        Some(HYPER_BASE_URL)
    }

    fn stream_route(&self, _model: &str, configured_kind: OpenAiApiKind) -> StreamRoute {
        // Registry pins charm-hyper to chat-completions; honor config if Responses is set.
        match configured_kind {
            OpenAiApiKind::Responses => StreamRoute::Responses,
            OpenAiApiKind::ChatCompletions => StreamRoute::ChatCompletions,
        }
    }

    fn build_headers(&self, api_key: &str, endpoint: Endpoint) -> Result<HeaderMap, ProviderError> {
        let content_type = matches!(
            endpoint,
            Endpoint::ChatCompletions | Endpoint::Responses | Endpoint::AnthropicMessages
        );
        standard_bearer_headers(api_key, content_type)
    }

    fn apply_request_headers(
        &self,
        headers: &mut HeaderMap,
        request: &ModelRequest,
    ) -> Result<(), ProviderError> {
        if let Some(session_id) = request.session_id.as_deref().filter(|s| !s.is_empty()) {
            insert_hyper_session_headers(headers, session_id)?;
        }
        Ok(())
    }

    fn supports_parallel_tool_calls(&self, endpoint: Endpoint) -> bool {
        matches!(endpoint, Endpoint::Responses | Endpoint::ChatCompletions)
    }
}

// ─── Nvidia NIM ──────────────────────────────────────────────────────────────

pub(crate) struct NvidiaBehavior;

impl ProviderBehavior for NvidiaBehavior {
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
        ProviderId::CHARM_HYPER => Box::new(CharmHyperBehavior),
        ProviderId::GROQ => Box::new(GroqBehavior),
        ProviderId::XAI => Box::new(XaiBehavior),
        ProviderId::NVIDIA => Box::new(NvidiaBehavior),
        _ => Box::new(CustomBehavior),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn xai_oauth_jwt_adds_cli_token_auth_header() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::XAI));
        let headers = behavior
            .build_headers(
                "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
                Endpoint::Responses,
            )
            .expect("headers");
        assert_eq!(
            headers
                .get("X-XAI-Token-Auth")
                .and_then(|v| v.to_str().ok()),
            Some("xai-grok-cli")
        );
        let version = crate::oauth::xai_grok_cli_client_version();
        assert_eq!(
            headers
                .get("x-grok-client-version")
                .and_then(|v| v.to_str().ok()),
            Some(version.as_str())
        );
        assert_eq!(
            headers
                .get("x-grok-client-mode")
                .and_then(|v| v.to_str().ok()),
            Some(crate::oauth::XAI_GROK_CLI_CLIENT_MODE)
        );
        assert_eq!(
            headers
                .get("x-grok-client-surface")
                .and_then(|v| v.to_str().ok()),
            Some(crate::oauth::XAI_GROK_CLI_CLIENT_SURFACE)
        );
        let client_id = crate::oauth::xai_client_identifier();
        let agent_id = crate::oauth::xai_agent_id();
        let ua = format!("grok/{version}");
        assert_eq!(
            headers
                .get("x-grok-client-identifier")
                .and_then(|v| v.to_str().ok()),
            Some(client_id.as_str())
        );
        assert_eq!(
            headers.get("x-grok-agent-id").and_then(|v| v.to_str().ok()),
            Some(agent_id.as_str())
        );
        assert_eq!(
            headers.get(USER_AGENT).and_then(|v| v.to_str().ok()),
            Some(ua.as_str())
        );
        assert!(
            headers
                .get(AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("Bearer eyJ"))
        );
    }

    #[test]
    fn xai_oauth_request_headers_set_model_override_and_session_ids() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::XAI));
        let mut headers = behavior
            .build_headers(
                "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
                Endpoint::Responses,
            )
            .expect("headers");
        let request = ModelRequest {
            model: "grok-4.5".into(),
            instructions: None,
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
            session_id: Some("sess-xyz".into()),
        };
        behavior
            .apply_request_headers(&mut headers, &request)
            .expect("request headers");

        assert_eq!(
            headers
                .get("x-grok-model-override")
                .and_then(|v| v.to_str().ok()),
            Some("grok-4.5")
        );
        assert_eq!(
            headers
                .get("x-grok-session-id")
                .and_then(|v| v.to_str().ok()),
            Some("sess-xyz")
        );
        assert_eq!(
            headers.get("x-grok-conv-id").and_then(|v| v.to_str().ok()),
            Some("sess-xyz")
        );
        assert_eq!(
            headers.get("x-grok-turn-id").and_then(|v| v.to_str().ok()),
            Some("sess-xyz")
        );
        assert!(
            headers
                .get("x-grok-req-id")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| !v.is_empty())
        );
    }

    #[test]
    fn xai_oauth_background_requests_do_not_share_fallback_session() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::XAI));
        let request = ModelRequest {
            model: "grok-4.5".into(),
            instructions: None,
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
            session_id: None,
        };

        let mut first = behavior
            .build_headers(
                "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
                Endpoint::Responses,
            )
            .expect("first headers");
        behavior
            .apply_request_headers(&mut first, &request)
            .expect("first request headers");

        let mut second = behavior
            .build_headers(
                "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
                Endpoint::Responses,
            )
            .expect("second headers");
        behavior
            .apply_request_headers(&mut second, &request)
            .expect("second request headers");

        let first_session = first
            .get("x-grok-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("first session id");
        let second_session = second
            .get("x-grok-session-id")
            .and_then(|value| value.to_str().ok())
            .expect("second session id");

        assert_ne!(first_session, "navi");
        assert_ne!(first_session, second_session);
        assert_eq!(
            first
                .get("x-grok-conv-id")
                .and_then(|value| value.to_str().ok()),
            Some(first_session)
        );
    }

    #[test]
    fn xai_platform_key_skips_cli_token_auth_header() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::XAI));
        let mut headers = behavior
            .build_headers("xai-platform-key-abc", Endpoint::Responses)
            .expect("headers");
        assert!(headers.get("X-XAI-Token-Auth").is_none());
        assert!(headers.get("x-grok-client-version").is_none());

        // Platform keys must not get Grok routing headers either.
        let request = ModelRequest {
            model: "grok-4.5".into(),
            instructions: None,
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
            session_id: Some("sess".into()),
        };
        behavior
            .apply_request_headers(&mut headers, &request)
            .expect("request headers");
        assert!(headers.get("x-grok-model-override").is_none());
        assert!(headers.get("x-grok-session-id").is_none());
    }

    #[test]
    fn commandcode_routes_claude_to_messages_and_openai_models_to_chat_completions() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::COMMANDCODE));

        assert!(matches!(
            behavior.stream_route("claude-sonnet-4-6", OpenAiApiKind::ChatCompletions),
            StreamRoute::AnthropicMessages
        ));
        assert!(matches!(
            behavior.stream_route("xiaomi/mimo-v2.5-pro", OpenAiApiKind::ChatCompletions),
            StreamRoute::ChatCompletions
        ));
        assert!(matches!(
            behavior.stream_route("deepseek/deepseek-v4-flash", OpenAiApiKind::ChatCompletions),
            StreamRoute::ChatCompletions
        ));
    }

    #[test]
    fn charm_hyper_sets_stable_session_affinity_headers() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::CHARM_HYPER));
        let mut headers = behavior
            .build_headers("hk-test", Endpoint::ChatCompletions)
            .expect("headers");
        let request = ModelRequest {
            model: "kimi-k2.5".into(),
            instructions: None,
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
            session_id: Some("session-abc-123".into()),
        };
        behavior
            .apply_request_headers(&mut headers, &request)
            .expect("affinity");

        let expected = hyper_session_affinity_token("session-abc-123");
        assert_eq!(
            headers.get("x-session-id").and_then(|v| v.to_str().ok()),
            Some(expected.as_str())
        );
        assert_eq!(
            headers
                .get("x-session-affinity")
                .and_then(|v| v.to_str().ok()),
            Some(expected.as_str())
        );

        // Same session → same token (required for KV-cache stickiness).
        assert_eq!(
            hyper_session_affinity_token("session-abc-123"),
            hyper_session_affinity_token("session-abc-123")
        );
        // Different session → different token.
        assert_ne!(
            hyper_session_affinity_token("session-abc-123"),
            hyper_session_affinity_token("session-other")
        );
        // Token is process-stable (SHA-256 prefix), not DefaultHasher.
        assert_eq!(hyper_session_affinity_token("session-abc-123").len(), 32);
        assert!(
            hyper_session_affinity_token("session-abc-123")
                .chars()
                .all(|c| c.is_ascii_hexdigit())
        );
    }

    #[test]
    fn charm_hyper_skips_affinity_without_session_id() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::CHARM_HYPER));
        let mut headers = behavior
            .build_headers("hk-test", Endpoint::ChatCompletions)
            .expect("headers");
        let request = ModelRequest {
            model: "kimi-k2.5".into(),
            instructions: None,
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
            session_id: None,
        };
        behavior
            .apply_request_headers(&mut headers, &request)
            .expect("affinity");
        assert!(headers.get("x-session-id").is_none());
        assert!(headers.get("x-session-affinity").is_none());
    }

    #[test]
    fn json_u64_value_handles_numeric_variants() {
        assert_eq!(json_u64_value(&json!(42)), Some(42));
        assert_eq!(json_u64_value(&json!(-1)), None);
        assert_eq!(json_u64_value(&json!(1.7)), Some(2));
        assert_eq!(json_u64_value(&json!(-1.5)), None);
        assert_eq!(json_u64_value(&json!("3")), Some(3));
        assert_eq!(json_u64_value(&json!("not a number")), None);
        assert_eq!(json_u64(None), None);
    }

    #[test]
    fn parse_usage_extracts_openai_chat_and_gemini_fields() {
        let openai = behavior_for_provider(&ProviderId::from_config_id(ProviderId::OPENAI));
        let usage = openai.parse_usage(&json!({
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 20,
            "prompt_tokens_details": { "cached_tokens": 2 }
        }));
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.cache_read_tokens, Some(2));

        // Falls back to total_tokens when input not present.
        let usage = openai.parse_usage(&json!({
            "output_tokens": 5,
            "total_tokens": 20
        }));
        assert_eq!(usage.input_tokens, Some(15));

        let gemini = behavior_for_provider(&ProviderId::from_config_id(ProviderId::GOOGLE_GEMINI));
        let usage = gemini.parse_usage(&json!({
            "promptTokenCount": 7,
            "candidatesTokenCount": 3
        }));
        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(3));
    }

    #[test]
    fn all_provider_behaviors_build_headers_and_route() {
        let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
            (
                ProviderId::OPENAI,
                Some(OPENAI_BASE_URL),
                vec!["authorization"],
            ),
            (
                ProviderId::ANTHROPIC,
                None,
                vec!["x-api-key", "anthropic-version"],
            ),
            (ProviderId::GOOGLE_GEMINI, Some(GEMINI_BASE_URL), vec![]),
            (
                ProviderId::OPENROUTER,
                None,
                vec!["authorization", "HTTP-Referer", "X-Title"],
            ),
            (
                ProviderId::GITHUB_COPILOT,
                None,
                vec![
                    "authorization",
                    "user-agent",
                    "Openai-Intent",
                    "x-initiator",
                ],
            ),
            (
                ProviderId::OPENCODE,
                Some(OPENCODE_ZEN_BASE_URL),
                vec!["authorization"],
            ),
            (
                ProviderId::OPENCODE_ZEN,
                Some(OPENCODE_ZEN_BASE_URL),
                vec!["authorization"],
            ),
            (
                ProviderId::OPENCODE_GO,
                Some(OPENCODE_GO_BASE_URL),
                vec!["authorization"],
            ),
            (
                ProviderId::COMMANDCODE,
                Some(COMMANDCODE_BASE_URL),
                vec!["authorization"],
            ),
            (ProviderId::GROQ, None, vec!["authorization"]),
            (
                ProviderId::CHARM_HYPER,
                Some(HYPER_BASE_URL),
                vec!["authorization"],
            ),
            (ProviderId::NVIDIA, None, vec!["authorization"]),
            // Mimo Anthropic aliases route to AnthropicBehavior.
            (
                ProviderId::MIMO_ANTHROPIC_CN,
                None,
                vec!["x-api-key", "anthropic-version"],
            ),
            (
                ProviderId::MIMO_ANTHROPIC_SGP,
                None,
                vec!["x-api-key", "anthropic-version"],
            ),
            (
                ProviderId::MIMO_ANTHROPIC_AMS,
                None,
                vec!["x-api-key", "anthropic-version"],
            ),
        ];

        for (id, expected_base, expected_headers) in cases {
            let behavior = behavior_for_provider(&ProviderId::from_config_id(id));
            assert_eq!(
                behavior.default_base_url(),
                expected_base,
                "{id} base url mismatch"
            );
            let _ = behavior.stream_route("test-model", OpenAiApiKind::ChatCompletions);
            let headers = behavior
                .build_headers("test-key", Endpoint::ChatCompletions)
                .unwrap();
            for header in expected_headers {
                assert!(headers.contains_key(header), "{id} missing {header}");
            }
        }

        // Custom fallback for unregistered provider ids.
        let custom = behavior_for_provider(&ProviderId::from_config_id("unknown-provider"));
        assert!(custom.default_base_url().is_none());
        assert!(matches!(
            custom.stream_route("x", OpenAiApiKind::ChatCompletions),
            StreamRoute::ChatCompletions
        ));
        assert!(custom.build_headers("k", Endpoint::ChatCompletions).is_ok());
    }

    #[test]
    fn opencode_routes_by_model_family() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::OPENCODE));
        assert!(matches!(
            behavior.stream_route("opencode/gpt-foo", OpenAiApiKind::ChatCompletions),
            StreamRoute::Responses
        ));
        assert!(matches!(
            behavior.stream_route("opencode/claude-sonnet", OpenAiApiKind::ChatCompletions),
            StreamRoute::AnthropicMessages
        ));
        assert!(matches!(
            behavior.stream_route("opencode/other", OpenAiApiKind::ChatCompletions),
            StreamRoute::ChatCompletions
        ));
    }

    #[test]
    fn openai_headers_vary_by_endpoint() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::OPENAI));
        let chat = behavior
            .build_headers("k", Endpoint::ChatCompletions)
            .unwrap();
        assert!(chat.contains_key("content-type"));
        let models = behavior.build_headers("k", Endpoint::Models).unwrap();
        assert!(!models.contains_key("content-type"));
        assert!(behavior.supports_parallel_tool_calls(Endpoint::ChatCompletions));
        assert!(!behavior.supports_parallel_tool_calls(Endpoint::Models));
    }

    #[test]
    fn anthropic_models_endpoint_adds_bearer_authorization() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::ANTHROPIC));
        let headers = behavior.build_headers("k", Endpoint::Models).unwrap();
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer k")
        );
    }

    #[test]
    fn commandcode_supports_parallel_tool_calls_for_openai_and_anthropic() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::COMMANDCODE));
        assert!(behavior.supports_parallel_tool_calls(Endpoint::ChatCompletions));
        assert!(behavior.supports_parallel_tool_calls(Endpoint::AnthropicMessages));
        assert!(!behavior.supports_parallel_tool_calls(Endpoint::Models));
    }

    #[test]
    fn charm_hyper_responses_endpoint_has_content_type() {
        let behavior = behavior_for_provider(&ProviderId::from_config_id(ProviderId::CHARM_HYPER));
        let headers = behavior.build_headers("k", Endpoint::Responses).unwrap();
        assert!(headers.contains_key("content-type"));
    }

    // ── Response header parsing ───────────────────────────────────────────────

    #[test]
    fn parse_openai_response_headers_emits_api_meta() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req_123"));
        headers.insert("openai-organization", HeaderValue::from_static("org_456"));
        headers.insert("openai-processing-ms", HeaderValue::from_static("42"));
        headers.insert("openai-version", HeaderValue::from_static("2020-10-01"));
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "x-ratelimit-limit-tokens",
            HeaderValue::from_static("60000"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("99"),
        );
        headers.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("59999"),
        );
        headers.insert(
            "x-ratelimit-reset-requests",
            HeaderValue::from_static("2.467s"),
        );
        headers.insert(
            "x-ratelimit-reset-tokens",
            HeaderValue::from_static("47m40s"),
        );

        let event = parse_openai_response_headers(&headers).expect("expected ApiMeta event");
        let ModelStreamEvent::ApiMeta(meta) = event else {
            panic!("expected ApiMeta, got {event:?}");
        };

        assert_eq!(meta.request_id, Some("req_123".into()));
        assert_eq!(meta.organization, Some("org_456".into()));
        assert_eq!(meta.processing_ms, Some(42));
        assert_eq!(meta.api_version, Some("2020-10-01".into()));
        assert_eq!(meta.rate_limits.limit_requests, Some("100".into()));
        assert_eq!(meta.rate_limits.limit_tokens, Some("60000".into()));
        assert_eq!(meta.rate_limits.remaining_requests, Some("99".into()));
        assert_eq!(meta.rate_limits.remaining_tokens, Some("59999".into()));
        assert_eq!(meta.rate_limits.reset_requests, Some("2.467s".into()));
        assert_eq!(meta.rate_limits.reset_tokens, Some("47m40s".into()));
    }

    #[test]
    fn parse_openai_response_headers_project_rate_limits() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-project-tokens",
            HeaderValue::from_static("100000"),
        );
        headers.insert(
            "x-ratelimit-remaining-project-tokens",
            HeaderValue::from_static("99999"),
        );
        headers.insert(
            "x-ratelimit-reset-project-tokens",
            HeaderValue::from_static("12m"),
        );

        let event = parse_openai_response_headers(&headers).expect("expected ApiMeta event");
        let ModelStreamEvent::ApiMeta(meta) = event else {
            panic!("expected ApiMeta, got {event:?}");
        };

        assert_eq!(meta.rate_limits.limit_project_tokens, Some("100000".into()));
        assert_eq!(
            meta.rate_limits.remaining_project_tokens,
            Some("99999".into())
        );
        assert_eq!(meta.rate_limits.reset_project_tokens, Some("12m".into()));
        assert!(meta.request_id.is_none());
        assert!(meta.processing_ms.is_none());
    }

    #[test]
    fn parse_openai_response_headers_returns_none_when_empty() {
        let headers = HeaderMap::new();
        assert!(parse_openai_response_headers(&headers).is_none());
    }

    #[test]
    fn parse_openai_response_headers_ignores_invalid_processing_ms() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("req-invalid"));
        headers.insert(
            "openai-processing-ms",
            HeaderValue::from_static("not-a-number"),
        );

        let event = parse_openai_response_headers(&headers).expect("expected ApiMeta event");
        let ModelStreamEvent::ApiMeta(meta) = event else {
            panic!("expected ApiMeta, got {event:?}");
        };
        assert_eq!(meta.request_id, Some("req-invalid".into()));
        assert!(meta.processing_ms.is_none());
    }

    #[test]
    fn provider_behaviors_parse_openai_compatible_response_headers() {
        let ids = vec![
            ProviderId::OPENAI,
            ProviderId::OPENROUTER,
            ProviderId::GROQ,
            ProviderId::OPENCODE,
            ProviderId::OPENCODE_ZEN,
            ProviderId::OPENCODE_GO,
            ProviderId::COMMANDCODE,
            ProviderId::CHARM_HYPER,
            ProviderId::NVIDIA,
        ];

        for id in ids {
            let behavior = behavior_for_provider(&ProviderId::from_config_id(id));
            let mut headers = HeaderMap::new();
            headers.insert("x-request-id", HeaderValue::from_static("req-abc"));
            headers.insert("openai-organization", HeaderValue::from_static("org-xyz"));
            headers.insert("openai-processing-ms", HeaderValue::from_static("15"));
            headers.insert(
                "x-ratelimit-remaining-requests",
                HeaderValue::from_static("8"),
            );

            let event = behavior
                .parse_response_headers(&headers)
                .expect("{id} should parse OpenAI-compatible headers");
            let ModelStreamEvent::ApiMeta(meta) = event else {
                panic!("expected ApiMeta from {id}, got {event:?}");
            };
            assert_eq!(meta.request_id, Some("req-abc".into()), "{id} request_id");
            assert_eq!(
                meta.organization,
                Some("org-xyz".into()),
                "{id} organization"
            );
            assert_eq!(meta.processing_ms, Some(15), "{id} processing_ms");
            assert_eq!(
                meta.rate_limits.remaining_requests,
                Some("8".into()),
                "{id} remaining_requests"
            );
        }
    }

    #[test]
    fn anthropic_and_gemini_return_none_for_unrecognized_headers() {
        let mut headers = HeaderMap::new();
        // Anthropic and Gemini do not emit OpenAI-prefixed headers.
        headers.insert("request-id", HeaderValue::from_static("anthropic-req"));

        for id in [ProviderId::ANTHROPIC, ProviderId::GOOGLE_GEMINI] {
            let behavior = behavior_for_provider(&ProviderId::from_config_id(id));
            assert!(
                behavior.parse_response_headers(&headers).is_none(),
                "{id} should not emit ApiMeta for unrecognized headers"
            );
        }
    }
}
