use crate::types::OpenAiApiKind;
use navi_core::{ModelMessage, ModelRole, ThinkingAdapter, ToolDefinition, ToolInvocation};
use serde_json::{Map, Value, json};

pub(crate) fn message_to_json(message: &ModelMessage) -> Value {
    // Pre-size the map for the common case of role + content (+ tool fields).
    let mut obj = Map::with_capacity(6);

    let role = match message.role {
        ModelRole::System => "system",
        ModelRole::User => "user",
        ModelRole::Assistant => "assistant",
        ModelRole::Tool => "tool",
    };
    obj.insert("role".into(), Value::String(role.into()));
    obj.insert("content".into(), Value::String(message.content.clone()));

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
    adapter: ThinkingAdapter,
    api_kind: OpenAiApiKind,
) {
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

pub(crate) fn thinking_adapter_for_api(
    provider_id: &str,
    thinking: navi_core::ThinkingConfig,
    api_kind: OpenAiApiKind,
) -> ThinkingAdapter {
    if navi_core::ProviderId::from_config_id(provider_id).is_opencode_family()
        && matches!(api_kind, OpenAiApiKind::Responses)
    {
        return thinking
            .to_openai_effort()
            .map(|effort| ThinkingAdapter::OpenAiResponses(json!({ "effort": effort })))
            .unwrap_or(ThinkingAdapter::Unsupported);
    }

    thinking.adapter_for_provider(provider_id)
}

pub(crate) fn text_delta(text: &str) -> anyhow::Result<navi_core::ModelStreamEvent> {
    Ok(navi_core::ModelStreamEvent::TextDelta {
        text: text.to_string(),
    })
}

pub(crate) fn usage_from_value(
    value: Option<&Value>,
) -> Vec<anyhow::Result<navi_core::ModelStreamEvent>> {
    let Some(usage) = value else {
        return Vec::new();
    };
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .or_else(|| usage.get("promptTokenCount"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .or_else(|| usage.get("candidatesTokenCount"))
        .and_then(Value::as_u64);

    if input_tokens.is_some() || output_tokens.is_some() {
        vec![Ok(navi_core::ModelStreamEvent::Usage {
            input_tokens,
            output_tokens,
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
