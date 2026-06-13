use crate::errors::ProviderError;
use crate::mapping::{
    apply_thinking_to_body, chat_tool_to_json, message_to_json, reasoning_text,
    responses_input_item_to_json, responses_tool_to_json, text_delta, thinking_request_for_api,
    usage_from_value,
};
use crate::sse::SseDecoder;
use crate::transport::ensure_success;
use crate::types::OpenAiApiKind;
use anyhow::Result;
use async_stream::try_stream;
use futures_util::StreamExt;
use navi_core::{ModelRequest, ModelStream, ModelStreamEvent, ToolInvocation};
use serde_json::{Value, json};
use std::time::Duration;

impl crate::provider::OpenAiProvider {
    pub(crate) fn stream_responses(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();
        let request_options = self.config.request_options.clone().unwrap_or_default();
        let behavior = self.behavior.clone();

        Box::pin(try_stream! {
        let headers = behavior
            .build_headers(&api_key, crate::providers::behavior::Endpoint::Responses)?;
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
            thinking_request_for_api(request.thinking, OpenAiApiKind::Responses, &provider_id),
            OpenAiApiKind::Responses,
            &provider_id,
        );
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
        if let Some(prompt_cache_key) = &request_options.prompt_cache_key {
            body["prompt_cache_key"] = json!(prompt_cache_key);
        }
        if let Some(retention) = &request_options.prompt_cache_retention
            && should_send_prompt_cache_retention(&model, retention)
        {
            body["prompt_cache_retention"] = json!(retention);
        }

        let response = client
            .post(format!("{}/responses", base_url.trim_end_matches('/')))
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Transport)?;

        tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
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

    pub(crate) fn stream_chat_completions(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();
        let request_options = self.config.request_options.clone().unwrap_or_default();
        let behavior = self.behavior.clone();

        Box::pin(try_stream! {
        let headers = behavior.build_headers(
            &api_key,
            crate::providers::behavior::Endpoint::ChatCompletions,
        )?;
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
            thinking_request_for_api(request.thinking, OpenAiApiKind::ChatCompletions, &provider_id),
            OpenAiApiKind::ChatCompletions,
            &provider_id,
        );
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
        if let Some(prompt_cache_key) = &request_options.prompt_cache_key {
            body["prompt_cache_key"] = json!(prompt_cache_key);
        }
        if let Some(retention) = &request_options.prompt_cache_retention
            && should_send_prompt_cache_retention(&model, retention)
        {
            body["prompt_cache_retention"] = json!(retention);
        }

        let req = client
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .headers(headers);

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

        let idle_timeout = Duration::from_millis(stream_idle_timeout_ms);
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
}

pub(crate) fn parse_openai_responses_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
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
        .unwrap_or_else(|| serde_json::json!({}));
    Some(ToolInvocation {
        id,
        tool_name,
        input,
    })
}

#[cfg(test)]
pub(crate) fn parse_chat_completions_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    parse_chat_completions_sse_with_state(data, &mut ChatToolCallAccumulator::default())
}

pub(crate) fn parse_chat_completions_sse_with_state(
    data: &str,
    tool_calls: &mut ChatToolCallAccumulator,
) -> Vec<Result<ModelStreamEvent>> {
    if data == "[DONE]" {
        let mut events = tool_calls.drain_pending_text();
        events.push(Ok(ModelStreamEvent::Done));
        return events;
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
            events.extend(tool_calls.push_content(content));
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
    }
    events
}

#[derive(Default)]
pub(crate) struct ChatToolCallAccumulator {
    calls: Vec<PartialChatToolCall>,
    think_tags: ThinkTagSplitter,
}

#[derive(Default)]
struct PartialChatToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatToolCallAccumulator {
    fn push_content(&mut self, content: &str) -> Vec<Result<ModelStreamEvent>> {
        self.think_tags.push(content)
    }

    fn drain_pending_text(&mut self) -> Vec<Result<ModelStreamEvent>> {
        self.think_tags.drain_pending()
    }

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
                    serde_json::json!({
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

#[derive(Default)]
struct ThinkTagSplitter {
    in_think: bool,
    pending: String,
}

impl ThinkTagSplitter {
    fn push(&mut self, content: &str) -> Vec<Result<ModelStreamEvent>> {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(content);
        self.split(&input, false)
    }

    fn drain_pending(&mut self) -> Vec<Result<ModelStreamEvent>> {
        let pending = std::mem::take(&mut self.pending);
        let tag = if self.in_think { "</think>" } else { "<think>" };
        if is_partial_tag_prefix(&pending, tag) {
            return Vec::new();
        }
        self.split(&pending, true)
    }

    fn split(&mut self, input: &str, final_chunk: bool) -> Vec<Result<ModelStreamEvent>> {
        let mut events = Vec::new();
        let mut remaining = input;

        while !remaining.is_empty() {
            let tag = if self.in_think { "</think>" } else { "<think>" };
            if let Some(pos) = find_ascii_case_insensitive(remaining, tag) {
                self.push_segment(&mut events, &remaining[..pos]);
                remaining = &remaining[pos + tag.len()..];
                self.in_think = !self.in_think;
                continue;
            }

            let keep = if final_chunk {
                0
            } else {
                partial_tag_suffix_len(remaining, tag)
            };
            let emit_len = remaining.len().saturating_sub(keep);
            self.push_segment(&mut events, &remaining[..emit_len]);
            self.pending.push_str(&remaining[emit_len..]);
            break;
        }

        events
    }

    fn push_segment(&self, events: &mut Vec<Result<ModelStreamEvent>>, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.in_think {
            events.push(Ok(ModelStreamEvent::ThinkingDelta {
                text: text.to_string(),
            }));
        } else {
            events.push(text_delta(text));
        }
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn partial_tag_suffix_len(text: &str, tag: &str) -> usize {
    let bytes = text.as_bytes();
    let tag_bytes = tag.as_bytes();
    let max_len = bytes.len().min(tag_bytes.len().saturating_sub(1));
    for len in (1..=max_len).rev() {
        if bytes[bytes.len() - len..].eq_ignore_ascii_case(&tag_bytes[..len]) {
            return len;
        }
    }
    0
}

fn is_partial_tag_prefix(text: &str, tag: &str) -> bool {
    !text.is_empty()
        && text.len() < tag.len()
        && tag.as_bytes()[..text.len()].eq_ignore_ascii_case(text.as_bytes())
}

/// Returns `true` if the model supports OpenAI's extended prompt cache retention (24h).
pub(crate) fn model_supports_extended_cache(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    // gpt-5.5, gpt-5.5-pro, gpt-5.4, gpt-5.2, gpt-5.1*, gpt-5, gpt-5-codex, gpt-4.1
    m.starts_with("gpt-5") || m.starts_with("gpt-4.1") || m.starts_with("o4") || m.starts_with("o3")
}

fn should_send_prompt_cache_retention(model: &str, retention: &str) -> bool {
    retention != "24h" || model_supports_extended_cache(model)
}
