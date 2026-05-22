use anyhow::{Context, Result};
use async_stream::try_stream;
use async_trait::async_trait;
use futures_util::StreamExt;
use navi_core::{
    ModelMessage, ModelProvider, ModelRequest, ModelRole, ModelStream, ModelStreamEvent,
    ProviderConfig, ProviderKind, ThinkingAdapter, ToolDefinition, ToolInvocation,
};
use reqwest::{Client, Response};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy)]
pub enum OpenAiApiKind {
    Responses,
    ChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamRoute {
    Responses,
    ChatCompletions,
    AnthropicMessages,
}

#[derive(Clone)]
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    api_kind: OpenAiApiKind,
    provider_id: String,
    config: ProviderConfig,
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
        let base_url = match &provider.base_url {
            Some(url) => url.clone(),
            None => match provider.id.as_str() {
                "opencode" => "https://opencode.ai/zen/v1".to_string(),
                "opencode-zen" => "https://opencode.ai/zen/v1".to_string(),
                "opencode-go" => "https://opencode.ai/zen/go/v1".to_string(),
                _ => anyhow::bail!("provider {} requires base_url", provider.id),
            },
        };
        let api_kind = match provider.kind {
            ProviderKind::OpenAiResponses => OpenAiApiKind::Responses,
            ProviderKind::OpenAiChatCompletions => OpenAiApiKind::ChatCompletions,
        };

