use crate::errors::ProviderError;
use crate::mapping::text_delta;
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

const RUNTIME_CONTEXT_MARKER: &str = "=== Runtime Context ===";
/// Anthropic allows at most 4 explicit cache breakpoints per request.
/// We budget them as: tools(1) + system(1) + last tool result(1) + last user message(1).
const MAX_CACHE_BREAKPOINTS: usize = 4;

impl crate::provider::OpenAiProvider {
    pub(crate) fn stream_anthropic_messages(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();
        let cache_control = self
            .config
            .request_options
            .as_ref()
            .and_then(|opts| opts.anthropic_cache_control.clone());
        let behavior = self.behavior.clone();

        Box::pin(try_stream! {
            let headers = behavior.build_headers(
                &api_key,
                crate::providers::behavior::Endpoint::AnthropicMessages,
            )?;
            let model = request.model.clone();
            tracing::info!(provider = %provider_id, model = %model, api = "anthropic-messages", tools = request.tools.len(), "provider stream started");
            let (mut system, messages) =
                anthropic_messages_with_cache_control(&request.messages, cache_control.as_ref());
            // Prepend the stable base instructions as the first system block
            // with cache_control so the prefix is cached independently of
            // the dynamic developer blocks that follow.
            if let Some(instructions) = &request.instructions {
                if !instructions.is_empty() {
                    let mut block = json!({ "type": "text", "text": instructions });
                    if let Some(cache_control) = &cache_control {
                        block["cache_control"] = cache_control.clone();
                    }
                    system.insert(0, block);
                }
            }
            let thinking = request.thinking.to_thinking_request();
            let budget = thinking.budget_tokens.unwrap_or(0);
            let max_tokens = (budget + 1024).max(4096);
            let mut body = json!({
                "model": request.model,
                "max_tokens": max_tokens,
                "stream": true,
                "messages": messages,
            });
            if let Some(cache_control) = &cache_control {
                body["cache_control"] = cache_control.clone();
            }
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            if thinking.enabled {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            }
            if !request.tools.is_empty() {
                body["tools"] = json!(anthropic_tools_to_json_with_cache_control(
                    &request.tools,
                    cache_control.as_ref()
                ));
            }

            let url = if base_url.ends_with("/v1") {
                format!("{}/messages", base_url.trim_end_matches('/'))
            } else {
                format!("{}/v1/messages", base_url.trim_end_matches('/'))
            };
            let response = client
                .post(url)
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
        Some("message_delta") => crate::mapping::usage_from_value_with_behavior(
            value.get("usage"),
            Some(&crate::providers::behavior::AnthropicBehavior),
        ),
        Some("message_stop") => vec![Ok(ModelStreamEvent::Done)],
        Some("error") => vec![Err(anyhow::anyhow!(
            "{}",
            value.get("error").unwrap_or(&value)
        ))],
        _ => Vec::new(),
    }
}

// ── Anthropic message conversion ─────────────────────────────────────────────

/// Converts NAVI messages into Anthropic Messages API format.
///
/// Manages explicit cache breakpoints (max 4 per Anthropic limit):
///   1. Last tool definition (set in `anthropic_tools_to_json`)
///   2. Stable system prefix (before RUNTIME_CONTEXT_MARKER)
///   3. Last tool result
///   4. Last stable user message (before the current turn)
///
/// The top-level `cache_control` in the request body handles automatic caching
/// for the remaining content.
#[cfg(test)]
pub(crate) fn anthropic_messages(messages: &[ModelMessage]) -> (Vec<Value>, Vec<Value>) {
    let cache_control = default_anthropic_cache_control();
    anthropic_messages_with_cache_control(messages, Some(&cache_control))
}

pub(crate) fn anthropic_messages_with_cache_control(
    messages: &[ModelMessage],
    cache_control: Option<&Value>,
) -> (Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let caching_enabled = cache_control.is_some();

    // Pre-compute indices for breakpoint placement.
    let last_tool_index = messages.iter().rposition(|m| m.role == ModelRole::Tool);
    // Last user message that is NOT at the end of the conversation
    // (i.e., a stable user message from a previous turn).
    let last_stable_user_index = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(i, m)| {
            m.role == ModelRole::User
                && *i + 1 < messages.len()
                && !matches!(messages[*i + 1].role, ModelRole::User)
        })
        .map(|(i, _)| i);

    // Count system blocks that want caching to pre-allocate budget.
    let system_cache_count = if caching_enabled {
        messages
            .iter()
            .filter(|m| m.role == ModelRole::System || m.role == ModelRole::Developer)
            .map(|m| {
                if let Some((stable, _)) = m.content.split_once(RUNTIME_CONTEXT_MARKER) {
                    if stable.trim_end().is_empty() { 0 } else { 1 }
                } else {
                    1
                }
            })
            .sum::<usize>()
            .min(MAX_CACHE_BREAKPOINTS)
    } else {
        0
    };

    // Remaining budget after system blocks.
    let remaining = MAX_CACHE_BREAKPOINTS.saturating_sub(system_cache_count);
    // Tool result has higher priority than user message.
    let cache_tool_result = caching_enabled && remaining >= 1 && last_tool_index.is_some();
    let cache_user = caching_enabled && remaining >= 2 && last_stable_user_index.is_some();

    // ── System messages ───────────────────────────────────────────────────
    let mut system_breakpoints = 0usize;
    for message in messages {
        if message.role != ModelRole::System && message.role != ModelRole::Developer {
            continue;
        }
        if let Some((stable, dynamic)) = message.content.split_once(RUNTIME_CONTEXT_MARKER) {
            let stable = stable.trim_end();
            if !stable.is_empty() {
                let cache = system_breakpoints < system_cache_count;
                let mut block = json!({ "type": "text", "text": stable });
                if cache {
                    if let Some(cache_control) = cache_control {
                        block["cache_control"] = cache_control.clone();
                    }
                    system_breakpoints += 1;
                }
                system.push(block);
            }
            // Dynamic tail (runtime context) — never cached.
            system.push(json!({
                "type": "text",
                "text": format!("{RUNTIME_CONTEXT_MARKER}{dynamic}"),
            }));
        } else {
            let cache = system_breakpoints < system_cache_count;
            let mut block = json!({ "type": "text", "text": message.content });
            if cache {
                if let Some(cache_control) = cache_control {
                    block["cache_control"] = cache_control.clone();
                }
                system_breakpoints += 1;
            }
            system.push(block);
        }
    }

    // ── Conversation messages ─────────────────────────────────────────────
    let mut converted = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message.role {
            ModelRole::System | ModelRole::Developer => {} // already handled above
            ModelRole::User => {
                if !message.content_parts.is_empty() {
                    // Multimodal message: emit Anthropic-native content blocks.
                    let mut blocks: Vec<Value> = message
                        .content_parts
                        .iter()
                        .map(|part| match part {
                            ContentPart::Text { text } => {
                                json!({ "type": "text", "text": text })
                            }
                            ContentPart::Image { media_type, data } => {
                                json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    }
                                })
                            }
                            ContentPart::Audio { media_type, name, .. } => json!({
                                "type": "text",
                                "text": attachment_placeholder("audio", media_type, name.as_deref())
                            }),
                            ContentPart::Video { media_type, name, .. } => json!({
                                "type": "text",
                                "text": attachment_placeholder("video", media_type, name.as_deref())
                            }),
                            ContentPart::Document {
                                media_type,
                                data,
                                name,
                            } => {
                                if *media_type == "application/pdf" {
                                    json!({
                                        "type": "document",
                                        "source": {
                                            "type": "base64",
                                            "media_type": media_type,
                                            "data": data,
                                        }
                                    })
                                } else {
                                    json!({
                                        "type": "text",
                                        "text": attachment_placeholder("document", media_type, name.as_deref())
                                    })
                                }
                            }
                        })
                        .collect();
                    // Cache last stable user message.
                    if Some(index) == last_stable_user_index
                        && cache_user
                        && let Some(cache_control) = cache_control
                        && let Some(last) = blocks.last_mut()
                    {
                        last["cache_control"] = cache_control.clone();
                    }
                    converted.push(json!({
                        "role": "user",
                        "content": blocks,
                    }));
                } else {
                    let mut msg = json!({
                        "role": "user",
                        "content": message.content,
                    });
                    // Cache the last stable user message (previous turn).
                    if Some(index) == last_stable_user_index
                        && cache_user
                        && let Some(cache_control) = cache_control
                    {
                        let content = json!({
                            "type": "text",
                            "text": message.content,
                            "cache_control": cache_control,
                        });
                        msg["content"] = json!([content]);
                    }
                    converted.push(msg);
                }
            }
            ModelRole::Tool => {
                let tool_use_id = message.tool_call_id.as_deref().unwrap_or("");
                // Anthropic accepts images inside tool_result content blocks.
                let content: Value = if message.content_parts.iter().any(|p| p.is_image()) {
                    let mut blocks = vec![json!({ "type": "text", "text": message.content })];
                    for part in &message.content_parts {
                        if let ContentPart::Image { media_type, data } = part {
                            blocks.push(json!({
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": media_type,
                                    "data": data,
                                }
                            }));
                        }
                    }
                    Value::Array(blocks)
                } else {
                    Value::String(message.content.clone())
                };
                let mut tool_result = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": content,
                });
                // Cache the last tool result.
                if Some(index) == last_tool_index
                    && cache_tool_result
                    && let Some(cache_control) = cache_control
                {
                    tool_result["cache_control"] = cache_control.clone();
                }
                converted.push(json!({
                    "role": "user",
                    "content": [tool_result],
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

    (system, converted)
}

// ── Anthropic tool definition conversion ─────────────────────────────────────

/// Converts tool definitions to Anthropic format.
/// Places `cache_control` on the last tool definition to cache the full tools prefix.
#[cfg(test)]
pub(crate) fn anthropic_tools_to_json(tools: &[ToolDefinition]) -> Vec<Value> {
    let cache_control = default_anthropic_cache_control();
    anthropic_tools_to_json_with_cache_control(tools, Some(&cache_control))
}

pub(crate) fn anthropic_tools_to_json_with_cache_control(
    tools: &[ToolDefinition],
    cache_control: Option<&Value>,
) -> Vec<Value> {
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| anthropic_tool_to_json(tool, index + 1 == tools.len(), cache_control))
        .collect()
}

fn anthropic_tool_to_json(
    tool: &ToolDefinition,
    cache: bool,
    cache_control: Option<&Value>,
) -> Value {
    let mut value = json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    });
    if cache && let Some(cache_control) = cache_control {
        value["cache_control"] = cache_control.clone();
    }
    value
}

fn attachment_placeholder(kind: &str, media_type: &str, name: Option<&str>) -> String {
    match name {
        Some(name) if !name.is_empty() => {
            format!("[{kind} attachment omitted from this provider request: {name} ({media_type})]")
        }
        _ => format!("[{kind} attachment omitted from this provider request: {media_type}]"),
    }
}

#[cfg(test)]
fn default_anthropic_cache_control() -> Value {
    json!({ "type": "ephemeral" })
}
