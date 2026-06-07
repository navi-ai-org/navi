use crate::errors::ProviderError;
use crate::mapping::{text_delta, usage_from_value};
use crate::sse::SseDecoder;
use crate::transport::ensure_success;
use anyhow::Result;
use async_stream::try_stream;
use futures_util::StreamExt;
use navi_core::{
    ModelMessage, ModelRequest, ModelRole, ModelStream, ModelStreamEvent, ToolDefinition,
    ToolInvocation,
};
use serde_json::{Value, json};
use std::time::Duration;

impl crate::provider::OpenAiProvider {
    pub(crate) fn stream_anthropic_messages(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();
        let behavior = self.behavior.clone();

        Box::pin(try_stream! {
            let headers = behavior.build_headers(
                &api_key,
                crate::providers::behavior::Endpoint::AnthropicMessages,
            )?;
            let model = request.model.clone();
            tracing::info!(provider = %provider_id, model = %model, api = "anthropic-messages", tools = request.tools.len(), "provider stream started");
            let (system, messages) = anthropic_messages(&request.messages);
            let thinking = request.thinking.to_thinking_request();
            let budget = thinking.budget_tokens.unwrap_or(0);
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
            if thinking.enabled {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            }
            if !request.tools.is_empty() {
                body["tools"] = json!(request.tools.iter().map(anthropic_tool_to_json).collect::<Vec<_>>());
            }

            let response = client
                .post(format!("{}/messages", base_url.trim_end_matches('/')))
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(ProviderError::Transport)?;

            tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut chunks = response.bytes_stream();
            let mut tool_state = AnthropicToolState::default();

            let idle_timeout = Duration::from_millis(stream_idle_timeout_ms);
            loop {
                let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
                match next_chunk {
                    Ok(Some(chunk_res)) => {
                        let bytes = chunk_res.map_err(ProviderError::Transport)?;
                        for data in decoder.push_bytes(bytes.as_ref()) {
                            for event in parse_anthropic_sse_with_state(&data, &mut tool_state) {
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
}

// ── Anthropic tool state accumulator ──────────────────────────────────────────

#[derive(Default)]
pub(crate) struct AnthropicToolState {
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_json_buf: String,
}

// ── Anthropic SSE parsing ────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) fn parse_anthropic_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    parse_anthropic_sse_with_state(data, &mut AnthropicToolState::default())
}

pub(crate) fn parse_anthropic_sse_with_state(
    data: &str,
    state: &mut AnthropicToolState,
) -> Vec<Result<ModelStreamEvent>> {
    let value = match serde_json::from_str::<Value>(data) {
        Ok(value) => value,
        Err(err) => return vec![Err(err.into())],
    };

    match value.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let block = value.get("content_block");
            if let Some(block_type) = block.and_then(|b| b.get("type")).and_then(Value::as_str)
                && block_type == "tool_use"
            {
                state.current_tool_id = block
                    .and_then(|b| b.get("id"))
                    .and_then(Value::as_str)
                    .map(String::from);
                state.current_tool_name = block
                    .and_then(|b| b.get("name"))
                    .and_then(Value::as_str)
                    .map(String::from);
                state.current_json_buf.clear();
            }
            Vec::new()
        }
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
            Some("input_json_delta") => {
                if let Some(partial) = value
                    .get("delta")
                    .and_then(|delta| delta.get("partial_json"))
                    .and_then(Value::as_str)
                {
                    state.current_json_buf.push_str(partial);
                }
                Vec::new()
            }
            Some("signature_delta") => {
                vec![Ok(ModelStreamEvent::Status {
                    label: "thinking".to_string(),
                })]
            }
            _ => Vec::new(),
        },
        Some("content_block_stop") => {
            if let (Some(id), Some(name)) =
                (state.current_tool_id.take(), state.current_tool_name.take())
            {
                let input: Value = serde_json::from_str(&state.current_json_buf)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                state.current_json_buf.clear();
                vec![Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                    id,
                    tool_name: name,
                    input,
                }))]
            } else {
                Vec::new()
            }
        }
        Some("message_delta") => usage_from_value(value.get("usage")),
        Some("message_stop") => vec![Ok(ModelStreamEvent::Done)],
        Some("error") => vec![Err(anyhow::anyhow!(
            "{}",
            value.get("error").unwrap_or(&value)
        ))],
        _ => Vec::new(),
    }
}

// ── Anthropic message conversion ─────────────────────────────────────────────

pub(crate) fn anthropic_messages(messages: &[ModelMessage]) -> (String, Vec<Value>) {
    let mut system = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role {
            ModelRole::System => system.push(message.content.clone()),
            ModelRole::User => converted.push(json!({
                "role": "user",
                "content": message.content,
            })),
            ModelRole::Tool => {
                let tool_use_id = message.tool_call_id.as_deref().unwrap_or("");
                converted.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": message.content,
                    }],
                }));
            }
            ModelRole::Assistant => {
                let mut content: Vec<Value> = Vec::new();
                if !message.content.is_empty() {
                    content.push(json!({ "type": "text", "text": message.content }));
                }
                for tc in &message.tool_calls {
                    content.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.tool_name,
                        "input": tc.input,
                    }));
                }
                converted.push(json!({
                    "role": "assistant",
                    "content": content,
                }));
            }
        }
    }

    (system.join("\n\n"), converted)
}

// ── Anthropic tool definition conversion ─────────────────────────────────────

fn anthropic_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}