        Ok(Self::new(api_key)
            .with_base_url(base_url)
            .with_api_kind(api_kind)
            .with_provider_id(provider.id.clone())
            .with_config(provider.clone()))
    }

    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
            api_kind: OpenAiApiKind::Responses,
            provider_id: "openai".to_string(),
            config: ProviderConfig::default(),
        }
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
        self.provider_id = provider_id.into();
        self
    }

    pub fn with_config(mut self, config: ProviderConfig) -> Self {
        self.config = config;
        self
    }

    fn stream_inner(&self, request: ModelRequest) -> ModelStream {
        if self.provider_id == "opencode" {
            return match opencode_stream_route(&request.model) {
                StreamRoute::Responses => self.stream_responses(request),
                StreamRoute::AnthropicMessages => self.stream_anthropic_messages(request),
                StreamRoute::ChatCompletions => self.stream_chat_completions(request),
            };
        }

        match self.api_kind {
            OpenAiApiKind::Responses => self.stream_responses(request),
            OpenAiApiKind::ChatCompletions => match self.provider_id.as_str() {
                "anthropic" => self.stream_anthropic_messages(request),
                "google-gemini" => self.stream_gemini_generate_content(request),
                _ => self.stream_chat_completions(request),
            },
        }
    }

    async fn send_with_retry(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<Response, ProviderError> {
        let max_attempts = self.config.request_max_retries();
        let retry_429 = self.config.retry_429();
        let mut attempt = 0;

        loop {
            attempt += 1;

            let req = builder.try_clone().ok_or_else(|| {
                ProviderError::Other("failed to clone RequestBuilder".to_string())
            })?;

            let req = req.timeout(std::time::Duration::from_millis(
                self.config.request_timeout_ms(),
            ));

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

fn opencode_stream_route(model: &str) -> StreamRoute {
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

        let mut req = self.client.get(&url);

        if self.provider_id == "anthropic" {
            req = req
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01");
        } else {
            req = req.bearer_auth(&self.api_key);
        }

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

fn unique_sorted_model_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut models: Vec<String> = ids.into_iter().collect();
    models.sort();
    models.dedup();
    models
}

impl OpenAiProvider {
    fn stream_responses(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();

        Box::pin(try_stream! {
        let model = request.model.clone();
        tracing::info!(provider = %provider_id, model = %model, api = "responses", tools = request.tools.len(), "provider stream started");
        let mut body = json!({
            "model": request.model,
            "input": request.messages.iter().flat_map(responses_input_item_to_json).collect::<Vec<_>>(),
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(responses_tool_to_json).collect::<Vec<_>>());
            body["tool_choice"] = json!("auto");
        }
        apply_thinking_to_body(
            &mut body,
            thinking_adapter_for_api(&provider_id, request.thinking, OpenAiApiKind::Responses),
            OpenAiApiKind::Responses,
        );
        body["stream"] = json!(true);

        let response = client
            .post(format!("{}/responses", base_url.trim_end_matches('/')))
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Transport)?;

        tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
        let response = ensure_success(response).await?;
        let mut decoder = SseDecoder::default();
        let mut chunks = response.bytes_stream();

        let idle_timeout = std::time::Duration::from_millis(stream_idle_timeout_ms);
        loop {
            let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
            match next_chunk {
                Ok(Some(chunk_res)) => {
                    let bytes = chunk_res.map_err(ProviderError::Transport)?;
                    for data in decoder.push_bytes(bytes.as_ref()) {
                        for event in parse_openai_responses_sse(&data) {
                            yield event?;
                        }
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(_) => {
                    Err(ProviderError::StreamIdleTimeout(idle_timeout))?;
                }
            }
        }
        tracing::info!(provider = %provider_id, model = %model, "provider stream completed");
        yield ModelStreamEvent::Done;
        })
    }

    fn stream_chat_completions(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();

        Box::pin(try_stream! {
        let model = request.model.clone();
        tracing::info!(provider = %provider_id, model = %model, api = "chat-completions", tools = request.tools.len(), "provider stream started");
        let mut body = json!({
            "model": request.model,
            "messages": request.messages.iter().map(message_to_json).collect::<Vec<_>>(),
        });
        if !request.tools.is_empty() {
            body["tools"] = json!(request.tools.iter().map(chat_tool_to_json).collect::<Vec<_>>());
            body["tool_choice"] = json!("auto");
        }
        apply_thinking_to_body(
            &mut body,
            thinking_adapter_for_api(
                &provider_id,
                request.thinking,
                OpenAiApiKind::ChatCompletions,
            ),
            OpenAiApiKind::ChatCompletions,
        );
        body["stream"] = json!(true);

        let mut req = client
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .header("Accept", "text/event-stream")
            .bearer_auth(&api_key);

        if provider_id == "openrouter" {
            req = req.header("HTTP-Referer", "https://github.com/enrell/navi")
                     .header("X-Title", "Navi");
        } else if provider_id == "github-copilot" {
            req = req.header("User-Agent", "navi/0.1.0")
                     .header("Openai-Intent", "conversation-edits")
                     .header("x-initiator", "user");
        }

        let response = req
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Transport)?;

        tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
        let response = ensure_success(response).await?;
        let mut decoder = SseDecoder::default();
        let mut tool_calls = ChatToolCallAccumulator::default();
        let mut chunks = response.bytes_stream();

        let idle_timeout = std::time::Duration::from_millis(stream_idle_timeout_ms);
        loop {
            let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
            match next_chunk {
                Ok(Some(chunk_res)) => {
                    let bytes = chunk_res.map_err(ProviderError::Transport)?;
                    for data in decoder.push_bytes(bytes.as_ref()) {
                        for event in parse_chat_completions_sse_with_state(&data, &mut tool_calls) {
                            yield event?;
                        }
                    }
                }
                Ok(None) => {
                    break;
                }
                Err(_) => {
                    Err(ProviderError::StreamIdleTimeout(idle_timeout))?;
                }
            }
        }
        tracing::info!(provider = %provider_id, model = %model, "provider stream completed");
        yield ModelStreamEvent::Done;
        })
    }

    fn stream_anthropic_messages(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();

        Box::pin(try_stream! {
            let model = request.model.clone();
            tracing::info!(provider = %provider_id, model = %model, api = "anthropic-messages", tools = request.tools.len(), "provider stream started");
            if !request.tools.is_empty() {
                Err(anyhow::anyhow!("native Anthropic tool calling is not implemented yet"))?;
            }
            let (system, messages) = anthropic_messages(&request.messages);
            let thinking = request.thinking.to_anthropic_thinking();
            let budget = thinking
                .get("budget_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let max_tokens = (budget + 1024).max(4096);
            let mut body = json!({
                "model": request.model,
                "max_tokens": max_tokens,
                "stream": true,
                "messages": messages,
            });
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
                body["thinking"] = thinking;
            }

            let response = client
                .post(format!("{}/messages", base_url.trim_end_matches('/')))
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("Accept", "text/event-stream")
                .json(&body)
                .send()
                .await
                .map_err(ProviderError::Transport)?;

            tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut chunks = response.bytes_stream();

            let idle_timeout = std::time::Duration::from_millis(stream_idle_timeout_ms);
            loop {
                let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
                match next_chunk {
                    Ok(Some(chunk_res)) => {
                        let bytes = chunk_res.map_err(ProviderError::Transport)?;
                        for data in decoder.push_bytes(bytes.as_ref()) {
                            for event in parse_anthropic_sse(&data) {
                                yield event?;
                            }
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(_) => {
                        Err(ProviderError::StreamIdleTimeout(idle_timeout))?;
                    }
                }
            }
            tracing::info!(provider = %provider_id, model = %model, "provider stream completed");
            yield ModelStreamEvent::Done;
        })
    }

    fn stream_gemini_generate_content(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();

        Box::pin(try_stream! {
            let model_name = request.model.clone();
            tracing::info!(provider = %provider_id, model = %model_name, api = "gemini-generate-content", tools = request.tools.len(), "provider stream started");
            if !request.tools.is_empty() {
                Err(anyhow::anyhow!("native Gemini tool calling is not implemented yet"))?;
            }
            let (system, contents) = gemini_contents(&request.messages);
            let mut body = json!({
                "contents": contents,
                "generationConfig": {
                    "thinkingConfig": request.thinking.to_gemini_thinking_config(),
                }
            });
            if !system.is_empty() {
                body["systemInstruction"] = json!({
                    "parts": [{ "text": system }]
                });
            }
            let model = encode_model_for_url(&request.model);
            let response = client
                .post(format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{model}:streamGenerateContent?alt=sse&key={api_key}"
                ))
                .json(&body)
                .send()
                .await
                .map_err(ProviderError::Transport)?;

            tracing::debug!(provider = %provider_id, model = %model_name, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut chunks = response.bytes_stream();

            let idle_timeout = std::time::Duration::from_millis(stream_idle_timeout_ms);
            loop {
                let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
                match next_chunk {
                    Ok(Some(chunk_res)) => {
                        let bytes = chunk_res.map_err(ProviderError::Transport)?;
                        for data in decoder.push_bytes(bytes.as_ref()) {
                            for event in parse_gemini_sse(&data) {
                                yield event?;
                            }
                        }
                    }
                    Ok(None) => {
                        break;
                    }
                    Err(_) => {
                        Err(ProviderError::StreamIdleTimeout(idle_timeout))?;
                    }
                }
            }
            tracing::info!(provider = %provider_id, model = %model_name, "provider stream completed");
            yield ModelStreamEvent::Done;
        })
    }
}

fn message_to_json(message: &ModelMessage) -> Value {
    let mut value = json!({
        "role": match message.role {
            ModelRole::System => "system",
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
            ModelRole::Tool => "tool",
        },
        "content": message.content,
    });
    if let Some(tool_call_id) = &message.tool_call_id {
        value["tool_call_id"] = json!(tool_call_id);
    }
    if let Some(tool_name) = &message.tool_name {
        value["name"] = json!(tool_name);
    }
    if !message.tool_calls.is_empty() {
        value["content"] = Value::Null;
        value["tool_calls"] = json!(
            message
                .tool_calls
                .iter()
                .map(chat_tool_call_to_json)
                .collect::<Vec<_>>()
        );
    }
    value
}

fn responses_input_item_to_json(message: &ModelMessage) -> Vec<Value> {
    if message.role == ModelRole::Tool {
        return vec![json!({
            "type": "function_call_output",
            "call_id": message.tool_call_id,
            "output": message.content,
        })];
    }

    if !message.tool_calls.is_empty() {
        return message
            .tool_calls
            .iter()
            .map(|call| {
                json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.tool_name,
                    "arguments": call.input.to_string(),
                })
            })
            .collect();
    }

    vec![message_to_json(message)]
}

fn chat_tool_call_to_json(invocation: &ToolInvocation) -> Value {
    json!({
        "id": invocation.id,
        "type": "function",
        "function": {
            "name": invocation.tool_name,
            "arguments": invocation.input.to_string(),
        }
    })
}

fn responses_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

fn chat_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

fn apply_thinking_to_body(body: &mut Value, adapter: ThinkingAdapter, api_kind: OpenAiApiKind) {
    let Some(object) = body.as_object_mut() else {
        return;
    };

    match adapter {
        ThinkingAdapter::OpenAiResponses(reasoning) => {
            if matches!(api_kind, OpenAiApiKind::Responses) {
                object.insert("reasoning".to_string(), reasoning);
            }
        }
        ThinkingAdapter::OpenAiChatCompletions(effort) | ThinkingAdapter::Groq(effort) => {
            if matches!(api_kind, OpenAiApiKind::ChatCompletions) {
                object.insert("reasoning_effort".to_string(), json!(effort));
            }
        }
        ThinkingAdapter::AnthropicOpenAiCompatible(thinking) => {
            if matches!(api_kind, OpenAiApiKind::ChatCompletions) {
                object.insert("thinking".to_string(), thinking);
            }
        }
        ThinkingAdapter::GeminiOpenAiCompatible(thinking_config) => {
            if matches!(api_kind, OpenAiApiKind::ChatCompletions) {
                object.insert(
                    "extra_body".to_string(),
                    json!({ "google": { "thinking_config": thinking_config } }),
                );
            }
        }
        ThinkingAdapter::OpenRouter(reasoning) => {
            if matches!(api_kind, OpenAiApiKind::ChatCompletions) {
                object.insert("reasoning".to_string(), reasoning);
            }
        }
        ThinkingAdapter::Unsupported => {}
    }
}

fn thinking_adapter_for_api(
    provider_id: &str,
    thinking: navi_core::ThinkingConfig,
    api_kind: OpenAiApiKind,
) -> ThinkingAdapter {
    if provider_id == "opencode" && matches!(api_kind, OpenAiApiKind::Responses) {
        return thinking
            .to_openai_effort()
            .map(|effort| ThinkingAdapter::OpenAiResponses(json!({ "effort": effort })))
            .unwrap_or(ThinkingAdapter::Unsupported);
    }

    thinking.adapter_for_provider(provider_id)
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("API error {status}: {body} (requested delay: {requested_delay:?})")]
    Api {
        status: reqwest::StatusCode,
        body: String,
        requested_delay: Option<std::time::Duration>,
    },
    #[error("Transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Stream idle timeout: stream was idle for more than {0:?}")]
    StreamIdleTimeout(std::time::Duration),
    #[error("Other error: {0}")]
    Other(String),
}

fn should_retry_status(status: reqwest::StatusCode, retry_429: bool) -> bool {
    status.is_server_error() || (retry_429 && status == reqwest::StatusCode::TOO_MANY_REQUESTS)
}

fn should_retry_error(err: &ProviderError, retry_429: bool) -> bool {
    match err {
        ProviderError::Transport(_) => true,
        ProviderError::Api { status, body, .. } => {
            should_retry_status(*status, retry_429) && !is_usage_limit_error(body)
        }
        ProviderError::StreamIdleTimeout(_) => true,
        ProviderError::Other(_) => false,
    }
}

fn retry_delay_for_error(err: &ProviderError, attempt: u32) -> std::time::Duration {
    const MAX_REQUESTED_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(60);

    if let ProviderError::Api {
        requested_delay: Some(delay),
        ..
    } = err
    {
        return (*delay).min(MAX_REQUESTED_RETRY_DELAY);
    }

    get_backoff_delay(attempt)
}

fn is_usage_limit_error(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    body.contains("freeusagelimiterror")
        || body.contains("free usage limit")
        || body.contains("usage limit exceeded")
}

fn get_jitter() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(123456789);
    let a: u64 = 6364136223846793005;
    let c: u64 = 1442695040888963407;
    let seed = nanos as u64;
    let rand_val = seed.wrapping_mul(a).wrapping_add(c);
    let normalized = (rand_val as f64) / (u64::MAX as f64);
    (normalized * 0.20) - 0.10
}

fn get_backoff_delay(attempt: u32) -> std::time::Duration {
    let exponent = (attempt.saturating_sub(1)).min(10);
    let base_ms = 200 * (1 << exponent);

    let jitter_pct = get_jitter();
    let jitter_ms = (base_ms as f64 * jitter_pct) as i64;
    let final_ms = (base_ms as i64 + jitter_ms).max(0) as u64;

    std::time::Duration::from_millis(final_ms)
}

fn parse_retry_after(header_val: &str) -> Option<std::time::Duration> {
    if let Ok(seconds) = header_val.trim().parse::<u64>() {
        return Some(std::time::Duration::from_secs(seconds));
    }
    None
}

fn value_to_duration_seconds(val: &Value) -> Option<std::time::Duration> {
    if let Some(ms) = val.as_u64() {
        Some(std::time::Duration::from_secs(ms))
    } else if let Some(secs) = val.as_f64() {
        Some(std::time::Duration::from_secs_f64(secs))
    } else {
        None
    }
}

fn extract_requested_delay_from_json(json: &Value) -> Option<std::time::Duration> {
    if let Some(val) = json.get("requested_delay_ms").and_then(Value::as_u64) {
        return Some(std::time::Duration::from_millis(val));
    }
    if let Some(error) = json.get("error") {
        if let Some(val) = error.get("requested_delay_ms").and_then(Value::as_u64) {
            return Some(std::time::Duration::from_millis(val));
        }
    }

    if let Some(val) = json.get("requested_delay") {
        if let Some(dur) = value_to_duration_seconds(val) {
            return Some(dur);
        }
    }
    if let Some(error) = json.get("error") {
        if let Some(val) = error.get("requested_delay") {
            if let Some(dur) = value_to_duration_seconds(val) {
                return Some(dur);
            }
        }
    }

    None
}

async fn ensure_success(response: Response) -> Result<Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let mut requested_delay = None;
    if let Some(retry_after_header) = response.headers().get(reqwest::header::RETRY_AFTER) {
        if let Ok(retry_after_str) = retry_after_header.to_str() {
            requested_delay = parse_retry_after(retry_after_str);
        }
    }

    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<failed to read error body>".to_string());

    if let Ok(json_body) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(delay) = extract_requested_delay_from_json(&json_body) {
            requested_delay = Some(delay);
        }
    }

    tracing::warn!(status = %status, ?requested_delay, "provider request failed");
    Err(ProviderError::Api {
        status,
        body,
        requested_delay,
    })
}

