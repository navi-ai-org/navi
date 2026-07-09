use crate::types::OpenAiApiKind;
use navi_core::{
    ContentPart, ModelMessage, ModelRole, ThinkingRequest, ToolDefinition, ToolInvocation,
};
use serde_json::{Map, Value, json};

/// Converts content_parts into OpenAI Chat Completions content array format.
///
/// Text parts become `{ "type": "text", "text": "..." }`.
/// Image parts become `{ "type": "image_url", "image_url": { "url": "data:<mime>;base64,<data>" } }`.
fn content_parts_to_chat_json(parts: &[ContentPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({ "type": "text", "text": text }),
            ContentPart::Image { media_type, data } => json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{media_type};base64,{data}")
                }
            }),
            ContentPart::Audio {
                media_type, name, ..
            } => json!({
                "type": "text",
                "text": attachment_placeholder("audio", media_type, name.as_deref())
            }),
            ContentPart::Video {
                media_type, name, ..
            } => json!({
                "type": "text",
                "text": attachment_placeholder("video", media_type, name.as_deref())
            }),
            ContentPart::Document {
                media_type, name, ..
            } => json!({
                "type": "text",
                "text": attachment_placeholder("document", media_type, name.as_deref())
            }),
        })
        .collect()
}

/// Converts content_parts into OpenAI Responses input content array format.
///
/// Text parts become `{ "type": "input_text", "text": "..." }`.
/// Image parts become `{ "type": "input_image", "image_url": "data:<mime>;base64,<data>" }`.
fn content_parts_to_responses_json(parts: &[ContentPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({ "type": "input_text", "text": text }),
            ContentPart::Image { media_type, data } => json!({
                "type": "input_image",
                "image_url": format!("data:{media_type};base64,{data}")
            }),
            ContentPart::Audio {
                media_type, name, ..
            } => json!({
                "type": "input_text",
                "text": attachment_placeholder("audio", media_type, name.as_deref())
            }),
            ContentPart::Video {
                media_type, name, ..
            } => json!({
                "type": "input_text",
                "text": attachment_placeholder("video", media_type, name.as_deref())
            }),
            ContentPart::Document {
                media_type, name, ..
            } => json!({
                "type": "input_text",
                "text": attachment_placeholder("document", media_type, name.as_deref())
            }),
        })
        .collect()
}

fn attachment_placeholder(kind: &str, media_type: &str, name: Option<&str>) -> String {
    match name {
        Some(name) if !name.is_empty() => {
            format!("[{kind} attachment omitted from this provider request: {name} ({media_type})]")
        }
        _ => format!("[{kind} attachment omitted from this provider request: {media_type}]"),
    }
}

