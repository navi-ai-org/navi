use crate::errors::ProviderError;
use crate::mapping::unique_sorted_model_ids;
use crate::providers::behavior::{Endpoint, ProviderBehavior, behavior_for_provider};
use crate::transport::{
    ensure_success, get_backoff_delay, retry_delay_for_error, should_retry_error,
};
use crate::types::{OpenAiApiKind, StreamRoute};
use anyhow::{Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures_util::StreamExt;
use navi_core::ProviderId;
use navi_core::{
    ModelProvider, ModelRequest, ModelStream, ModelStreamEvent, ProviderConfig, ProviderKind,
};
use reqwest::Client;
use std::time::Duration;

#[derive(Clone)]
pub struct OpenAiProvider {
    pub(crate) client: Client,
    pub(crate) api_key: String,
    pub(crate) base_url: String,
    pub(crate) api_kind: OpenAiApiKind,
    pub(crate) provider_id: String,
    pub(crate) provider_identity: ProviderId,
    pub(crate) behavior: std::sync::Arc<dyn ProviderBehavior>,
    pub(crate) config: ProviderConfig,
}

impl std::fmt::Debug for OpenAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiProvider")
            .field("provider_id", &self.provider_id)
            .field("base_url", &self.base_url)
            .field("api_kind", &self.api_kind)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl OpenAiProvider {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is not set")?;
        Ok(Self::new(api_key))
    }

    pub fn from_provider_config(provider: &ProviderConfig) -> Result<Self> {
        let api_key = std::env::var(&provider.api_key_env)
            .with_context(|| format!("{} is not set", provider.api_key_env))?;
        Self::from_provider_config_with_key(provider, api_key)
    }

    pub fn from_provider_config_with_key(
        provider: &ProviderConfig,
        api_key: String,
    ) -> Result<Self> {
        let identity = ProviderId::from_config_id(&provider.id);
        let behavior = behavior_for_provider(&identity);
        let base_url = match &provider.base_url {
            Some(url) => url.clone(),
            None => behavior
                .default_base_url()
                .ok_or_else(|| {
                    anyhow::anyhow!("provider {} requires base_url in config", provider.id)
                })?
                .to_string(),
        };
        let api_kind = match provider.kind {
            ProviderKind::OpenAiResponses => OpenAiApiKind::Responses,
            ProviderKind::OpenAiChatCompletions => OpenAiApiKind::ChatCompletions,
            ProviderKind::AnthropicMessages => OpenAiApiKind::ChatCompletions,
            ProviderKind::GeminiGenerateContent => OpenAiApiKind::ChatCompletions,
        };

        Ok(Self::new(api_key)
            .with_base_url(base_url)
            .with_api_kind(api_kind)
            .with_provider_id(provider.id.clone())
            .with_identity(identity)
            .with_behavior(behavior)
            .with_config(provider.clone()))
    }

    pub fn new(api_key: String) -> Self {
        let identity = ProviderId::known(ProviderId::OPENAI);
        Self {
            client: Client::new(),
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
            api_kind: OpenAiApiKind::Responses,
            provider_id: "openai".to_string(),
            behavior: std::sync::Arc::from(behavior_for_provider(&identity)),
            provider_identity: identity,
            config: ProviderConfig::default(),
        }
    }

    pub fn with_identity(mut self, identity: ProviderId) -> Self {
        self.provider_identity = identity;
        self
    }

    pub(crate) fn with_behavior(mut self, behavior: Box<dyn ProviderBehavior>) -> Self {
        self.behavior = std::sync::Arc::from(behavior);
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_api_kind(mut self, api_kind: OpenAiApiKind) -> Self {
        self.api_kind = api_kind;
        self
    }

    pub fn with_provider_id(mut self, provider_id: impl Into<String>) -> Self {
        let id = provider_id.into();
        let identity = ProviderId::from_config_id(&id);
        self.behavior = std::sync::Arc::from(behavior_for_provider(&identity));
        self.provider_identity = identity;
        self.provider_id = id;
        self
    }

    pub fn with_config(mut self, config: ProviderConfig) -> Self {
        self.config = config;
        self
    }

    fn stream_inner(&self, request: ModelRequest) -> ModelStream {
        let route = self.behavior.stream_route(&request.model, self.api_kind);
        match route {
            StreamRoute::Responses => self.stream_responses(request),
            StreamRoute::ChatCompletions => self.stream_chat_completions(request),
            StreamRoute::AnthropicMessages => self.stream_anthropic_messages(request),
            StreamRoute::GeminiGenerateContent => self.stream_gemini_generate_content(request),
            StreamRoute::CommandCodeAlphaGenerate => {
                self.stream_commandcode_alpha_generate(request)
            }
        }
    }

    async fn send_with_retry(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ProviderError> {
        let max_attempts = self.config.request_max_retries();
        let retry_429 = self.config.retry_429();
        let mut attempt = 0;

        loop {
            attempt += 1;

            let req = builder.try_clone().ok_or_else(|| {
                ProviderError::Other("failed to clone RequestBuilder".to_string())
            })?;

            let req = req.timeout(Duration::from_millis(self.config.request_timeout_ms()));

            match req.send().await {
                Ok(resp) => match ensure_success(resp).await {
                    Ok(success_resp) => return Ok(success_resp),
                    Err(err) => {
                        if attempt >= max_attempts || !should_retry_error(&err, retry_429) {
                            return Err(err);
                        }

                        let delay = retry_delay_for_error(&err, attempt);

                        tracing::warn!(?delay, attempt, "retrying request after delay");
                        tokio::time::sleep(delay).await;
                    }
                },
                Err(err) => {
                    let err = ProviderError::Transport(err);
                    if attempt >= max_attempts {
                        return Err(err);
                    }

                    let delay = get_backoff_delay(attempt);
                    tracing::warn!(?delay, attempt, "retrying request after transport error");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

/// Builds a request for a mid-stream prefill resume attempt.
///
/// When `accumulated_text` is non-empty, appends an assistant message so the
/// provider continues from the already-generated prefix instead of restarting.
fn request_with_prefill(request: &ModelRequest, accumulated_text: &str) -> ModelRequest {
    let mut req = request.clone();
    if !accumulated_text.is_empty() {
        req.messages
            .push(navi_core::ModelMessage::assistant(accumulated_text.to_string()));
    }
    req
}

/// Whether a mid-stream error can be recovered via prefill resumption.
fn can_resume_with_prefill(
    attempt_text: &str,
    attempt_yielded_tool_call: bool,
    yielded_tool_call: bool,
    attempt: u32,
    max_attempts: u32,
    provider_err: &ProviderError,
    retry_429: bool,
) -> bool {
    !attempt_text.is_empty()
        && !attempt_yielded_tool_call
        && !yielded_tool_call
        && attempt < max_attempts
        && should_retry_error(provider_err, retry_429)
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn stream(&self, request: ModelRequest) -> ModelStream {
        let provider = self.clone();
        Box::pin(try_stream! {
            let max_attempts = provider.config.stream_max_retries();
            let retry_429 = provider.config.retry_429();
            let mut attempt = 0;
            let mut content_yielded = false;
            // Accumulated text from the current attempt — used for prefill
            // resumption when the stream breaks mid-generation.
            let mut accumulated_text = String::new();
            // Whether any tool calls were yielded in the current attempt.
            // If so, we cannot safely resume via prefill because the model
            // may have started a tool invocation whose JSON is incomplete.
            let mut yielded_tool_call = false;

            loop {
                attempt += 1;
                tracing::debug!(attempt, max_attempts, "starting stream attempt");

                // Build the request, optionally with a prefill assistant message
                // containing text accumulated before a mid-stream break.
                let req = request_with_prefill(&request, &accumulated_text);
                let resuming = attempt > 1 && !accumulated_text.is_empty() && !yielded_tool_call;
                if resuming {
                    tracing::info!(
                        chars = accumulated_text.len(),
                        attempt,
                        "resuming stream with prefill",
                    );
                    yield ModelStreamEvent::Status {
                        label: "resuming".to_string(),
                    };
                }

                let mut inner_stream = provider.stream_inner(req);
                let mut failed = false;
                // Track what was accumulated *this* attempt so we can combine
                // it with prior accumulation on retry.
                let mut attempt_text = String::new();
                let mut attempt_yielded_tool_call = false;

                while let Some(item) = inner_stream.next().await {
                    match item {
                        Ok(event) => {
                            match &event {
                                ModelStreamEvent::TextDelta { text } => {
                                    content_yielded = true;
                                    attempt_text.push_str(text);
                                }
                                ModelStreamEvent::ThinkingDelta { .. } => {
                                    content_yielded = true;
                                }
                                ModelStreamEvent::ToolCall(_) => {
                                    content_yielded = true;
                                    attempt_yielded_tool_call = true;
                                }
                                _ => {}
                            }
                            yield event;
                        }
                        Err(err) => {
                            tracing::warn!(?err, attempt, "stream chunk error occurred");

                            if content_yielded {
                                // Mid-stream break. Try prefill resumption if:
                                // - The error is retryable.
                                // - We have accumulated text (not just thinking).
                                // - No tool calls were yielded (incomplete tool
                                //   JSON cannot be safely resumed).
                                let provider_err = if let Some(pe) = err.downcast_ref::<ProviderError>() {
                                    pe
                                } else {
                                    &ProviderError::Other(err.to_string())
                                };

                                let can_resume = can_resume_with_prefill(
                                    &attempt_text,
                                    attempt_yielded_tool_call,
                                    yielded_tool_call,
                                    attempt,
                                    max_attempts,
                                    provider_err,
                                    retry_429,
                                );

                                if can_resume {
                                    accumulated_text.push_str(&attempt_text);
                                    yielded_tool_call = false;
                                    let delay = retry_delay_for_error(provider_err, attempt);
                                    tracing::info!(
                                        ?delay,
                                        attempt,
                                        accumulated_chars = accumulated_text.len(),
                                        "retrying stream with prefill after mid-stream error",
                                    );
                                    tokio::time::sleep(delay).await;
                                    failed = true;
                                    break;
                                }

                                Err(err)?;
                                failed = true;
                                break;
                            }

                            let provider_err = if let Some(pe) = err.downcast_ref::<ProviderError>() {
                                pe
                            } else {
                                &ProviderError::Other(err.to_string())
                            };

                            if attempt >= max_attempts || !should_retry_error(provider_err, retry_429) {
                                Err(err)?;
                                failed = true;
                                break;
                            }

                            let delay = retry_delay_for_error(provider_err, attempt);

                            tracing::info!(?delay, attempt, "retrying stream after delay");
                            tokio::time::sleep(delay).await;
                            failed = true;
                            break;
                        }
                    }
                }

                if !failed {
                    break;
                }
            }
        })
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        // Some providers don't expose a standard /models endpoint.
        // Return models from the registry config instead of hitting a non-existent API.
        if self.provider_identity.is_opencode_family() {
            let models: Vec<String> = self.config.models.iter().map(|m| m.name.clone()).collect();
            if !models.is_empty() {
                tracing::info!(
                    provider = %self.provider_id,
                    models = models.len(),
                    "provider model list from registry (no remote endpoint)"
                );
                return Ok(models);
            }
        }

        let mut base_url = self.base_url.trim_end_matches('/').to_string();
        if base_url.ends_with("/anthropic") {
            base_url = base_url.replace("/anthropic", "/v1");
        }
        let url = if self.provider_identity.as_str() == navi_core::ProviderId::COMMANDCODE {
            if base_url.ends_with("/provider/v1") {
                format!("{}/models", base_url)
            } else {
                format!("{}/provider/v1/models", base_url)
            }
        } else {
            format!("{}/models", base_url)
        };
        tracing::info!(provider = %self.provider_id, "provider model list request started");

        let headers = self
            .behavior
            .build_headers(&self.api_key, Endpoint::Models)?;
        let req = self.client.get(&url).headers(headers);

        let res = self.send_with_retry(req).await?;
        tracing::debug!(provider = %self.provider_id, status = %res.status(), "provider model list response received");

        #[derive(serde::Deserialize)]
        struct OpenAiModelsList {
            data: Vec<OpenAiModelItem>,
        }

        #[derive(serde::Deserialize)]
        struct OpenAiModelItem {
            id: String,
        }

        let list: OpenAiModelsList = res
            .json()
            .await
            .context("failed to parse models JSON response")?;

        let models = unique_sorted_model_ids(list.data.into_iter().map(|item| item.id));
        tracing::info!(provider = %self.provider_id, models = models.len(), "provider model list completed");
        Ok(models)
    }
}

#[cfg(test)]
mod prefill_tests {
    use super::*;
    use navi_core::{ModelMessage, ModelRole, ThinkingConfig};
    use reqwest::StatusCode;

    fn sample_request() -> ModelRequest {
        ModelRequest {
            model: "test-model".into(),
            instructions: None,
            messages: vec![ModelMessage::user("hello")],
            thinking: ThinkingConfig::Off,
            tools: Vec::new(),
            session_id: None,
        }
    }

    #[test]
    fn request_with_prefill_appends_assistant_prefix() {
        let req = request_with_prefill(&sample_request(), "partial answer");
        assert_eq!(req.messages.len(), 2);
        assert_eq!(req.messages[1].role, ModelRole::Assistant);
        assert_eq!(req.messages[1].content, "partial answer");
    }

    #[test]
    fn request_with_prefill_skips_empty_prefix() {
        let req = request_with_prefill(&sample_request(), "");
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, ModelRole::User);
    }

    #[test]
    fn can_resume_with_prefill_allows_transport_after_text() {
        let err = ProviderError::StreamIdleTimeout(Duration::from_secs(1));
        assert!(can_resume_with_prefill(
            "hello", false, false, 1, 5, &err, true
        ));
    }

    #[test]
    fn can_resume_with_prefill_blocks_after_tool_call() {
        let err = ProviderError::StreamIdleTimeout(Duration::from_secs(1));
        assert!(!can_resume_with_prefill(
            "hello", true, false, 1, 5, &err, true
        ));
        assert!(!can_resume_with_prefill(
            "hello", false, true, 1, 5, &err, true
        ));
    }

    #[test]
    fn can_resume_with_prefill_blocks_empty_text_and_exhausted_attempts() {
        let err = ProviderError::StreamIdleTimeout(Duration::from_secs(1));
        assert!(!can_resume_with_prefill("", false, false, 1, 5, &err, true));
        assert!(!can_resume_with_prefill(
            "hello", false, false, 5, 5, &err, true
        ));
    }

    #[test]
    fn can_resume_with_prefill_blocks_non_retryable_error() {
        let err = ProviderError::Api {
            status: StatusCode::BAD_REQUEST,
            body: "bad".into(),
            requested_delay: None,
            body_read_error: None,
        };
        assert!(!can_resume_with_prefill(
            "hello", false, false, 1, 5, &err, true
        ));
    }
}