#[derive(Default)]
struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    fn push_bytes(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();

        while let Some(index) = self.buffer.find("\n\n") {
            let raw = self.buffer[..index].to_string();
            self.buffer.drain(..index + 2);
            if let Some(data) = sse_data(&raw) {
                events.push(data);
            }
        }

        events
    }
}

fn sse_data(raw: &str) -> Option<String> {
    let data = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");

    (!data.is_empty()).then_some(data)
}

fn parse_openai_responses_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    if data == "[DONE]" {
        return vec![Ok(ModelStreamEvent::Done)];
    }
    let value = match serde_json::from_str::<Value>(data) {
        Ok(value) => value,
        Err(err) => return vec![Err(err.into())],
    };

    match value.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => value
            .get("delta")
            .and_then(Value::as_str)
            .map(text_delta)
            .into_iter()
            .collect(),
        Some("response.reasoning_summary_text.delta") | Some("response.reasoning_text.delta") => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                vec![Ok(ModelStreamEvent::ThinkingDelta {
                    text: delta.to_string(),
                })]
            } else {
                vec![Ok(ModelStreamEvent::Status {
                    label: "thinking".to_string(),
                })]
            }
        }
        Some("response.output_item.done") => value
            .get("item")
            .and_then(parse_responses_tool_call)
            .map(|tool_call| vec![Ok(ModelStreamEvent::ToolCall(tool_call))])
            .unwrap_or_default(),
        Some("response.completed") => {
            let mut events = usage_from_value(value.get("response").and_then(|v| v.get("usage")));
            events.push(Ok(ModelStreamEvent::Done));
            events
        }
        Some("response.failed") => vec![Err(anyhow::anyhow!(
            "{}",
            value
                .get("response")
                .and_then(|v| v.get("error"))
                .unwrap_or(&value)
        ))],
        _ => Vec::new(),
    }
}

