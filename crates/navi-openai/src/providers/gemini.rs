use crate::errors::ProviderError;
use crate::mapping::{text_delta, usage_from_value};
use crate::sse::SseDecoder;
use crate::transport::ensure_success;
use anyhow::Result;
use async_stream::try_stream;
use futures_util::StreamExt;
use navi_core::{
    ContentPart, ModelMessage, ModelRequest, ModelRole, ModelStream, ModelStreamEvent,
    ToolDefinition, ToolInvocation,
};
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
            let (system, contents) = gemini_contents(&request.messages);
            let thinking = request.thinking.to_thinking_request();
            let thinking_budget = if thinking.enabled {
                thinking.budget_tokens.unwrap_or(0)
            } else {
                0
            };
            let mut body = json!({
                "contents": contents,
                "generationConfig": {
                    "thinkingConfig": { "thinkingBudget": thinking_budget },
                }
            });
            if !system.is_empty() {
                body["systemInstruction"] = json!({
                    "parts": [{ "text": system }]
                });
            }
            if !request.tools.is_empty() {
                body["tools"] = json!([{
                    "functionDeclarations": request.tools.iter().map(gemini_tool_to_json).collect::<Vec<_>>()
                }]);
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
            if let Some(fc) = part.get("functionCall") {
                let name = fc
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args = fc.get("args").cloned().unwrap_or(json!({}));
                let id = format!("gemini-{}", uuid_hex());
                events.push(Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                    id,
                    tool_name: name,
                    input: args,
                })));
            } else if let Some(text) = part.get("text").and_then(Value::as_str) {
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
            ModelRole::User => {
                if !message.content_parts.is_empty() {
                    // Multimodal message: emit Gemini-native inlineData parts.
                    let parts: Vec<Value> = message
                        .content_parts
                        .iter()
                        .map(|part| match part {
                            ContentPart::Text { text } => json!({ "text": text }),
                            ContentPart::Image { media_type, data } => json!({
                                "inlineData": {
                                    "mimeType": media_type,
                                    "data": data,
                                }
                            }),
                        })
                        .collect();
                    contents.push(json!({
                        "role": "user",
                        "parts": parts,
                    }));
                } else {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{ "text": message.content }],
                    }));
                }
            }
            ModelRole::Tool => {
                let function_name = message.tool_name.as_deref().unwrap_or("");
                let response_value: Value = serde_json::from_str(&message.content)
                    .unwrap_or_else(|_| json!({ "result": message.content }));
                contents.push(json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "name": function_name,
                            "response": response_value,
                        }
                    }],
                }));
            }
            ModelRole::Assistant => {
                let mut parts: Vec<Value> = Vec::new();
                if !message.content.is_empty() {
                    parts.push(json!({ "text": message.content }));
                }
                for tc in &message.tool_calls {
                    parts.push(json!({
                        "functionCall": {
                            "name": tc.tool_name,
                            "args": tc.input,
                        }
                    }));
                }
                contents.push(json!({
                    "role": "model",
                    "parts": parts,
                }));
            }
        }
    }

    (system.join("\n\n"), contents)
}

fn gemini_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

pub(crate) fn encode_model_for_url(model: &str) -> String {
    model
        .replace('/', "%2F")
        .replace(':', "%3A")
        .replace(' ', "%20")
}

fn uuid_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", t)
}
