use crate::errors::ProviderError;
use crate::mapping::{text_delta, usage_from_value};
use crate::sse::SseDecoder;
use crate::transport::ensure_success;
use anyhow::Result;
use async_stream::try_stream;
use futures_util::StreamExt;
use navi_core::{ModelMessage, ModelRequest, ModelRole, ModelStream, ModelStreamEvent};
use serde_json::{Value, json};
use std::time::Duration;

impl crate::provider::OpenAiProvider {
    pub(crate) fn stream_gemini_generate_content(&self, request: ModelRequest) -> ModelStream {
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

            let idle_timeout = Duration::from_millis(stream_idle_timeout_ms);
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

pub(crate) fn parse_gemini_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
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

pub(crate) fn gemini_contents(messages: &[ModelMessage]) -> (String, Vec<Value>) {
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

pub(crate) fn encode_model_for_url(model: &str) -> String {
    model
        .replace('/', "%2F")
        .replace(':', "%3A")
        .replace(' ', "%20")
}