fn parse_responses_tool_call(item: &Value) -> Option<ToolInvocation> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)?
        .to_string();
    let tool_name = item.get("name").and_then(Value::as_str)?.to_string();
    let input = item
        .get("arguments")
        .and_then(Value::as_str)
        .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
        .unwrap_or_else(|| json!({}));
    Some(ToolInvocation {
        id,
        tool_name,
        input,
    })
}

#[cfg(test)]
fn parse_chat_completions_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    parse_chat_completions_sse_with_state(data, &mut ChatToolCallAccumulator::default())
}

fn parse_chat_completions_sse_with_state(
    data: &str,
    tool_calls: &mut ChatToolCallAccumulator,
) -> Vec<Result<ModelStreamEvent>> {
    if data == "[DONE]" {
        return vec![Ok(ModelStreamEvent::Done)];
    }
    let value = match serde_json::from_str::<Value>(data) {
        Ok(value) => value,
        Err(err) => return vec![Err(err.into())],
    };

    let mut events = usage_from_value(value.get("usage"));
    if let Some(delta) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("delta"))
    {
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            events.push(text_delta(content));
        }
        if let Some(chunks) = delta.get("tool_calls").and_then(Value::as_array) {
            tool_calls.push_chunks(chunks);
        }
        if let Some(reasoning) = delta
            .get("reasoning")
            .or_else(|| delta.get("reasoning_content"))
            .or_else(|| delta.get("reasoning_details"))
        {
            let text = reasoning_text(reasoning);
            if !text.is_empty() {
                events.push(Ok(ModelStreamEvent::ThinkingDelta { text }));
            }
        }
    }
    if value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
        .is_some()
    {
        events.extend(tool_calls.drain_complete());
        events.push(Ok(ModelStreamEvent::Done));
    }
    events
}

