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
    /// Last arguments length we emitted a progress event for.
    last_progress_args: usize,
}

/// Emit argument-stream progress every N chars (plus on first arg byte).
const TOOL_CALL_PROGRESS_ARG_STEP: usize = 256;

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
        // Anthropic reports prompt/cache usage at message start, then reports
        // the cumulative completion count in message_delta. Keeping both
        // events lets NAVI update the context meter immediately and reconcile
        // the final output later.
        Some("message_start") => crate::mapping::usage_from_value_with_behavior(
            value
                .get("message")
                .and_then(|message| message.get("usage")),
            Some(&crate::providers::behavior::AnthropicBehavior),
        ),
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
                state.last_progress_args = 0;
                if let Some(name) = state.current_tool_name.clone() {
                    if !name.is_empty() {
                        return vec![Ok(ModelStreamEvent::ToolCallProgress {
                            id: state.current_tool_id.clone(),
                            tool_name: name,
                            arguments_chars: 0,
                        })];
                    }
                }
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
                let args_len = state.current_json_buf.len();
                let should_emit = state.current_tool_name.is_some()
                    && (args_len.saturating_sub(state.last_progress_args)
                        >= TOOL_CALL_PROGRESS_ARG_STEP
                        || (args_len > 0 && state.last_progress_args == 0));
                if should_emit {
                    state.last_progress_args = args_len;
                    if let Some(name) = state.current_tool_name.clone() {
                        return vec![Ok(ModelStreamEvent::ToolCallProgress {
                            id: state.current_tool_id.clone(),
                            tool_name: name,
                            arguments_chars: args_len,
                        })];
                    }
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
                state.last_progress_args = 0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_anthropic_sse_returns_error_for_invalid_json() {
        let events = parse_anthropic_sse("not-json");
        assert_eq!(events.len(), 1);
        assert!(events[0].is_err());
    }

    #[test]
    fn parse_anthropic_sse_content_block_start_tool_use_emits_progress() {
        let data = r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"tu_1","name":"read_file"}}"#;
        let events = parse_anthropic_sse(data);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].as_ref().unwrap(),
            ModelStreamEvent::ToolCallProgress { id, tool_name, arguments_chars: 0 }
                if id.as_deref() == Some("tu_1") && tool_name == "read_file"
        ));
    }

    #[test]
    fn parse_anthropic_sse_content_block_start_tool_use_with_empty_name_is_ignored() {
        let data = r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"tu_1","name":""}}"#;
        let events = parse_anthropic_sse(data);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_anthropic_sse_thinking_delta_without_text_emits_status() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"thinking_delta"}}"#;
        let events = parse_anthropic_sse(data);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].as_ref().unwrap(),
            ModelStreamEvent::Status { label } if label == "thinking"
        ));
    }

    #[test]
    fn parse_anthropic_sse_signature_delta_emits_thinking_status() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"signature_delta"}}"#;
        let events = parse_anthropic_sse(data);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0].as_ref().unwrap(),
            ModelStreamEvent::Status { label } if label == "thinking"
        ));
    }

    #[test]
    fn parse_anthropic_sse_unknown_content_block_delta_is_ignored() {
        let data = r#"{"type":"content_block_delta","delta":{"type":"unknown_delta"}}"#;
        let events = parse_anthropic_sse(data);
        assert!(events.is_empty());
    }

    #[test]
    fn parse_anthropic_sse_error_returns_error() {
        let data = r#"{"type":"error","error":{"message":"boom"}}"#;
        let events = parse_anthropic_sse(data);
        assert_eq!(events.len(), 1);
        assert!(events[0].is_err());
    }

    #[test]
    fn anthropic_messages_split_system_with_runtime_context_marker() {
        use navi_core::{ModelMessage, ModelRole};
        let messages = vec![ModelMessage {
            role: ModelRole::System,
            content: "Stable prefix.\n\n=== Runtime Context ===\nDynamic tail.".into(),
            content_parts: vec![],
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }];
        let cache_control = default_anthropic_cache_control();
        let (system, converted) =
            anthropic_messages_with_cache_control(&messages, Some(&cache_control));
        assert_eq!(system.len(), 2);
        assert_eq!(system[0]["text"], "Stable prefix.");
        assert!(system[0]["cache_control"].is_object());
        assert_eq!(system[1]["text"], "=== Runtime Context ===\nDynamic tail.");
        assert!(converted.is_empty());
    }

    #[test]
    fn anthropic_messages_user_with_audio_video_and_document_parts() {
        use navi_core::{ContentPart, ModelMessage, ModelRole};
        let messages = vec![ModelMessage {
            role: ModelRole::User,
            content: "analyze".into(),
            content_parts: vec![
                ContentPart::Audio {
                    media_type: "audio/mpeg".into(),
                    data: "audiodata".into(),
                    name: Some("clip.mp3".into()),
                },
                ContentPart::Video {
                    media_type: "video/mp4".into(),
                    data: "videodata".into(),
                    name: Some("clip.mp4".into()),
                },
                ContentPart::Document {
                    media_type: "application/pdf".into(),
                    data: "pdfdata".into(),
                    name: Some("doc.pdf".into()),
                },
                ContentPart::Document {
                    media_type: "text/plain".into(),
                    data: "textdata".into(),
                    name: Some("notes.txt".into()),
                },
            ],
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }];
        let (system, converted) = anthropic_messages(&messages);
        let content = converted[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 4);
        assert_eq!(content[0]["type"], "text");
        assert!(content[0]["text"].as_str().unwrap().contains("audio"));
        assert_eq!(content[1]["type"], "text");
        assert!(content[1]["text"].as_str().unwrap().contains("video"));
        assert_eq!(content[2]["type"], "document");
        assert_eq!(content[2]["source"]["media_type"], "application/pdf");
        assert_eq!(content[3]["type"], "text");
        assert!(content[3]["text"].as_str().unwrap().contains("document"));
        assert!(system.is_empty());
    }

    #[test]
    fn anthropic_messages_tool_result_with_image() {
        use navi_core::{ContentPart, ModelMessage, ModelRole};
        let messages = vec![ModelMessage {
            role: ModelRole::Tool,
            content: "image result".into(),
            tool_name: Some("read_file".into()),
            tool_call_id: Some("tu_1".into()),
            content_parts: vec![ContentPart::Image {
                media_type: "image/png".into(),
                data: "pngdata".into(),
            }],
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }];
        let cache_control = default_anthropic_cache_control();
        let (_, converted) = anthropic_messages_with_cache_control(&messages, Some(&cache_control));
        let content = converted[0]["content"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn anthropic_messages_caches_last_stable_user_message() {
        use navi_core::ModelMessage;
        let messages = vec![
            ModelMessage::user("first"),
            ModelMessage::user("stable"),
            ModelMessage::assistant("ok"),
        ];
        let cache_control = default_anthropic_cache_control();
        let (_, converted) = anthropic_messages_with_cache_control(&messages, Some(&cache_control));
        let cached = &converted[1];
        assert_eq!(cached["role"], "user");
        let content = cached["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], "stable");
        assert!(content[0]["cache_control"].is_object());
    }

    #[test]
    fn anthropic_messages_caches_last_tool_result() {
        use navi_core::ModelMessage;
        let messages = vec![
            ModelMessage::user("run"),
            ModelMessage::assistant("done"),
            ModelMessage::tool_result("tu_1", "bash", "output"),
        ];
        let cache_control = default_anthropic_cache_control();
        let (_, converted) = anthropic_messages_with_cache_control(&messages, Some(&cache_control));
        let tool_result = &converted[2]["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        assert!(tool_result["cache_control"].is_object());
    }

    #[test]
    fn attachment_placeholder_with_and_without_name() {
        assert_eq!(
            attachment_placeholder("audio", "audio/mpeg", Some("clip.mp3")),
            "[audio attachment omitted from this provider request: clip.mp3 (audio/mpeg)]"
        );
        assert_eq!(
            attachment_placeholder("video", "video/mp4", None),
            "[video attachment omitted from this provider request: video/mp4]"
        );
    }
}