pub(crate) fn message_to_json(message: &ModelMessage) -> Value {
    // Pre-size the map for the common case of role + content (+ tool fields).
    let mut obj = Map::with_capacity(6);

    let role = match message.role {
        ModelRole::System => "system",
        ModelRole::Developer => "developer",
        ModelRole::User => "user",
        ModelRole::Assistant => "assistant",
        ModelRole::Tool => "tool",
    };
    obj.insert("role".into(), Value::String(role.into()));
    // Use multimodal content array when content_parts is non-empty,
    // otherwise fall back to plain text string.
    if !message.content_parts.is_empty() {
        obj.insert(
            "content".into(),
            Value::Array(content_parts_to_chat_json(&message.content_parts)),
        );
    } else {
        obj.insert("content".into(), Value::String(message.content.clone()));
    }

    if let Some(tool_call_id) = &message.tool_call_id {
        obj.insert("tool_call_id".into(), Value::String(tool_call_id.clone()));
    }
    if let Some(tool_name) = &message.tool_name {
        obj.insert("name".into(), Value::String(tool_name.clone()));
    }
    if !message.tool_calls.is_empty() {
        // OpenAI requires content to be either a string or null when tool_calls
        // are present; empty strings get normalized to null.
        if message.content.is_empty() {
            obj.insert("content".into(), Value::Null);
        }
        let tool_calls: Vec<Value> = message
            .tool_calls
            .iter()
            .map(chat_tool_call_to_json)
            .collect();
        obj.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    if let Some(thinking) = &message.thinking_content
        && message.role == ModelRole::Assistant
        && !thinking.is_empty()
    {
        obj.insert("reasoning_content".into(), Value::String(thinking.clone()));
    }
    Value::Object(obj)
}

pub(crate) fn responses_input_item_to_json(message: &ModelMessage) -> Vec<Value> {
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

    // For multimodal user messages in Responses API, emit a message item
    // with the content array in Responses format.
    if !message.content_parts.is_empty() && message.role == ModelRole::User {
        let content = content_parts_to_responses_json(&message.content_parts);
        return vec![json!({
            "type": "message",
            "role": "user",
            "content": Value::Array(content),
        })];
    }

    // Developer messages go into the input array as developer-role message items.
    // System messages are sent in the `instructions` field, not the input array.
    if message.role == ModelRole::Developer {
        return vec![json!({
            "type": "message",
            "role": "developer",
            "content": message.content,
        })];
    }

    if message.role == ModelRole::System {
        return Vec::new();
    }

    vec![message_to_json(message)]
}

pub(crate) fn chat_tool_call_to_json(invocation: &ToolInvocation) -> Value {
    json!({
        "id": invocation.id,
        "type": "function",
        "function": {
            "name": invocation.tool_name,
            "arguments": invocation.input.to_string(),
        }
    })
}

pub(crate) fn responses_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
    })
}