fn reasoning_text(value: &Value) -> String {
    if value.is_null() {
        return String::new();
    }
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .map(reasoning_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    if let Some(text) = value
        .get("text")
        .or_else(|| value.get("content"))
        .or_else(|| value.get("summary"))
        .and_then(Value::as_str)
    {
        return text.to_string();
    }
    String::new()
}

#[derive(Default)]
struct ChatToolCallAccumulator {
    calls: Vec<PartialChatToolCall>,
}

#[derive(Default)]
struct PartialChatToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatToolCallAccumulator {
    fn push_chunks(&mut self, chunks: &[Value]) {
        for chunk in chunks {
            let index = chunk.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while self.calls.len() <= index {
                self.calls.push(PartialChatToolCall::default());
            }
            let call = &mut self.calls[index];
            if let Some(id) = chunk.get("id").and_then(Value::as_str) {
                call.id = Some(id.to_string());
            }
            if let Some(function) = chunk.get("function") {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    call.name = Some(name.to_string());
                }
                if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                    call.arguments.push_str(arguments);
                }
            }
        }
    }

    fn drain_complete(&mut self) -> Vec<Result<ModelStreamEvent>> {
        self.calls
            .drain(..)
            .filter_map(|call| {
                let id = call.id?;
                let tool_name = call.name?;
                let input = serde_json::from_str::<Value>(&call.arguments).unwrap_or_else(|_| {
                    json!({
                        "raw_arguments": call.arguments,
                    })
                });
                Some(Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                    id,
                    tool_name,
                    input,
                })))
            })
            .collect()
    }
}

fn parse_anthropic_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    let value = match serde_json::from_str::<Value>(data) {
        Ok(value) => value,
        Err(err) => return vec![Err(err.into())],
    };

    match value.get("type").and_then(Value::as_str) {
        Some("content_block_delta") => match value
            .get("delta")
            .and_then(|delta| delta.get("type"))
            .and_then(Value::as_str)
        {
            Some("text_delta") => value
                .get("delta")
                .and_then(|delta| delta.get("text"))
                .and_then(Value::as_str)
                .map(text_delta)
                .into_iter()
                .collect(),
            Some("thinking_delta") => {
                if let Some(thinking) = value
                    .get("delta")
                    .and_then(|delta| delta.get("thinking"))
                    .and_then(Value::as_str)
                {
                    vec![Ok(ModelStreamEvent::ThinkingDelta {
                        text: thinking.to_string(),
                    })]
                } else {
                    vec![Ok(ModelStreamEvent::Status {
                        label: "thinking".to_string(),
                    })]
                }
            }
            Some("signature_delta") => {
                vec![Ok(ModelStreamEvent::Status {
                    label: "thinking".to_string(),
                })]
            }
            _ => Vec::new(),
        },
        Some("message_delta") => usage_from_value(value.get("usage")),
        Some("message_stop") => vec![Ok(ModelStreamEvent::Done)],
        Some("error") => vec![Err(anyhow::anyhow!(
            "{}",
            value.get("error").unwrap_or(&value)
        ))],
        _ => Vec::new(),
    }
}

fn parse_gemini_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    let value = match serde_json::from_str::<Value>(data) {
        Ok(value) => value,
        Err(err) => return vec![Err(err.into())],
    };

    let mut events = usage_from_value(value.get("usageMetadata"));
    if let Some(parts) = value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("content"))
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
    {
        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    events.push(Ok(ModelStreamEvent::ThinkingDelta {
                        text: text.to_string(),
                    }));
                } else {
                    events.push(text_delta(text));
                }
            } else if part.get("thought").and_then(Value::as_bool) == Some(true) {
                events.push(Ok(ModelStreamEvent::Status {
                    label: "thinking".to_string(),
                }));
            }
        }
    }
    if value
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str)
        .is_some()
    {
        events.push(Ok(ModelStreamEvent::Done));
    }
    events
}

