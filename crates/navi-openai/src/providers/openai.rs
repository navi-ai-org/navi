use crate::errors::ProviderError;
use crate::mapping::{
    apply_thinking_to_body, chat_tool_to_json, message_to_json, reasoning_text,
    responses_input_item_to_json, responses_tool_to_json, text_delta,
    thinking_request_for_api_with_levels, tool_image_followup_user_message, usage_from_value,
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
        let reasoning_levels = reasoning_levels_for_model(&self.config, &request.model);

        Box::pin(try_stream! {
        let mut headers = behavior
            .build_headers(&api_key, crate::providers::behavior::Endpoint::Responses)?;
        behavior.apply_request_headers(&mut headers, &request)?;
        apply_extra_headers(&mut headers, &request_options);
        let model = request.model.clone();
        tracing::info!(provider = %provider_id, model = %model, api = "responses", tools = request.tools.len(), "provider stream started");
        let mut body = json!({
            "model": request.model,
            "input": request.messages.iter().flat_map(responses_input_item_to_json).collect::<Vec<_>>(),
        });
        // Use the `instructions` field for the stable base prompt when
        // available. This lets the provider cache the prefix independently
        // of dynamic developer messages in the input array.
        if let Some(instructions) = &request.instructions {
            if !instructions.is_empty() {
                body["instructions"] = json!(instructions);
            }
        }
        if !request.tools.is_empty() {
            // Callers (navi-core turn path) already sort tools for prefix cache.
            // Only re-sort when out of order to avoid redundant clone+sort.
            let tools = if request
                .tools
                .windows(2)
                .any(|w| w[0].name > w[1].name)
            {
                let mut tools = request.tools.clone();
                tools.sort_by(|a, b| a.name.cmp(&b.name));
                tools
            } else {
                request.tools.clone()
            };
            body["tools"] = json!(tools.iter().map(responses_tool_to_json).collect::<Vec<_>>());
            body["tool_choice"] = if requires_initial_session_title(&request) {
                json!({ "type": "function", "name": "set_session_title" })
            } else {
                json!("auto")
            };
            if behavior.supports_parallel_tool_calls(crate::providers::behavior::Endpoint::Responses) {
                body["parallel_tool_calls"] = json!(true);
            }
        }
        apply_thinking_to_body(
            &mut body,
            thinking_request_for_api_with_levels(
                request.thinking,
                OpenAiApiKind::Responses,
                &provider_id,
                &reasoning_levels,
            ),
            OpenAiApiKind::Responses,
            &provider_id,
        );
        apply_openai_request_options(
            &mut body,
            &request_options,
            OpenAiApiKind::Responses,
        );
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
        apply_prompt_cache_fields(
            &mut body,
            &provider_id,
            &model,
            &request_options,
            request.session_id.as_deref(),
        );

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
        let reasoning_levels = reasoning_levels_for_model(&self.config, &request.model);

        Box::pin(try_stream! {
        let mut headers = behavior.build_headers(
            &api_key,
            crate::providers::behavior::Endpoint::ChatCompletions,
        )?;
        behavior.apply_request_headers(&mut headers, &request)?;
        apply_extra_headers(&mut headers, &request_options);
        let model = request.model.clone();
        tracing::info!(provider = %provider_id, model = %model, api = "chat-completions", tools = request.tools.len(), "provider stream started");
        let messages_json = chat_completions_messages(&request);
        let mut body = json!({
            "model": request.model,
            "messages": messages_json,
        });
        if !request.tools.is_empty() {
            // Stable tool order is required for provider prefix caching.
            // Callers usually already sort; re-sort only when needed.
            let tools = if request
                .tools
                .windows(2)
                .any(|w| w[0].name > w[1].name)
            {
                let mut tools = request.tools.clone();
                tools.sort_by(|a, b| a.name.cmp(&b.name));
                tools
            } else {
                request.tools.clone()
            };
            body["tools"] = json!(tools.iter().map(chat_tool_to_json).collect::<Vec<_>>());
            body["tool_choice"] = if requires_initial_session_title(&request) {
                json!({
                    "type": "function",
                    "function": { "name": "set_session_title" }
                })
            } else {
                json!("auto")
            };
            if behavior.supports_parallel_tool_calls(crate::providers::behavior::Endpoint::ChatCompletions) {
                body["parallel_tool_calls"] = json!(true);
            }
        }
        apply_thinking_to_body(
            &mut body,
            thinking_request_for_api_with_levels(
                request.thinking,
                OpenAiApiKind::ChatCompletions,
                &provider_id,
                &reasoning_levels,
            ),
            OpenAiApiKind::ChatCompletions,
            &provider_id,
        );
        apply_openai_request_options(
            &mut body,
            &request_options,
            OpenAiApiKind::ChatCompletions,
        );
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });
        apply_prompt_cache_fields(
            &mut body,
            &provider_id,
            &model,
            &request_options,
            request.session_id.as_deref(),
        );

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

/// Look up registry `reasoning_levels` for the active model on this provider.
fn reasoning_levels_for_model(config: &navi_core::ProviderConfig, model_name: &str) -> Vec<String> {
    config
        .models
        .iter()
        .find(|model| model.name == model_name || model.name.eq_ignore_ascii_case(model_name))
        .map(|model| model.reasoning_levels.clone())
        .unwrap_or_default()
}

/// The session-title tool is installed only for the primary chat session. If
/// it has not produced a tool result yet, force it as the first model action.
/// This avoids a separate title-generation completion while making naming
/// deterministic on OpenAI-compatible providers such as Charm Hyper.
fn requires_initial_session_title(request: &ModelRequest) -> bool {
    request
        .tools
        .iter()
        .any(|tool| tool.name == "set_session_title")
        && !request.messages.iter().any(|message| {
            message.role == navi_core::ModelRole::Tool
                && message.tool_name.as_deref() == Some("set_session_title")
        })
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
        Some("response.output_item.added") => value
            .get("item")
            .and_then(parse_responses_tool_call_progress)
            .map(|event| vec![Ok(event)])
            .unwrap_or_default(),
        Some("response.function_call_arguments.delta") => {
            let id = value
                .get("item_id")
                .or_else(|| value.get("call_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let delta = value.get("delta").and_then(Value::as_str).unwrap_or("");
            // Without a name we still signal activity so the UI leaves idle wait.
            if name.is_empty() && delta.is_empty() {
                Vec::new()
            } else {
                vec![Ok(ModelStreamEvent::ToolCallProgress {
                    id,
                    tool_name: if name.is_empty() {
                        "tool".to_string()
                    } else {
                        name
                    },
                    arguments_chars: delta.len(),
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
        Some("response.incomplete") => {
            // The model stopped generating before finishing (e.g. max_tokens).
            // Treat as end-of-stream; downstream can still use text/tool deltas.
            vec![Ok(ModelStreamEvent::Done)]
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

fn parse_responses_tool_call_progress(item: &Value) -> Option<ModelStreamEvent> {
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    let id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let tool_name = item.get("name").and_then(Value::as_str)?.to_string();
    if tool_name.is_empty() {
        return None;
    }
    let arguments_chars = item
        .get("arguments")
        .and_then(Value::as_str)
        .map(str::len)
        .unwrap_or(0);
    Some(ModelStreamEvent::ToolCallProgress {
        id,
        tool_name,
        arguments_chars,
    })
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

/// Build Chat Completions `messages` without duplicating the base prompt.
///
/// The turn layer puts the same base text in both `request.instructions` and a
/// leading System message. Chat Completions has no separate `instructions`
/// field, so we emit the base prompt **once** as the leading system message.
/// Duplicating it doubles prompt tokens and breaks prefix caching (severe
/// credit burn on Charm Hyper / aggregators).
///
/// Developer context blocks are mapped to `system` for OpenAI-compat providers
/// that only accept classic roles.
pub(crate) fn chat_completions_messages(request: &ModelRequest) -> Vec<Value> {
    let instructions = request.instructions.as_ref().filter(|s| !s.is_empty());
    let has_instructions = instructions.is_some();
    let mut messages_json: Vec<Value> =
        Vec::with_capacity(request.messages.len().saturating_add(1));
    if let Some(content) = instructions {
        messages_json.push(json!({
            "role": "system",
            "content": content,
        }));
    }
    for message in &request.messages {
        if message.role == navi_core::ModelRole::System {
            // Already emitted via `instructions` (same text). Skip to avoid
            // double system prefix. If instructions is empty, fall through.
            if has_instructions {
                continue;
            }
        }
        let mut mapped = message_to_json(message);
        if message.role == navi_core::ModelRole::Developer {
            if let Some(obj) = mapped.as_object_mut() {
                obj.insert("role".into(), Value::String("system".into()));
            }
        }
        messages_json.push(mapped);
        // Chat Completions tool messages are text-only; attach view_image
        // (and similar) bytes as a follow-up multimodal user message.
        if let Some(followup) = tool_image_followup_user_message(message) {
            messages_json.push(message_to_json(&followup));
        }
    }
    messages_json
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
            events.extend(tool_calls.push_chunks(chunks));
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
    /// Last arguments length we emitted a progress event for.
    last_progress_args: usize,
    /// Whether we already announced the tool name via ToolCallProgress.
    name_emitted: bool,
}

/// Emit argument-stream progress every N chars (plus on first name / first arg).
const TOOL_CALL_PROGRESS_ARG_STEP: usize = 256;

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

    fn push_chunks(&mut self, chunks: &[Value]) -> Vec<Result<ModelStreamEvent>> {
        let mut events = Vec::new();
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
                    if !name.is_empty() {
                        call.name = Some(name.to_string());
                    }
                }
                if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                    call.arguments.push_str(arguments);
                }
            }
            if let Some(progress) = call.maybe_progress_event() {
                events.push(Ok(progress));
            }
        }
        events
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

impl PartialChatToolCall {
    /// Emit progress when the name first appears or arguments grow enough.
    fn maybe_progress_event(&mut self) -> Option<ModelStreamEvent> {
        let tool_name = self.name.as_ref()?;
        if tool_name.is_empty() {
            return None;
        }
        let args_len = self.arguments.len();
        let name_just_appeared = !self.name_emitted;
        let args_step = args_len.saturating_sub(self.last_progress_args)
            >= TOOL_CALL_PROGRESS_ARG_STEP
            || (args_len > 0 && self.last_progress_args == 0);
        if !name_just_appeared && !args_step {
            return None;
        }
        self.name_emitted = true;
        self.last_progress_args = args_len;
        Some(ModelStreamEvent::ToolCallProgress {
            id: self.id.clone(),
            tool_name: tool_name.clone(),
            arguments_chars: args_len,
        })
    }
}

/// Tencent Hy / Hunyuan family (`hy_v3`) uses tagged tool calls, e.g.:
/// ```text
/// <tool_calls:opensource>
/// <tool_call:opensource>read_file<tool_sep:opensource>
/// <arg_key:opensource>path</arg_key:opensource>
/// <arg_value:opensource>main.rs</arg_value:opensource>
/// </tool_call:opensource>
/// </tool_calls:opensource>
/// ```
/// Also accept un-suffixed `<tool_call>…</tool_call>` JSON (MiniMax / generic).
const TOOL_CALL_START_MARKERS: &[&str] = &[
    "]<]minimax[>[<tool_call>",
    "<]minimax[>[<tool_call>",
    "]<|minimal|>[<tool_call>",
    "<|minimal|>[<tool_call>",
    "<tool_call:opensource>",
    "<tool_call>",
];

const TOOL_CALL_END_MARKERS: &[&str] = &["</tool_call:opensource>", "</tool_call>"];

const TOOL_CALLS_WRAPPER_OPEN: &[&str] = &["<tool_calls:opensource>", "<tool_calls>"];
const TOOL_CALLS_WRAPPER_CLOSE: &[&str] = &["</tool_calls:opensource>", "</tool_calls>"];

const HY_TOOL_SEP: &[&str] = &["<tool_sep:opensource>", "<tool_sep>"];
const HY_ARG_KEY_OPEN: &[&str] = &["<arg_key:opensource>", "<arg_key>"];
const HY_ARG_KEY_CLOSE: &[&str] = &["</arg_key:opensource>", "</arg_key>"];
const HY_ARG_VALUE_OPEN: &[&str] = &["<arg_value:opensource>", "<arg_value>"];
const HY_ARG_VALUE_CLOSE: &[&str] = &["</arg_value:opensource>", "</arg_value>"];

const THINK_OPEN_TAGS: &[&str] = &["<think:opensource>", "<think>"];
const THINK_CLOSE_TAGS: &[&str] = &["</think:opensource>", "</think>"];

#[derive(Default)]
struct TextToolCallExtractor {
    pending: String,
    in_tool_call: bool,
    next_tool_call_index: u64,
    tool_call_events: Vec<Result<ModelStreamEvent>>,
    /// Last pending length we announced while inside a tool-call block.
    last_progress_pending: usize,
    /// Whether we already announced an in-progress text tool call.
    progress_emitted: bool,
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
                if let Some((end, end_len)) =
                    find_first_marker(&self.pending, TOOL_CALL_END_MARKERS)
                {
                    let block = self.pending[..end].to_string();
                    self.pending.drain(..end + end_len);
                    self.in_tool_call = false;
                    self.progress_emitted = false;
                    self.last_progress_pending = 0;
                    let calls = self.parse_tool_call_block(&block);
                    self.tool_call_events.extend(calls);
                    continue;
                }

                // Throttled progress while the tool-call body is still open.
                self.push_text_tool_progress();

                if final_chunk {
                    let block = std::mem::take(&mut self.pending);
                    self.in_tool_call = false;
                    self.progress_emitted = false;
                    self.last_progress_pending = 0;
                    let calls = self.parse_tool_call_block(&block);
                    self.tool_call_events.extend(calls);
                }
                break;
            }

            // Prefer real tool-call starts over outer Hy wrappers so
            // `</tool_calls:opensource>` never swallows an inner call body.
            let tool_start = find_tool_call_start(&self.pending);
            let wrapper = find_first_marker(&self.pending, TOOL_CALLS_WRAPPER_OPEN)
                .or_else(|| find_first_marker(&self.pending, TOOL_CALLS_WRAPPER_CLOSE));

            match (tool_start, wrapper) {
                (Some((t_pos, _)), Some((w_pos, w_len))) if w_pos < t_pos => {
                    if w_pos > 0 {
                        clean_text.push_str(&self.pending[..w_pos]);
                    }
                    // Drop wrapper tag only.
                    self.pending.drain(..w_pos + w_len);
                    continue;
                }
                (Some((t_pos, t_len)), _) => {
                    if t_pos > 0 {
                        clean_text.push_str(&self.pending[..t_pos]);
                    }
                    self.pending.drain(..t_pos + t_len);
                    self.in_tool_call = true;
                    self.last_progress_pending = 0;
                    self.progress_emitted = false;
                    // Announce immediately so the UI leaves "waiting for model"
                    // while the tagged tool-call body streams in.
                    self.push_text_tool_progress();
                    continue;
                }
                (None, Some((w_pos, w_len))) => {
                    if w_pos > 0 {
                        clean_text.push_str(&self.pending[..w_pos]);
                    }
                    self.pending.drain(..w_pos + w_len);
                    continue;
                }
                (None, None) => {}
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

    fn push_text_tool_progress(&mut self) {
        let pending_len = self.pending.len();
        let should_emit = !self.progress_emitted
            || pending_len.saturating_sub(self.last_progress_pending)
                >= TOOL_CALL_PROGRESS_ARG_STEP
            || (pending_len > 0 && self.last_progress_pending == 0 && !self.progress_emitted);
        if !should_emit {
            return;
        }
        self.progress_emitted = true;
        self.last_progress_pending = pending_len;
        let tool_name = peek_text_tool_name(&self.pending).unwrap_or_else(|| "tool".to_string());
        self.tool_call_events
            .push(Ok(ModelStreamEvent::ToolCallProgress {
                id: None,
                tool_name,
                arguments_chars: pending_len,
            }));
    }

    fn parse_tool_call_block(&mut self, block: &str) -> Vec<Result<ModelStreamEvent>> {
        // Prefer Tencent hy_v3 tagged form; fall back to JSON `<tool_call>{...}</tool_call>`.
        let values = if let Some(value) = parse_hy_v3_tool_call(block) {
            vec![value]
        } else {
            parse_tool_call_values(block)
        };
        values
            .into_iter()
            .filter_map(|value| self.tool_invocation_from_value(value))
            .map(|invocation| Ok(ModelStreamEvent::ToolCall(invocation)))
            .collect()
    }
}

/// Best-effort tool name while a tagged tool-call body is still streaming.
fn peek_text_tool_name(pending: &str) -> Option<String> {
    let trimmed = pending.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    // Hy form: `read_file<tool_sep…>` / `read_file<tool_sep:opensource>`
    if let Some(sep_idx) = HY_TOOL_SEP.iter().find_map(|marker| trimmed.find(marker)) {
        let name = trimmed[..sep_idx].trim();
        if !name.is_empty() && !name.starts_with('{') && !name.contains('<') {
            return Some(name.to_string());
        }
    }
    // JSON form: `{"name":"write_file",...}`
    if let Some(key_idx) = trimmed.find("\"name\"") {
        let after_key = trimmed[key_idx + "\"name\"".len()..].trim_start();
        if let Some(after_colon) = after_key.strip_prefix(':') {
            let after_colon = after_colon.trim_start();
            if let Some(rest) = after_colon.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                let name = &rest[..end];
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    // Bare leading identifier before whitespace / brace (hy without sep yet).
    let ident: String = trimmed
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if ident.len() >= 2
        && !ident.eq_ignore_ascii_case("true")
        && !ident.eq_ignore_ascii_case("null")
    {
        return Some(ident);
    }
    None
}

impl TextToolCallExtractor {
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

/// Parse Tencent Hy3 / Hunyuan `hy_v3` tool call body (without surrounding tags).
fn parse_hy_v3_tool_call(block: &str) -> Option<Value> {
    let trimmed = block.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Require at least one hy marker so we don't steal plain JSON blocks.
    let looks_hy = HY_TOOL_SEP
        .iter()
        .chain(HY_ARG_KEY_OPEN.iter())
        .any(|m| find_ascii_case_insensitive(trimmed, m).is_some());
    if !looks_hy {
        return None;
    }

    let (name_raw, args_src) = if let Some((pos, len)) = find_first_marker(trimmed, HY_TOOL_SEP) {
        (&trimmed[..pos], &trimmed[pos + len..])
    } else {
        // No sep — treat whole block as name if there are no args; otherwise fail.
        if find_first_marker(trimmed, HY_ARG_KEY_OPEN).is_none() {
            let name = trimmed.trim();
            if name.is_empty() || name.starts_with('{') {
                return None;
            }
            return Some(json!({ "name": name, "arguments": {} }));
        }
        // Name is text before first arg_key
        let (pos, _) = find_first_marker(trimmed, HY_ARG_KEY_OPEN)?;
        (&trimmed[..pos], trimmed)
    };

    let name = name_raw.trim();
    if name.is_empty() || name.contains('<') {
        return None;
    }

    let mut arguments = serde_json::Map::new();
    let mut rest = args_src;
    while let Some((key_open_pos, key_open_len)) = find_first_marker(rest, HY_ARG_KEY_OPEN) {
        rest = &rest[key_open_pos + key_open_len..];
        let (key_close_pos, key_close_len) = find_first_marker(rest, HY_ARG_KEY_CLOSE)?;
        let key = rest[..key_close_pos].trim().to_string();
        rest = &rest[key_close_pos + key_close_len..];

        let (val_open_pos, val_open_len) = find_first_marker(rest, HY_ARG_VALUE_OPEN)?;
        rest = &rest[val_open_pos + val_open_len..];
        let (val_close_pos, val_close_len) = find_first_marker(rest, HY_ARG_VALUE_CLOSE)?;
        let raw_value = rest[..val_close_pos].to_string();
        rest = &rest[val_close_pos + val_close_len..];

        if key.is_empty() {
            continue;
        }
        let value = parse_hy_arg_value(&raw_value);
        arguments.insert(key, value);
    }

    Some(json!({
        "name": name,
        "arguments": Value::Object(arguments),
    }))
}

fn parse_hy_arg_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    // Prefer JSON for objects/arrays/numbers/bools; otherwise keep as string.
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || trimmed == "true"
        || trimmed == "false"
        || trimmed == "null"
        || trimmed.parse::<i64>().is_ok()
        || trimmed.parse::<f64>().is_ok()
    {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return value;
        }
    }
    // Quoted JSON string
    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return value;
        }
    }
    Value::String(raw.to_string())
}

fn normalize_tool_input(value: &Value) -> Value {
    match value {
        Value::String(text) => {
            serde_json::from_str::<Value>(text).unwrap_or_else(|_| value.clone())
        }
        value => value.clone(),
    }
}

fn find_first_marker(text: &str, markers: &[&str]) -> Option<(usize, usize)> {
    markers
        .iter()
        .filter_map(|marker| {
            find_ascii_case_insensitive(text, marker).map(|pos| (pos, marker.len()))
        })
        .min_by_key(|(pos, _)| *pos)
}

fn find_tool_call_start(text: &str) -> Option<(usize, usize)> {
    if let Some(result) = find_first_marker(text, TOOL_CALL_START_MARKERS) {
        return Some(result);
    }
    find_generic_bracket_tool_call_prefix(text)
}

fn find_generic_bracket_tool_call_prefix(text: &str) -> Option<(usize, usize)> {
    // Prefer hy-suffixed tag if present.
    let (tc_pos, marker_len) =
        if let Some(pos) = find_ascii_case_insensitive(text, "<tool_call:opensource>") {
            (pos, "<tool_call:opensource>".len())
        } else if let Some(pos) = find_ascii_case_insensitive(text, "<tool_call>") {
            (pos, "<tool_call>".len())
        } else {
            return None;
        };
    let before = &text[..tc_pos];
    let bracket_end = before.rfind(">[")?;
    if bracket_end > before.len().saturating_sub(64) && bracket_end >= 1 {
        let candidate = &before[..bracket_end];
        let openers = [']', '<', '|'];
        if let Some(prefix_start) = candidate.rfind(|c: char| openers.contains(&c)) {
            let full_len = tc_pos + marker_len - prefix_start;
            return Some((prefix_start, full_len));
        }
    }
    None
}

fn partial_tool_call_start_suffix_len(text: &str) -> usize {
    let specific = TOOL_CALL_START_MARKERS
        .iter()
        .chain(TOOL_CALLS_WRAPPER_OPEN.iter())
        .chain(TOOL_CALLS_WRAPPER_CLOSE.iter())
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
        let tags = if self.in_think {
            THINK_CLOSE_TAGS
        } else {
            THINK_OPEN_TAGS
        };
        if tags.iter().any(|tag| is_partial_tag_prefix(&pending, tag)) {
            return Vec::new();
        }
        self.split(&pending, true)
    }

    fn split(&mut self, input: &str, final_chunk: bool) -> Vec<Result<ModelStreamEvent>> {
        let mut events = Vec::new();
        let mut remaining = input;

        while !remaining.is_empty() {
            let tags = if self.in_think {
                THINK_CLOSE_TAGS
            } else {
                THINK_OPEN_TAGS
            };
            if let Some((pos, len)) = find_first_marker(remaining, tags) {
                self.push_segment(&mut events, &remaining[..pos]);
                remaining = &remaining[pos + len..];
                self.in_think = !self.in_think;
                continue;
            }

            let keep = if final_chunk {
                0
            } else {
                tags.iter()
                    .map(|tag| partial_tag_suffix_len(remaining, tag))
                    .max()
                    .unwrap_or(0)
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

/// Apply first-class and passthrough request options to the OpenAI request body.
///
/// First-class options (`temperature`, `top_p`, `max_tokens`, `response_format`)
/// are applied after `extra_body` so explicit config wins over passthrough
/// values. `model`, `messages`, `input`, `stream`, and `stream_options` are set
/// by the caller after this function, so they cannot be accidentally disabled.
fn apply_openai_request_options(
    body: &mut Value,
    request_options: &navi_core::ProviderRequestOptions,
    api_kind: OpenAiApiKind,
) {
    if let Some(extra_body) = &request_options.extra_body {
        if let Some(obj) = extra_body.as_object() {
            for (k, v) in obj {
                body[k] = v.clone();
            }
        }
    }

    if let Some(temperature) = request_options.temperature {
        body["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request_options.top_p {
        body["top_p"] = json!(top_p);
    }
    if let Some(max_tokens) = request_options.max_tokens {
        match api_kind {
            OpenAiApiKind::Responses => body["max_tokens"] = json!(max_tokens),
            OpenAiApiKind::ChatCompletions => body["max_completion_tokens"] = json!(max_tokens),
        }
    }
    if let Some(response_format) = &request_options.response_format {
        match api_kind {
            OpenAiApiKind::Responses => {
                body["text"] = json!({ "format": response_format });
            }
            OpenAiApiKind::ChatCompletions => {
                body["response_format"] = response_format.clone();
            }
        }
    }
}

/// Apply provider-configured extra HTTP headers to the outbound request.
fn apply_extra_headers(
    headers: &mut reqwest::header::HeaderMap,
    request_options: &navi_core::ProviderRequestOptions,
) {
    if let Some(extra_headers) = &request_options.extra_headers {
        for (name, value) in extra_headers {
            if let Ok(name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
                if let Ok(value) = reqwest::header::HeaderValue::from_str(value) {
                    headers.insert(name, value);
                }
            }
        }
    }
}

/// Apply OpenAI-style prompt-cache body fields.
///
/// - OpenAI / providers that benefit from explicit keys: optional session
///   suffix keeps consecutive tool steps of one agent session together.
/// - Charm Hyper: never emit `prompt_cache_key`. Hyper (like Crush) shares the
///   common system/tool prefix by content hash and sticks a conversation with
///   `x-session-id` / `x-session-affinity` only. Session-scoping the key caused
///   multi-instance cache isolation and large Hypercredit burns.
fn apply_prompt_cache_fields(
    body: &mut Value,
    provider_id: &str,
    model: &str,
    request_options: &navi_core::ProviderRequestOptions,
    session_id: Option<&str>,
) {
    let is_charm_hyper = navi_core::ProviderId::from_config_id(provider_id).as_str()
        == navi_core::ProviderId::CHARM_HYPER;

    if is_charm_hyper {
        // Affinity headers carry session stickiness; body key would isolate
        // the shared NAVI prefix across concurrent instances.
        return;
    }

    if let Some(prompt_cache_key) = &request_options.prompt_cache_key {
        // Scope the cache key by session when available so consecutive tool
        // steps of the same agent session share prefix routing (OpenAI).
        let key = match session_id {
            Some(session_id) if !session_id.is_empty() => {
                format!("{prompt_cache_key}:{session_id}")
            }
            _ => prompt_cache_key.clone(),
        };
        body["prompt_cache_key"] = json!(key);
    }
    if let Some(retention) = &request_options.prompt_cache_retention
        && should_send_prompt_cache_retention(model, retention)
    {
        body["prompt_cache_retention"] = json!(retention);
    }
}

#[cfg(test)]
mod prompt_cache_field_tests {
    use super::apply_prompt_cache_fields;
    use navi_core::ProviderRequestOptions;
    use serde_json::json;

    #[test]
    fn openai_scopes_prompt_cache_key_by_session() {
        let mut body = json!({});
        let opts = ProviderRequestOptions {
            prompt_cache_key: Some("openai".into()),
            prompt_cache_retention: Some("24h".into()),
            ..Default::default()
        };
        apply_prompt_cache_fields(&mut body, "openai", "gpt-5", &opts, Some("sess-1"));
        assert_eq!(body["prompt_cache_key"], json!("openai:sess-1"));
        assert_eq!(body["prompt_cache_retention"], json!("24h"));
    }

    #[test]
    fn charm_hyper_never_emits_prompt_cache_key_even_if_configured() {
        let mut body = json!({});
        let opts = ProviderRequestOptions {
            prompt_cache_key: Some("charm-hyper".into()),
            prompt_cache_retention: Some("24h".into()),
            ..Default::default()
        };
        apply_prompt_cache_fields(
            &mut body,
            "charm-hyper",
            "glm-5.2",
            &opts,
            Some("session-a"),
        );
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("prompt_cache_retention").is_none());

        // Different sessions must not create different body cache keys either.
        let mut body_b = json!({});
        apply_prompt_cache_fields(
            &mut body_b,
            "charm-hyper",
            "glm-5.2",
            &opts,
            Some("session-b"),
        );
        assert!(body_b.get("prompt_cache_key").is_none());
        assert_eq!(body, body_b);
    }
}

#[cfg(test)]
mod message_build_tests {
    use super::chat_completions_messages;
    use navi_core::{ModelMessage, ModelRequest, ThinkingConfig};

    fn bare_request(messages: Vec<ModelMessage>, instructions: Option<&str>) -> ModelRequest {
        ModelRequest {
            model: "test-model".into(),
            instructions: instructions.map(str::to_string),
            messages,
            thinking: ThinkingConfig::Off,
            tools: Vec::new(),
            session_id: None,
        }
    }

    #[test]
    fn chat_completions_does_not_duplicate_system_when_instructions_set() {
        let base = "You are NAVI, base instructions.";
        let request = bare_request(
            vec![
                ModelMessage::system(base),
                ModelMessage::developer("=== AGENTS.md ===\nproject rules"),
                ModelMessage::user("hello"),
            ],
            Some(base),
        );
        let messages = chat_completions_messages(&request);
        let systems: Vec<&str> = messages
            .iter()
            .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("system"))
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
            .collect();
        // Exactly one copy of the base prompt + developer mapped to system.
        assert_eq!(
            systems.iter().filter(|s| **s == base).count(),
            1,
            "base instructions must appear once, got: {systems:?}"
        );
        assert!(
            systems.iter().any(|s| s.contains("AGENTS.md")),
            "developer block should map to system: {systems:?}"
        );
        assert_eq!(messages.len(), 3); // system base, system agents, user
        assert_eq!(
            messages
                .last()
                .unwrap()
                .get("role")
                .and_then(|r| r.as_str()),
            Some("user")
        );
    }

    #[test]
    fn chat_completions_keeps_system_when_instructions_empty() {
        let request = bare_request(
            vec![
                ModelMessage::system("fallback system"),
                ModelMessage::user("hi"),
            ],
            None,
        );
        let messages = chat_completions_messages(&request);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].get("content").and_then(|c| c.as_str()),
            Some("fallback system")
        );
    }
}

#[cfg(test)]
mod request_options_tests {
    use super::apply_openai_request_options;
    use navi_core::ProviderRequestOptions;
    use serde_json::json;

    #[test]
    fn applies_chat_completions_request_options() {
        let mut body = json!({"model":"gpt-5","messages":[]});
        let opts = ProviderRequestOptions {
            temperature: Some(0.5),
            top_p: Some(0.9),
            max_tokens: Some(256),
            response_format: Some(json!({"type": "json_object"})),
            extra_body: Some(json!({"metadata": {"run_id": "abc"}})),
            ..Default::default()
        };
        apply_openai_request_options(&mut body, &opts, super::OpenAiApiKind::ChatCompletions);

        assert_eq!(body["temperature"], 0.5);
        assert_eq!(body["top_p"], 0.9);
        assert_eq!(body["max_completion_tokens"], 256);
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
        assert_eq!(body["metadata"], json!({"run_id": "abc"}));
    }

    #[test]
    fn applies_responses_api_request_options() {
        let mut body = json!({"model":"gpt-5","input":[]});
        let opts = ProviderRequestOptions {
            temperature: Some(0.2),
            top_p: Some(0.95),
            max_tokens: Some(128),
            response_format: Some(json!({"type": "json_schema"})),
            extra_body: Some(json!({"store": false, "metadata": {"key": "val"}})),
            ..Default::default()
        };
        apply_openai_request_options(&mut body, &opts, super::OpenAiApiKind::Responses);

        assert_eq!(body["temperature"], 0.2);
        assert_eq!(body["top_p"], 0.95);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["text"], json!({"format": {"type": "json_schema"}}));
        assert_eq!(body["store"], false);
        assert_eq!(body["metadata"], json!({"key": "val"}));
    }

    #[test]
    fn first_class_options_override_extra_body() {
        let mut body = json!({});
        let opts = ProviderRequestOptions {
            temperature: Some(0.7),
            extra_body: Some(json!({"temperature": 0.1, "presence_penalty": 0.5})),
            ..Default::default()
        };
        apply_openai_request_options(&mut body, &opts, super::OpenAiApiKind::ChatCompletions);

        assert_eq!(
            body["temperature"], 0.7,
            "first-class temperature should win"
        );
        assert_eq!(
            body["presence_penalty"], 0.5,
            "extra_body-only field should remain"
        );
    }

    #[test]
    fn ignores_non_object_extra_body() {
        let mut body = json!({"model":"gpt-5"});
        let opts = ProviderRequestOptions {
            extra_body: Some(json!(["not", "an", "object"])),
            ..Default::default()
        };
        apply_openai_request_options(&mut body, &opts, super::OpenAiApiKind::ChatCompletions);

        assert_eq!(body["model"], "gpt-5");
        assert!(
            body.as_object().unwrap().len() == 1,
            "non-object extra_body should be ignored"
        );
    }

    #[test]
    fn empty_options_leave_body_unchanged() {
        let mut body = json!({"model":"gpt-5"});
        apply_openai_request_options(
            &mut body,
            &ProviderRequestOptions::default(),
            super::OpenAiApiKind::ChatCompletions,
        );
        assert_eq!(body, json!({"model":"gpt-5"}));
    }

    #[test]
    fn applies_extra_headers_and_skips_invalid_ones() {
        use std::collections::BTreeMap;

        let mut headers = reqwest::header::HeaderMap::new();
        let mut extras = BTreeMap::new();
        extras.insert("OpenAI-Project".to_string(), "proj_123".to_string());
        extras.insert("OpenAI-Organization".to_string(), "org_456".to_string());
        extras.insert("\n".to_string(), "bad-name".to_string());
        extras.insert("Good-Name".to_string(), "bad\r\nvalue".to_string());
        let opts = ProviderRequestOptions {
            extra_headers: Some(extras),
            ..Default::default()
        };
        super::apply_extra_headers(&mut headers, &opts);

        assert_eq!(headers["OpenAI-Project"], "proj_123");
        assert_eq!(headers["OpenAI-Organization"], "org_456");
        assert!(!headers.contains_key("\n"));
        assert!(!headers.contains_key("Good-Name"));
    }
}

#[cfg(test)]
mod responses_sse_tests {
    use super::parse_openai_responses_sse;
    use navi_core::ModelStreamEvent;

    fn first_event(data: &str) -> Option<ModelStreamEvent> {
        parse_openai_responses_sse(data)
            .into_iter()
            .next()
            .and_then(|r| r.ok())
    }

    #[test]
    fn done_sentinel_emits_done() {
        let events = parse_openai_responses_sse("[DONE]");
        assert!(matches!(
            events[0].as_ref().unwrap(),
            ModelStreamEvent::Done
        ));
    }

    #[test]
    fn malformed_json_returns_error() {
        let events = parse_openai_responses_sse("not-json");
        assert!(events[0].is_err());
    }

    #[test]
    fn response_output_text_delta_emits_text() {
        let event =
            first_event(r#"{"type":"response.output_text.delta","delta":"hello"}"#).unwrap();
        assert!(matches!(event, ModelStreamEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn response_reasoning_text_delta_emits_thinking() {
        let event =
            first_event(r#"{"type":"response.reasoning_text.delta","delta":"think"}"#).unwrap();
        assert!(matches!(event, ModelStreamEvent::ThinkingDelta { text } if text == "think"));
    }

    #[test]
    fn response_reasoning_text_delta_without_text_emits_status() {
        let event = first_event(r#"{"type":"response.reasoning_text.delta"}"#).unwrap();
        assert!(matches!(event, ModelStreamEvent::Status { label } if label == "thinking"));
    }

    #[test]
    fn response_reasoning_summary_text_delta_emits_thinking() {
        let event =
            first_event(r#"{"type":"response.reasoning_summary_text.delta","delta":"summary"}"#)
                .unwrap();
        assert!(matches!(event, ModelStreamEvent::ThinkingDelta { text } if text == "summary"));
    }

    #[test]
    fn response_output_item_added_emits_tool_progress() {
        let event = first_event(r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#).unwrap();
        assert!(matches!(
            event,
            ModelStreamEvent::ToolCallProgress { id, tool_name, arguments_chars }
                if id.as_deref() == Some("call_1") && tool_name == "read_file" && arguments_chars > 0
        ));
    }

    #[test]
    fn response_output_item_done_emits_tool_call() {
        let event = first_event(r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#).unwrap();
        assert!(matches!(
            event,
            ModelStreamEvent::ToolCall(inv) if inv.id == "call_1" && inv.tool_name == "read_file"
        ));
    }

    #[test]
    fn response_function_call_arguments_delta_emits_progress() {
        let event = first_event(r#"{"type":"response.function_call_arguments.delta","item_id":"call_1","name":"read_file","delta":"abc"}"#).unwrap();
        assert!(matches!(
            event,
            ModelStreamEvent::ToolCallProgress { id, tool_name, arguments_chars }
                if id.as_deref() == Some("call_1") && tool_name == "read_file" && arguments_chars == 3
        ));
    }

    #[test]
    fn response_function_call_arguments_delta_with_empty_name_and_delta_is_ignored() {
        let events = parse_openai_responses_sse(
            r#"{"type":"response.function_call_arguments.delta","item_id":"call_1","name":"","delta":""}"#,
        );
        assert!(events.is_empty());
    }

    #[test]
    fn response_completed_emits_done_and_usage() {
        let events = parse_openai_responses_sse(
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":5}}}"#,
        );
        let last = events.last().unwrap().as_ref().unwrap();
        assert!(matches!(last, ModelStreamEvent::Done));
    }

    #[test]
    fn response_incomplete_emits_done() {
        let events = parse_openai_responses_sse(
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_tokens"}}}"#,
        );
        assert!(matches!(
            events[0].as_ref().unwrap(),
            ModelStreamEvent::Done
        ));
    }

    #[test]
    fn response_failed_returns_error() {
        let events =
            parse_openai_responses_sse(r#"{"type":"response.failed","error":{"message":"boom"}}"#);
        assert!(events[0].is_err());
    }

    #[test]
    fn unknown_response_type_is_ignored() {
        let events = parse_openai_responses_sse(r#"{"type":"response.unknown","delta":"x"}"#);
        assert!(events.is_empty());
    }
}

#[cfg(test)]
mod tool_helpers_tests {
    use super::*;

    #[test]
    fn peek_text_tool_name_handles_variants() {
        assert_eq!(
            peek_text_tool_name("read_file<tool_sep>"),
            Some("read_file".into())
        );
        assert_eq!(
            peek_text_tool_name(r#"{"name":"write_file","args":{}}"#),
            Some("write_file".into())
        );
        assert_eq!(
            peek_text_tool_name("read_file<tool_sep:opensource>"),
            Some("read_file".into())
        );
        assert_eq!(peek_text_tool_name("valid_name"), Some("valid_name".into()));
        assert_eq!(peek_text_tool_name("true"), None);
        assert_eq!(peek_text_tool_name("null"), None);
        assert_eq!(peek_text_tool_name("  "), None);
    }

    #[test]
    fn parse_tool_call_values_handles_array_and_invalid() {
        let values = parse_tool_call_values(r#"[{"name":"a"},{"name":"b"}]"#);
        assert_eq!(values.len(), 2);
        assert_eq!(values[1]["name"], "b");

        let values = parse_tool_call_values(r#"{"name":"a"}{"name":"b"}garbage"#);
        assert_eq!(values.len(), 2);

        assert!(parse_tool_call_values("").is_empty());
    }

    #[test]
    fn parse_hy_arg_value_coerces_types() {
        assert_eq!(parse_hy_arg_value("1"), serde_json::json!(1));
        assert_eq!(parse_hy_arg_value("1.5"), serde_json::json!(1.5));
        assert_eq!(parse_hy_arg_value("true"), serde_json::json!(true));
        assert_eq!(parse_hy_arg_value("null"), serde_json::json!(null));
        assert_eq!(
            parse_hy_arg_value(r#""quoted""#),
            serde_json::json!("quoted")
        );
        assert_eq!(parse_hy_arg_value("plain"), serde_json::json!("plain"));
        assert_eq!(parse_hy_arg_value(""), serde_json::json!(""));
    }

    #[test]
    fn parse_hy_v3_tool_call_handles_name_only_and_arguments() {
        assert_eq!(
            parse_hy_v3_tool_call("read_file<tool_sep>").unwrap(),
            serde_json::json!({"name": "read_file", "arguments": {}})
        );
        assert_eq!(
            parse_hy_v3_tool_call(
                "read_file<tool_sep><arg_key>path</arg_key><arg_value>/tmp</arg_value>"
            )
            .unwrap(),
            serde_json::json!({"name": "read_file", "arguments": {"path": "/tmp"}})
        );
        assert!(parse_hy_v3_tool_call("plain-json-not-hy").is_none());
        assert!(parse_hy_v3_tool_call("").is_none());
    }

    #[test]
    fn find_generic_bracket_tool_call_prefix_detects_bracketed_tags() {
        assert!(
            find_generic_bracket_tool_call_prefix("]x>[<tool_call>read_file</tool_call>]")
                .is_some()
        );
        assert!(
            find_generic_bracket_tool_call_prefix(
                "]x>[<tool_call:opensource>read_file</tool_call>]"
            )
            .is_some()
        );
        assert!(
            find_generic_bracket_tool_call_prefix("<tool_call>read_file</tool_call>").is_none()
        );
    }

    #[test]
    fn partial_generic_bracket_suffix_len_returns_partial_lengths() {
        assert_eq!(partial_generic_bracket_suffix_len(">[<tool_c"), 9);
        assert_eq!(partial_generic_bracket_suffix_len(">[<tool_c"), 9);
        assert_eq!(partial_generic_bracket_suffix_len("x"), 0);
        assert_eq!(partial_generic_bracket_suffix_len(""), 0);
    }

    #[test]
    fn partial_tag_suffix_len_and_is_partial_tag_prefix() {
        assert_eq!(partial_tag_suffix_len("</thin", "</think>"), 6);
        assert_eq!(partial_tag_suffix_len("", "</think>"), 0);
        assert!(is_partial_tag_prefix("</thin", "</think>"));
        assert!(!is_partial_tag_prefix("xyz", "</think>"));
    }

    #[test]
    fn think_tag_splitter_drains_partial_tag() {
        let mut splitter = ThinkTagSplitter::default();
        let events = splitter.push("<think>text</think");
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], Ok(ModelStreamEvent::ThinkingDelta { text }) if text == "text")
        );
        // Pending "</think" is a partial close tag prefix while in_think=true, so drain is empty.
        let events = splitter.drain_pending();
        assert!(events.is_empty());
    }

    #[test]
    fn model_supports_extended_cache_and_retention_gate() {
        assert!(model_supports_extended_cache("gpt-5-foo"));
        assert!(model_supports_extended_cache("gpt-4.1"));
        assert!(model_supports_extended_cache("o4-mini"));
        assert!(model_supports_extended_cache("o3"));
        assert!(!model_supports_extended_cache("gpt-4o"));

        assert!(should_send_prompt_cache_retention("gpt-4o", "1h"));
        assert!(should_send_prompt_cache_retention("gpt-5", "24h"));
        assert!(!should_send_prompt_cache_retention("gpt-4o", "24h"));
    }

    #[test]
    fn requires_initial_session_title_detects_tool_availability() {
        use navi_core::{ModelMessage, ThinkingConfig, ToolDefinition};
        let mut set_title = ToolDefinition::default();
        set_title.name = "set_session_title".into();
        set_title.description = "set title".into();
        let mut request = ModelRequest {
            model: "gpt-5".into(),
            instructions: None,
            messages: vec![ModelMessage::user("hi")],
            thinking: ThinkingConfig::Off,
            tools: vec![set_title],
            session_id: None,
        };
        assert!(requires_initial_session_title(&request));
        request.messages.push(ModelMessage {
            role: navi_core::ModelRole::Tool,
            content: "ok".into(),
            tool_name: Some("set_session_title".into()),
            tool_call_id: Some("call_1".into()),
            content_parts: vec![],
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        });
        assert!(!requires_initial_session_title(&request));
    }
}