pub(crate) fn chat_tool_to_json(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

pub(crate) fn apply_thinking_to_body(
    body: &mut Value,
    thinking: ThinkingRequest,
    api_kind: OpenAiApiKind,
    provider_id: &str,
) {
    let Some(object) = body.as_object_mut() else {
        return;
    };

    if !thinking.enabled {
        return;
    }

    let provider = navi_core::ProviderId::from_config_id(provider_id);

    match api_kind {
        OpenAiApiKind::Responses => {
            if let Some(effort) = thinking.effort {
                object.insert("reasoning".to_string(), json!({ "effort": effort }));
            }
        }
        OpenAiApiKind::ChatCompletions => match provider.as_str() {
            navi_core::ProviderId::ANTHROPIC => {
                if let Some(budget) = thinking.budget_tokens {
                    object.insert(
                        "thinking".to_string(),
                        json!({ "type": "enabled", "budget_tokens": budget }),
                    );
                    let max_tokens = (budget + 1024).max(4096);
                    object.insert("max_tokens".to_string(), json!(max_tokens));
                }
            }
            navi_core::ProviderId::GOOGLE_GEMINI => {
                if let Some(budget) = thinking.budget_tokens {
                    object.insert(
                        "extra_body".to_string(),
                        json!({ "google": { "thinking_config": { "thinkingBudget": budget } } }),
                    );
                }
            }
            navi_core::ProviderId::OPENROUTER => {
                if let Some(effort) = thinking.effort {
                    object.insert(
                        "reasoning".to_string(),
                        json!({ "effort": effort, "exclude": true }),
                    );
                }
            }
            _ => {
                if let Some(effort) = thinking.effort {
                    object.insert("reasoning_effort".to_string(), json!(effort));
                }
            }
        },
    }
}

pub(crate) fn thinking_request_for_api(
    thinking: navi_core::ThinkingConfig,
    api_kind: OpenAiApiKind,
    provider_id: &str,
) -> ThinkingRequest {
    thinking_request_for_api_with_levels(thinking, api_kind, provider_id, &[])
}

/// Like [`thinking_request_for_api`], but prefers registry `reasoning_levels` labels.
pub(crate) fn thinking_request_for_api_with_levels(
    thinking: navi_core::ThinkingConfig,
    api_kind: OpenAiApiKind,
    provider_id: &str,
    reasoning_levels: &[String],
) -> ThinkingRequest {
    let mut request = thinking.to_thinking_request();

    // Prefer registry/provider effort label (may be xhigh, none, minimal, …).
    if let Some(label) =
        navi_core::resolve_effort_label(thinking, reasoning_levels, provider_id)
    {
        request.effort = Some(label);
    } else if matches!(thinking, navi_core::ThinkingConfig::Off) {
        request.effort = None;
        request.enabled = false;
    }

    // OpenRouter off uses "none" as an effort string when thinking is disabled
    // only if the model still exposes a none level; otherwise leave disabled.
    if navi_core::ProviderId::from_config_id(provider_id).as_str()
        == navi_core::ProviderId::OPENROUTER
        && matches!(thinking, navi_core::ThinkingConfig::Off)
        && reasoning_levels
            .iter()
            .any(|l| l.eq_ignore_ascii_case("none"))
    {
        request.enabled = false;
        request.effort = Some("none".to_string());
    }

    // Opencode family in Responses mode uses Responses-style effort
    let _ = api_kind;

    request
}

pub(crate) fn text_delta(text: &str) -> anyhow::Result<navi_core::ModelStreamEvent> {
    Ok(navi_core::ModelStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

pub(crate) fn usage_from_value(
    value: Option<&Value>,
) -> Vec<anyhow::Result<navi_core::ModelStreamEvent>> {
    usage_from_value_with_behavior(value, None)
}

pub(crate) fn usage_from_value_with_behavior(
    value: Option<&Value>,
    behavior: Option<&dyn crate::providers::behavior::ProviderBehavior>,
) -> Vec<anyhow::Result<navi_core::ModelStreamEvent>> {
    let Some(usage) = value else {
        return Vec::new();
    };

    let normalized = if let Some(b) = behavior {
        b.parse_usage(usage)
    } else {
        // Fallback: try all common field name variants (lenient numbers).
        use crate::providers::behavior::{json_u64, NormalizedUsage};
        let mut input_tokens = json_u64(
            usage
                .get("input_tokens")
                .or_else(|| usage.get("prompt_tokens"))
                .or_else(|| usage.get("promptTokenCount")),
        );
        let output_tokens = json_u64(
            usage
                .get("output_tokens")
                .or_else(|| usage.get("completion_tokens"))
                .or_else(|| usage.get("candidatesTokenCount")),
        );
        let cache_read_tokens = usage
            .get("input_tokens_details")
            .or_else(|| usage.get("prompt_tokens_details"))
            .and_then(|d| d.get("cached_tokens"))
            .and_then(crate::providers::behavior::json_u64_value)
            .or_else(|| json_u64(usage.get("cache_read_input_tokens")));
        let cache_creation_tokens = json_u64(usage.get("cache_creation_input_tokens"));
        if input_tokens.is_none() {
            if let Some(total) = json_u64(usage.get("total_tokens")) {
                input_tokens = Some(total.saturating_sub(output_tokens.unwrap_or(0)));
            }
        }
        NormalizedUsage {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        }
    };

    if normalized.input_tokens.is_some()
        || normalized.output_tokens.is_some()
        || normalized.cache_read_tokens.is_some()
        || normalized.cache_creation_tokens.is_some()
    {
        vec![Ok(navi_core::ModelStreamEvent::Usage {
            input_tokens: normalized.input_tokens,
            output_tokens: normalized.output_tokens,
            cache_creation_tokens: normalized.cache_creation_tokens,
            cache_read_tokens: normalized.cache_read_tokens,
        })]
    } else {
        Vec::new()
    }
}

pub(crate) fn unique_sorted_model_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut models: Vec<String> = ids.into_iter().collect();
    models.sort();
    models.dedup();
    models
}

pub(crate) fn reasoning_text(value: &Value) -> String {
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