fn text_delta(text: &str) -> Result<ModelStreamEvent> {
    Ok(ModelStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

fn usage_from_value(value: Option<&Value>) -> Vec<Result<ModelStreamEvent>> {
    let Some(usage) = value else {
        return Vec::new();
    };
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .or_else(|| usage.get("promptTokenCount"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .or_else(|| usage.get("candidatesTokenCount"))
        .and_then(Value::as_u64);

    if input_tokens.is_some() || output_tokens.is_some() {
        vec![Ok(ModelStreamEvent::Usage {
            input_tokens,
            output_tokens,
        })]
    } else {
        Vec::new()
    }
}

fn anthropic_messages(messages: &[ModelMessage]) -> (String, Vec<Value>) {
    let mut system = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role {
            ModelRole::System => system.push(message.content.clone()),
            ModelRole::User | ModelRole::Tool => converted.push(json!({
                "role": "user",
                "content": message.content,
            })),
            ModelRole::Assistant => converted.push(json!({
                "role": "assistant",
                "content": message.content,
            })),
        }
    }

    (system.join("\n\n"), converted)
}

fn gemini_contents(messages: &[ModelMessage]) -> (String, Vec<Value>) {
    let mut system = Vec::new();
    let mut contents = Vec::new();

    for message in messages {
        match message.role {
            ModelRole::System => system.push(message.content.clone()),
            ModelRole::User | ModelRole::Tool => contents.push(json!({
                "role": "user",
                "parts": [{ "text": message.content }],
            })),
            ModelRole::Assistant => contents.push(json!({
                "role": "model",
                "parts": [{ "text": message.content }],
            })),
        }
    }

    (system.join("\n\n"), contents)
}

fn encode_model_for_url(model: &str) -> String {
    model
        .replace('/', "%2F")
        .replace(':', "%3A")
        .replace(' ', "%20")
}

#[cfg(test)]
fn extract_output_text(value: &Value) -> String {
    if let Some(text) = value.get("output_text").and_then(Value::as_str) {
        return text.to_string();
    }

    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|item| {
            item.get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
fn extract_chat_completion_text(value: &Value) -> String {
    value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_listing_ids_are_sorted_and_deduplicated() {
        let ids = unique_sorted_model_ids(
            ["z/model", "a/model", "z/model", "m/model", "a/model"]
                .into_iter()
                .map(str::to_string),
        );

        assert_eq!(ids, vec!["a/model", "m/model", "z/model"]);
    }

    #[test]
    fn chat_messages_serialize_assistant_tool_call_and_result() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        };

        let assistant = message_to_json(&ModelMessage::assistant_tool_call(invocation));
        let tool = message_to_json(&ModelMessage::tool_result(
            "call-1",
            "read_file",
            "status: success",
        ));

        assert_eq!(assistant["role"], "assistant");
        assert!(assistant["content"].is_null());
        assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_call_id"], "call-1");
        assert_eq!(tool["name"], "read_file");
    }

    #[test]
    fn responses_messages_serialize_function_call_output() {
        let item = responses_input_item_to_json(&ModelMessage::tool_result(
            "call-1",
            "read_file",
            "status: success",
        ));

        assert_eq!(item.len(), 1);
        assert_eq!(item[0]["type"], "function_call_output");
        assert_eq!(item[0]["call_id"], "call-1");
        assert_eq!(item[0]["output"], "status: success");
    }

    #[test]
    fn extracts_responses_api_output_text_shortcut() {
        let value = json!({ "output_text": "done" });
        assert_eq!(extract_output_text(&value), "done");
    }

    #[test]
    fn extracts_nested_responses_api_text() {
        let value = json!({
            "output": [
                {
                    "content": [
                        { "type": "output_text", "text": "hello " },
                        { "type": "output_text", "text": "world" }
                    ]
                }
            ]
        });

        assert_eq!(extract_output_text(&value), "hello world");
    }

    #[test]
    fn extracts_chat_completion_text() {
        let value = json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "chat done"
                    }
                }
            ]
        });

        assert_eq!(extract_chat_completion_text(&value), "chat done");
    }

    #[test]
    fn applies_openai_responses_reasoning() {
        let mut body = json!({ "model": "gpt-5", "input": [] });

        apply_thinking_to_body(
            &mut body,
            navi_core::ThinkingConfig::High.adapter_for_provider("openai"),
            OpenAiApiKind::Responses,
        );

        assert_eq!(body["reasoning"], json!({ "effort": "high" }));
    }

    #[test]
    fn applies_anthropic_openai_compatible_thinking() {
        let mut body = json!({ "model": "claude-sonnet-4", "messages": [] });

        apply_thinking_to_body(
            &mut body,
            navi_core::ThinkingConfig::Low.adapter_for_provider("anthropic"),
            OpenAiApiKind::ChatCompletions,
        );

        assert_eq!(
            body["thinking"],
            json!({ "type": "enabled", "budget_tokens": 1024 })
        );
    }

    #[test]
    fn applies_openrouter_reasoning_effort() {
        let mut body = json!({ "model": "openai/gpt-5", "messages": [] });

        apply_thinking_to_body(
            &mut body,
            navi_core::ThinkingConfig::Max.adapter_for_provider("openrouter"),
            OpenAiApiKind::ChatCompletions,
        );

        assert_eq!(
            body["reasoning"],
            json!({ "effort": "xhigh", "exclude": true })
        );
    }

    #[test]
    fn parses_openai_responses_text_delta() {
        let events =
            parse_openai_responses_sse(r#"{"type":"response.output_text.delta","delta":"hello"}"#);

        assert_eq!(
            events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::TextDelta {
                text: "hello".to_string()
            }]
        );
    }

    #[test]
    fn parses_chat_completions_text_delta() {
        let events = parse_chat_completions_sse(
            r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
        );

        assert_eq!(
            events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::TextDelta {
                text: "hello".to_string()
            }]
        );
    }

    #[test]
    fn parses_chat_completions_object_reasoning_delta() {
        let events = parse_chat_completions_sse(
            r#"{"choices":[{"delta":{"reasoning_details":[{"text":"I should inspect files."}]},"finish_reason":null}]}"#,
        );

        assert_eq!(
            events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::ThinkingDelta {
                text: "I should inspect files.".to_string()
            }]
        );
    }

    #[test]
    fn ignores_null_reasoning_delta() {
        let events = parse_chat_completions_sse(
            r#"{"choices":[{"delta":{"content":null,"reasoning_details":null},"finish_reason":null}]}"#,
        );

        assert!(events.is_empty());
    }

    #[test]
    fn parses_openai_responses_tool_call() {
        let events = parse_openai_responses_sse(
            r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#,
        );

        assert_eq!(
            events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::ToolCall(ToolInvocation {
                id: "call_1".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({ "path": "Cargo.toml" }),
            })]
        );
    }

    #[test]
    fn accumulates_chat_completion_tool_call_arguments() {
        let mut state = ChatToolCallAccumulator::default();
        let first = parse_chat_completions_sse_with_state(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
            &mut state,
        );
        let second = parse_chat_completions_sse_with_state(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut state,
        );

        assert!(first.is_empty());
        assert_eq!(
            second.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![
                ModelStreamEvent::ToolCall(ToolInvocation {
                    id: "call_1".to_string(),
                    tool_name: "read_file".to_string(),
                    input: json!({ "path": "Cargo.toml" }),
                }),
                ModelStreamEvent::Done,
            ]
        );
    }

    #[test]
    fn parses_anthropic_text_and_thinking_delta() {
        let text = parse_anthropic_sse(
            r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}"#,
        );
        let thinking = parse_anthropic_sse(
            r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hidden"}}"#,
        );

        assert_eq!(
            text.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::TextDelta {
                text: "hello".to_string()
            }]
        );
        assert_eq!(
            thinking.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::ThinkingDelta {
                text: "hidden".to_string()
            }]
        );
    }

    #[test]
    fn parses_gemini_text_delta() {
        let events =
            parse_gemini_sse(r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#);

        assert_eq!(
            events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            vec![ModelStreamEvent::TextDelta {
                text: "hello".to_string()
            }]
        );
    }

    #[test]
    fn sse_decoder_collects_data_frames() {
        let mut decoder = SseDecoder::default();

        let first = decoder.push_bytes(b"event: message\ndata: {\"a\":");
        let second = decoder.push_bytes(b"1}\n\ndata: [DONE]\n\n");

        assert!(first.is_empty());
        assert_eq!(second, vec![r#"{"a":1}"#.to_string(), "[DONE]".to_string()]);
    }

    #[test]
    fn test_backoff_delay() {
        // Since get_jitter() relies on SystemTime, we test that get_backoff_delay(attempt) is within
        // the expected exponential range [base - 10%, base + 10%]
        for attempt in 1..=5 {
            let delay = get_backoff_delay(attempt).as_millis();
            let exponent = (attempt - 1) as u32;
            let base = (200 * (1 << exponent)) as f64;
            let min_expected = (base * 0.9) as u128;
            let max_expected = (base * 1.1) as u128;
            assert!(
                delay >= min_expected && delay <= max_expected,
                "Attempt {}: delay {} not in expected range [{}, {}]",
                attempt,
                delay,
                min_expected,
                max_expected
            );
        }
    }

    #[test]
    fn test_extract_requested_delay_from_json() {
        // requested_delay_ms at root
        let json1 = json!({ "requested_delay_ms": 1500 });
        assert_eq!(
            extract_requested_delay_from_json(&json1),
            Some(std::time::Duration::from_millis(1500))
        );

        // requested_delay_ms nested in error
        let json2 = json!({ "error": { "requested_delay_ms": 2500 } });
        assert_eq!(
            extract_requested_delay_from_json(&json2),
            Some(std::time::Duration::from_millis(2500))
        );

        // requested_delay (seconds as f64) at root
        let json3 = json!({ "requested_delay": 1.5 });
        assert_eq!(
            extract_requested_delay_from_json(&json3),
            Some(std::time::Duration::from_millis(1500))
        );

        // requested_delay (seconds as u64) nested in error
        let json4 = json!({ "error": { "requested_delay": 3 } });
        assert_eq!(
            extract_requested_delay_from_json(&json4),
            Some(std::time::Duration::from_secs(3))
        );

        // No delay present
        let json5 = json!({ "error": { "message": "something failed" } });
        assert_eq!(extract_requested_delay_from_json(&json5), None);
    }

    #[tokio::test]
    async fn test_should_retry_error() {
        use reqwest::StatusCode;

        // Transport error should always be retried
        let transport_err = ProviderError::Transport(
            reqwest::Client::new()
                .get("http://127.0.0.1:1/invalid")
                .send()
                .await
                .unwrap_err(),
        );
        assert!(should_retry_error(&transport_err, false));
        assert!(should_retry_error(&transport_err, true));

        // StreamIdleTimeout should always be retried
        let timeout_err = ProviderError::StreamIdleTimeout(std::time::Duration::from_secs(1));
        assert!(should_retry_error(&timeout_err, false));
        assert!(should_retry_error(&timeout_err, true));

        // 500 server error should always be retried
        let server_err = ProviderError::Api {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: "Internal error".to_string(),
            requested_delay: None,
        };
        assert!(should_retry_error(&server_err, false));
        assert!(should_retry_error(&server_err, true));

        // 429 rate limit error should only be retried if retry_429 is true
        let rate_limit_err = ProviderError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: "Rate limit reached".to_string(),
            requested_delay: None,
        };
        assert!(!should_retry_error(&rate_limit_err, false));
        assert!(should_retry_error(&rate_limit_err, true));

        let free_usage_limit_err = ProviderError::Api {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded."}}"#.to_string(),
            requested_delay: Some(std::time::Duration::from_secs(64_649)),
        };
        assert!(!should_retry_error(&free_usage_limit_err, true));
        assert_eq!(
            retry_delay_for_error(&free_usage_limit_err, 1),
            std::time::Duration::from_secs(60)
        );

        // 400 bad request should never be retried
        let client_err = ProviderError::Api {
            status: StatusCode::BAD_REQUEST,
            body: "Bad request".to_string(),
            requested_delay: None,
        };
        assert!(!should_retry_error(&client_err, false));
        assert!(!should_retry_error(&client_err, true));

        // Other error should never be retried
        let other_err = ProviderError::Other("random error".to_string());
        assert!(!should_retry_error(&other_err, false));
        assert!(!should_retry_error(&other_err, true));
    }

    #[tokio::test]
    async fn test_stream_normal() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        let chunk1 = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
        let chunk2 = "data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\n";
        let chunk3 = "data: [DONE]\n\n";
        let sse_body = format!("{}{}{}", chunk1, chunk2, chunk3);

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(sse_body)
                    .insert_header("content-type", "text/event-stream"),
            )
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "openai".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;

        let provider = OpenAiProvider::new("test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_config(config);

        let request = ModelRequest {
            model: "gpt-4".to_string(),
            messages: vec![ModelMessage {
                role: ModelRole::User,
                content: "Hi".to_string(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: vec![],
                created_at: None,
            }],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                ModelStreamEvent::TextDelta { text: t } => {
                    text.push_str(&t);
                }
                _ => {}
            }
        }
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn test_opencode_zen_chat_request_uses_bearer_api_key() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer zen_test_key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                    )
                    .insert_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "opencode".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;

        let provider = OpenAiProvider::new("zen_test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_provider_id("opencode".to_string())
            .with_config(config);

        let request = ModelRequest {
            model: "deepseek-v4-flash-free".to_string(),
            messages: vec![ModelMessage::user("Hi".to_string())],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }

    #[tokio::test]
    async fn test_opencode_zen_gpt_models_use_responses_endpoint() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer zen_test_key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\ndata: {\"type\":\"response.completed\"}\n\n",
                    )
                    .insert_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "opencode".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;

        let provider = OpenAiProvider::new("zen_test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_provider_id("opencode".to_string())
            .with_config(config);

        let request = ModelRequest {
            model: "gpt-5.5".to_string(),
            messages: vec![ModelMessage::user("Hi".to_string())],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }

    #[tokio::test]
    async fn test_github_copilot_request_uses_oauth_headers() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer copilot_token"))
            .and(header("Openai-Intent", "conversation-edits"))
            .and(header("x-initiator", "user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                    )
                    .insert_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "github-copilot".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;

        let provider = OpenAiProvider::new("copilot_token".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_provider_id("github-copilot".to_string())
            .with_config(config);

        let request = ModelRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![ModelMessage::user("Hi".to_string())],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }

    #[tokio::test]
    async fn test_opencode_zen_claude_models_use_messages_endpoint() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("x-api-key", "zen_test_key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
                    .insert_header("content-type", "text/event-stream"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "opencode".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;

        let provider = OpenAiProvider::new("zen_test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_provider_id("opencode".to_string())
            .with_config(config);

        let request = ModelRequest {
            model: "claude-sonnet-4.5".to_string(),
            messages: vec![ModelMessage::user("Hi".to_string())],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    }

    #[tokio::test]
    async fn test_request_timeout() {
        use std::time::Duration;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "data": [] }))
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "openai".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;
        config.request_timeout_ms = Some(100);
        config.request_max_retries = Some(1);

        let provider = OpenAiProvider::new("test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_config(config);

        let res = provider.list_models().await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        let provider_err = err.downcast_ref::<ProviderError>().unwrap();
        assert!(matches!(provider_err, ProviderError::Transport(e) if e.is_timeout()));
    }

    #[tokio::test]
    async fn test_stream_idle_timeout() {
        use std::time::Duration;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{}", addr);

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0; 1024];
                let _ = socket.read(&mut buf).await;

                let response_headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                let _ = socket.write_all(response_headers.as_bytes()).await;

                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        });

        let mut config = ProviderConfig::default();
        config.id = "openai".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;
        config.stream_idle_timeout_ms = Some(100);
        config.stream_max_retries = Some(1);

        let provider = OpenAiProvider::new("test_key".to_string())
            .with_base_url(base_url)
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_config(config);

        let request = ModelRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        let item = stream.next().await;
        assert!(item.is_some());
        let err = item.unwrap().unwrap_err();
        let provider_err = err.downcast_ref::<ProviderError>().unwrap();
        assert!(matches!(provider_err, ProviderError::StreamIdleTimeout(_)));
    }

    #[tokio::test]
    async fn test_rate_limit_retry() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": {
                    "message": "Rate limit reached",
                    "requested_delay_ms": 10
                }
            })))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                    .insert_header("content-type", "text/event-stream")
            )
            .mount(&mock_server)
            .await;

        let mut config = ProviderConfig::default();
        config.id = "openai".to_string();
        config.kind = ProviderKind::OpenAiChatCompletions;
        config.stream_max_retries = Some(3);
        config.retry_429 = Some(true);

        let provider = OpenAiProvider::new("test_key".to_string())
            .with_base_url(mock_server.uri())
            .with_api_kind(OpenAiApiKind::ChatCompletions)
            .with_config(config);

        let request = ModelRequest {
            model: "gpt-4".to_string(),
            messages: vec![],
            thinking: navi_core::ThinkingConfig::Off,
            tools: vec![],
        };

        let mut stream = provider.stream(request);
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                ModelStreamEvent::TextDelta { text: t } => {
                    text.push_str(&t);
                }
                _ => {}
            }
        }
        assert_eq!(text, "hello");
    }
}
