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
        .map(parse_tool_arguments)
        .unwrap_or_else(|| serde_json::json!({}));
    Some(ToolInvocation {
        id,
        tool_name,
        input,
    })
}

fn parse_tool_arguments(arguments: &str) -> Value {
    serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| {
        serde_json::json!({
            "raw_arguments": arguments,
        })
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
    tool_call_extractor: TextToolCallExtractor,
}

#[derive(Default)]
struct PartialChatToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ChatToolCallAccumulator {
    fn push_content(&mut self, content: &str) -> Vec<Result<ModelStreamEvent>> {
        let clean_text = self.tool_call_extractor.push_text(content);
        let mut events = self.tool_call_extractor.take_tool_call_events();
        events.extend(self.think_tags.push(&clean_text));
        events
    }

    fn drain_pending_text(&mut self) -> Vec<Result<ModelStreamEvent>> {
        let clean_text = self.tool_call_extractor.drain_pending_text();
        let mut events = self.tool_call_extractor.take_tool_call_events();
        if !clean_text.is_empty() {
            events.extend(self.think_tags.push(&clean_text));
        }
        events.extend(self.think_tags.drain_pending());
        events
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
struct TextToolCallExtractor {
    pending: String,
    in_tool_call: bool,
    next_tool_call_index: u64,
    tool_call_events: Vec<Result<ModelStreamEvent>>,
}

impl TextToolCallExtractor {
    fn push_text(&mut self, text: &str) -> String {
        self.pending.push_str(text);
        self.drain(false)
    }

    fn drain_pending_text(&mut self) -> String {
        self.drain(true)
    }

    fn take_tool_call_events(&mut self) -> Vec<Result<ModelStreamEvent>> {
        std::mem::take(&mut self.tool_call_events)
    }

    fn drain(&mut self, final_chunk: bool) -> String {
        let mut clean_text = String::new();

        loop {
            if self.in_tool_call {
                if let Some(end) = find_ascii_case_insensitive(&self.pending, "</tool_call>") {
                    let block = self.pending[..end].to_string();
                    self.pending.drain(..end + "</tool_call>".len());
                    self.in_tool_call = false;
                    let calls = self.parse_tool_call_block(&block);
                    self.tool_call_events.extend(calls);
                    continue;
                }

                if final_chunk {
                    let block = std::mem::take(&mut self.pending);
                    self.in_tool_call = false;
                    let calls = self.parse_tool_call_block(&block);
                    self.tool_call_events.extend(calls);
                }
                break;
            }

            if let Some((start, marker_len)) = find_tool_call_start(&self.pending) {
                if start > 0 {
                    clean_text.push_str(&self.pending[..start]);
                }
                self.pending.drain(..start + marker_len);
                self.in_tool_call = true;
                continue;
            }

            let keep = if final_chunk {
                0
            } else {
                partial_tool_call_start_suffix_len(&self.pending)
            };
            let emit_len = self.pending.len().saturating_sub(keep);
            if emit_len > 0 {
                clean_text.push_str(&self.pending[..emit_len]);
                self.pending.drain(..emit_len);
            }
            break;
        }

        clean_text
    }

    fn parse_tool_call_block(&mut self, block: &str) -> Vec<Result<ModelStreamEvent>> {
        parse_tool_call_values(block)
            .into_iter()
            .filter_map(|value| self.tool_invocation_from_value(value))
            .map(|invocation| Ok(ModelStreamEvent::ToolCall(invocation)))
            .collect()
    }

    fn tool_invocation_from_value(&mut self, value: Value) -> Option<ToolInvocation> {
        let tool_name = value
            .get("name")
            .or_else(|| value.get("toolName"))
            .or_else(|| value.get("tool_name"))
            .and_then(Value::as_str)?
            .to_string();
        let id = value
            .get("id")
            .or_else(|| value.get("toolCallId"))
            .or_else(|| value.get("tool_call_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let id = format!("text-tool-{}", self.next_tool_call_index);
                self.next_tool_call_index += 1;
                id
            });
        let input = value
            .get("arguments")
            .or_else(|| value.get("args"))
            .or_else(|| value.get("input"))
            .map(normalize_tool_input)
            .unwrap_or_else(|| json!({}));

        Some(ToolInvocation {
            id,
            tool_name,
            input,
        })
    }
}

fn parse_tool_call_values(block: &str) -> Vec<Value> {
    let trimmed = block.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return match value {
            Value::Array(values) => values,
            value => vec![value],
        };
    }
    let stream = serde_json::Deserializer::from_str(trimmed).into_iter::<Value>();
    stream.filter_map(std::result::Result::ok).collect()
}

fn normalize_tool_input(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| value.clone())
        }
        value => value.clone(),
    }
}

fn find_tool_call_start(text: &str) -> Option<(usize, usize)> {
    let patterns: &[&str] = &[
        "]<]minimax[>[<tool_call>",
        "<]minimax[>[<tool_call>",
        "]<|minimal|>[<tool_call>",
        "<|minimal|>[<tool_call>",
        "<tool_call>",
    ];

    if let Some(result) = patterns
        .iter()
        .filter_map(|marker| {
            find_ascii_case_insensitive(text, marker).map(|pos| (pos, marker.len()))
        })
        .min_by_key(|(pos, _)| *pos)
    {
        return Some(result);
    }

    find_generic_bracket_tool_call_prefix(text)
}

fn find_generic_bracket_tool_call_prefix(text: &str) -> Option<(usize, usize)> {
    let tc_pos = find_ascii_case_insensitive(text, "<tool_call>")?;
    let before = &text[..tc_pos];
    let bracket_end = before.rfind(">[")?;
    if bracket_end > before.len().saturating_sub(64) && bracket_end >= 1 {
        let candidate = &before[..bracket_end];
        let openers = [']', '<', '|'];
        if let Some(prefix_start) = candidate.rfind(|c: char| openers.contains(&c)) {
            let full_len = tc_pos + "<tool_call>".len() - prefix_start;
            return Some((prefix_start, full_len));
        }
    }
    None
}

fn partial_tool_call_start_suffix_len(text: &str) -> usize {
    let patterns: &[&str] = &[
        "]<]minimax[>[<tool_call>",
        "<]minimax[>[<tool_call>",
        "]<|minimal|>[<tool_call>",
        "<|minimal|>[<tool_call>",
        "<tool_call>",
    ];

    let specific = patterns
        .iter()
        .map(|marker| partial_tag_suffix_len(text, marker))
        .max()
        .unwrap_or(0);

    let generic = partial_generic_bracket_suffix_len(text);
    specific.max(generic)
}

fn partial_generic_bracket_suffix_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let needle = b">[<tool_call>";
    if bytes.len() < 3 {
        return 0;
    }
    if bytes.ends_with(b"<")
        || bytes.ends_with(b"<t")
        || bytes.ends_with(b"<to")
        || bytes.ends_with(b"<too")
        || bytes.ends_with(b"<tool")
    {
        return 1;
    }
    let max_len = bytes.len().min(needle.len());
    for len in (3..=max_len).rev() {
        let suffix = &bytes[bytes.len() - len..];
        if suffix.eq_ignore_ascii_case(&needle[..len]) {
            return len;
        }
    }
    0
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
