use anyhow::{Context, Result};
use async_trait::async_trait;
use navi_core::{
    ModelMessage, ModelProvider, ModelRequest, ModelResponse, ModelRole, ProviderConfig,
    ProviderKind, ThinkingAdapter,
};
use reqwest::Client;
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
        let base_url = provider
            .base_url
            .clone()
            .with_context(|| format!("provider {} requires base_url", provider.id))?;
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
    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
        match self.api_kind {
            OpenAiApiKind::Responses => self.complete_responses(request).await,
            OpenAiApiKind::ChatCompletions => self.complete_chat_completions(request).await,
        }
    }
}

impl OpenAiProvider {
    async fn complete_responses(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({
            "model": request.model,
            "input": request.messages.iter().map(message_to_json).collect::<Vec<_>>(),
        });
        apply_thinking_to_body(
            &mut body,
            request.thinking.adapter_for_provider(&self.provider_id),
            OpenAiApiKind::Responses,
        );

        let response = self
            .client
            .post(format!("{}/responses", self.base_url.trim_end_matches('/')))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send OpenAI Responses API request")?;

        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .context("failed to decode OpenAI Responses API response")?;

        if !status.is_success() {
            anyhow::bail!("OpenAI request failed with {status}: {value}");
        }

        Ok(ModelResponse {
            text: extract_output_text(&value),
        })
    }

    async fn complete_chat_completions(&self, request: ModelRequest) -> Result<ModelResponse> {
        let mut body = json!({
            "model": request.model,
            "messages": request.messages.iter().map(message_to_json).collect::<Vec<_>>(),
        });
        apply_thinking_to_body(
            &mut body,
            request.thinking.adapter_for_provider(&self.provider_id),
            OpenAiApiKind::ChatCompletions,
        );

        let response = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send OpenAI-compatible chat completions request")?;

        let status = response.status();
        let value = response
            .json::<Value>()
            .await
            .context("failed to decode OpenAI-compatible chat completions response")?;

        if !status.is_success() {
            anyhow::bail!("chat completions request failed with {status}: {value}");
        }

        Ok(ModelResponse {
            text: extract_chat_completion_text(&value),
        })
    }
}

fn message_to_json(message: &ModelMessage) -> Value {
    json!({
        "role": match message.role {
            ModelRole::System => "system",
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
        },
        "content": message.content,
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
}
