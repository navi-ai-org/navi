use crate::errors::ProviderError;
use crate::mapping::unique_sorted_model_ids;
use crate::providers::behavior::{Endpoint, ProviderBehavior, behavior_for_provider};
use crate::transport::{
    ensure_success, get_backoff_delay, retry_delay_for_error, should_retry_error,
};
use crate::types::{OpenAiApiKind, StreamRoute};
use navi_core::ProviderId;
use anyhow::{Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures_util::StreamExt;
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
                .ok_or_else(|| anyhow::anyhow!("provider {} requires base_url in config", provider.id))?
                .to_string(),
        };
        let api_kind = match provider.kind {
            ProviderKind::OpenAiResponses => OpenAiApiKind::Responses,
            ProviderKind::OpenAiChatCompletions => OpenAiApiKind::ChatCompletions,
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
        let identity = ProviderId::OpenAi;
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

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn stream(&self, request: ModelRequest) -> ModelStream {
        let provider = self.clone();
        Box::pin(try_stream! {
            let max_attempts = provider.config.stream_max_retries();
            let retry_429 = provider.config.retry_429();
            let mut attempt = 0;
            let mut content_yielded = false;

            loop {
                attempt += 1;
                tracing::debug!(attempt, max_attempts, "starting stream attempt");

                let mut inner_stream = provider.stream_inner(request.clone());
                let mut failed = false;

                while let Some(item) = inner_stream.next().await {
                    match item {
                        Ok(event) => {
                            match &event {
                                ModelStreamEvent::TextDelta { .. } |
                                ModelStreamEvent::ThinkingDelta { .. } |
                                ModelStreamEvent::ToolCall(_) => {
                                    content_yielded = true;
                                }
                                _ => {}
                            }
                            yield event;
                        }
                        Err(err) => {
                            tracing::warn!(?err, attempt, "stream chunk error occurred");

                            if content_yielded {
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
        let base_url = self.base_url.trim_end_matches('/');
        let url = format!("{}/models", base_url);
        tracing::info!(provider = %self.provider_id, "provider model list request started");

        let headers = self.behavior.build_headers(&self.api_key, Endpoint::Models)?;
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
