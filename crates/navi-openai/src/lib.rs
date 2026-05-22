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

pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    api_kind: OpenAiApiKind,
    provider_id: String,
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
            .with_provider_id(provider.id.clone()))
    }

    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
            base_url: "https://api.openai.com/v1".to_string(),
            api_kind: OpenAiApiKind::Responses,
            provider_id: "openai".to_string(),
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
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn stream(&self, request: ModelRequest) -> ModelStream {
        match self.api_kind {
            OpenAiApiKind::Responses => self.stream_responses(request),
            OpenAiApiKind::ChatCompletions => match self.provider_id.as_str() {
                "anthropic" => self.stream_anthropic_messages(request),
                "google-gemini" => self.stream_gemini_generate_content(request),
                _ => self.stream_chat_completions(request),
            },
        }
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

        let res = req.send().await.context("failed to send models request")?;
        tracing::debug!(provider = %self.provider_id, status = %res.status(), "provider model list response received");
        let res = ensure_success(res).await?;

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
            request.thinking.adapter_for_provider(&provider_id),
            OpenAiApiKind::Responses,
        );
        body["stream"] = json!(true);

        let response = client
            .post(format!("{}/responses", base_url.trim_end_matches('/')))
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send OpenAI Responses API request")?;

        tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
        let response = ensure_success(response).await?;
        let mut decoder = SseDecoder::default();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            for data in decoder.push_bytes(&chunk.context("failed to read OpenAI Responses stream")?) {
                for event in parse_openai_responses_sse(&data) {
                    yield event?;
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
            request.thinking.adapter_for_provider(&provider_id),
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
        }

        let response = req
            .json(&body)
            .send()
            .await
            .context("failed to send OpenAI-compatible chat completions request")?;

        tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
        let response = ensure_success(response).await?;
        let mut decoder = SseDecoder::default();
        let mut tool_calls = ChatToolCallAccumulator::default();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            for data in decoder.push_bytes(&chunk.context("failed to read chat completions stream")?) {
                for event in parse_chat_completions_sse_with_state(&data, &mut tool_calls) {
                    yield event?;
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
                .context("failed to send Anthropic Messages stream request")?;

            tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                for data in decoder.push_bytes(&chunk.context("failed to read Anthropic stream")?) {
                    for event in parse_anthropic_sse(&data) {
                        yield event?;
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
                .context("failed to send Gemini stream request")?;

            tracing::debug!(provider = %provider_id, model = %model_name, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut chunks = response.bytes_stream();
            while let Some(chunk) = chunks.next().await {
                for data in decoder.push_bytes(&chunk.context("failed to read Gemini stream")?) {
                    for event in parse_gemini_sse(&data) {
                        yield event?;
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

async fn ensure_success(response: Response) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<failed to read error body>".to_string());
    tracing::warn!(status = %status, "provider request failed");
    anyhow::bail!("provider request failed with {status}: {body}");
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
}
