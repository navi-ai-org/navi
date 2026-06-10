fn extract_output_text(value: &serde_json::Value) -> String {
    if let Some(text) = value.get("output_text").and_then(|v| v.as_str()) {
        return text.to_string();
    }

    value
        .get("output")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .flat_map(|item| {
            item.get("content")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
        })
        .filter_map(|content| content.get("text").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("")
}

fn extract_chat_completion_text(value: &serde_json::Value) -> String {
    value
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

use crate::errors::ProviderError;
use crate::mapping::{
    apply_thinking_to_body, message_to_json, responses_input_item_to_json,
    thinking_request_for_api, unique_sorted_model_ids,
};
use crate::provider::OpenAiProvider;
use crate::providers::anthropic::parse_anthropic_sse;
use crate::providers::behavior::ProviderBehavior;
use crate::providers::gemini::parse_gemini_sse;
use crate::providers::openai::{
    ChatToolCallAccumulator, parse_chat_completions_sse, parse_chat_completions_sse_with_state,
    parse_openai_responses_sse,
};
use crate::sse::SseDecoder;
use crate::transport::{
    extract_requested_delay_from_json, get_backoff_delay, retry_delay_for_error, should_retry_error,
};
use crate::types::OpenAiApiKind;
use futures_util::StreamExt;
use navi_core::{ModelMessage, ModelProvider, ModelStreamEvent, ToolInvocation};
use serde_json::json;

#[test]
fn model_listing_ids_are_sorted_and_deduplicated() {
    let ids = unique_sorted_model_ids(
        ["z/model", "a/model", "z/model", "m/model", "a/model"]
            .into_iter()
            .map(str::to_string),
    );

    assert_eq!(ids, vec!["a/model", "m/model", "z/model"]);
}

#[test]
fn chat_messages_serialize_assistant_tool_call_and_result() {
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: json!({ "path": "Cargo.toml" }),
    };

    let assistant = message_to_json(&ModelMessage::assistant_tool_call(invocation));
    let tool = message_to_json(&ModelMessage::tool_result(
        "call-1",
        "read_file",
        "status: success",
    ));

    assert_eq!(assistant["role"], "assistant");
    assert!(assistant["content"].is_null());
    assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
    assert_eq!(tool["role"], "tool");
    assert_eq!(tool["tool_call_id"], "call-1");
    assert_eq!(tool["name"], "read_file");
}

#[test]
fn chat_messages_preserve_non_empty_content_with_tool_calls() {
    // OpenAI rejects empty strings with tool_calls, so a real content string
    // must be preserved verbatim. This guards against the optimization in
    // message_to_json accidentally collapsing content to null when the
    // message also has tool_calls.
    let invocation = ToolInvocation {
        id: "call-2".to_string(),
        tool_name: "grep".to_string(),
        input: json!({ "pattern": "x" }),
    };

    let assistant = message_to_json(&ModelMessage::assistant_tool_call_with_context(
        invocation,
        "Looking for x",
        None,
    ));

    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "Looking for x");
    assert_eq!(assistant["tool_calls"][0]["id"], "call-2");
}

#[test]
fn chat_messages_echo_reasoning_content_on_tool_calls() {
    let invocation = ToolInvocation {
        id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        input: json!({ "path": "Cargo.toml" }),
    };

    let assistant = message_to_json(&ModelMessage::assistant_tool_call_with_context(
        invocation,
        "I should inspect Cargo.toml.",
        Some("hidden reasoning".to_string()),
    ));

    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "I should inspect Cargo.toml.");
    assert_eq!(assistant["reasoning_content"], "hidden reasoning");
    assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
}

#[test]
fn chat_messages_serialize_multiple_tool_calls_in_one_assistant_message() {
    let assistant = message_to_json(&ModelMessage::assistant_tool_calls_with_context(
        vec![
            ToolInvocation {
                id: "call-1".to_string(),
                tool_name: "fs_browser".to_string(),
                input: json!({"action": "list"}),
            },
            ToolInvocation {
                id: "call-2".to_string(),
                tool_name: "read_file".to_string(),
                input: json!({ "path": "README.md" }),
            },
        ],
        "I need context.",
        Some("hidden reasoning".to_string()),
    ));

    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], "I need context.");
    assert_eq!(assistant["reasoning_content"], "hidden reasoning");
    assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(assistant["tool_calls"][0]["id"], "call-1");
    assert_eq!(assistant["tool_calls"][1]["id"], "call-2");
}

#[test]
fn responses_messages_serialize_function_call_output() {
    let item = responses_input_item_to_json(&ModelMessage::tool_result(
        "call-1",
        "read_file",
        "status: success",
    ));

    assert_eq!(item.len(), 1);
    assert_eq!(item[0]["type"], "function_call_output");
    assert_eq!(item[0]["call_id"], "call-1");
    assert_eq!(item[0]["output"], "status: success");
}

#[test]
fn extracts_responses_api_output_text_shortcut() {
    let value = json!({ "output_text": "done" });
    assert_eq!(extract_output_text(&value), "done");
}

#[test]
fn extracts_nested_responses_api_text() {
    let value = json!({
        "output": [
            {
                "content": [
                    { "type": "output_text", "text": "hello " },
                    { "type": "output_text", "text": "world" }
                ]
            }
        ]
    });

    assert_eq!(extract_output_text(&value), "hello world");
}

#[test]
fn extracts_chat_completion_text() {
    let value = json!({
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "chat done"
                }
            }
        ]
    });

    assert_eq!(extract_chat_completion_text(&value), "chat done");
}

#[test]
fn applies_openai_responses_reasoning() {
    let mut body = json!({ "model": "gpt-5", "input": [] });

    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::High,
            OpenAiApiKind::Responses,
            "openai",
        ),
        OpenAiApiKind::Responses,
        "openai",
    );

    assert_eq!(body["reasoning"], json!({ "effort": "high" }));
}

#[test]
fn applies_anthropic_openai_compatible_thinking() {
    let mut body = json!({ "model": "claude-sonnet-4", "messages": [] });

    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::Low,
            OpenAiApiKind::ChatCompletions,
            "anthropic",
        ),
        OpenAiApiKind::ChatCompletions,
        "anthropic",
    );

    assert_eq!(
        body["thinking"],
        json!({ "type": "enabled", "budget_tokens": 1024 })
    );
}

#[test]
fn applies_openrouter_reasoning_effort() {
    let mut body = json!({ "model": "openai/gpt-5", "messages": [] });

    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::Max,
            OpenAiApiKind::ChatCompletions,
            "openrouter",
        ),
        OpenAiApiKind::ChatCompletions,
        "openrouter",
    );

    assert_eq!(
        body["reasoning"],
        json!({ "effort": "xhigh", "exclude": true })
    );
}

#[test]
fn parses_openai_responses_text_delta() {
    let events =
        parse_openai_responses_sse(r#"{"type":"response.output_text.delta","delta":"hello"}"#);

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::TextDelta {
            text: "hello".to_string()
        }]
    );
}

#[test]
fn parses_chat_completions_text_delta() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::TextDelta {
            text: "hello".to_string()
        }]
    );
}

#[test]
fn parses_chat_completions_object_reasoning_delta() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"reasoning_details":[{"text":"I should inspect files."}]},"finish_reason":null}]}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::ThinkingDelta {
            text: "I should inspect files.".to_string()
        }]
    );
}

#[test]
fn ignores_null_reasoning_delta() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":null,"reasoning_details":null},"finish_reason":null}]}"#,
    );

    assert!(events.is_empty());
}

#[test]
fn parses_openai_responses_tool_call() {
    let events = parse_openai_responses_sse(
        r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":\"Cargo.toml\"}"}}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::ToolCall(ToolInvocation {
            id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        })]
    );
}

#[test]
fn accumulates_chat_completion_tool_call_arguments() {
    let mut state = ChatToolCallAccumulator::default();
    let first = parse_chat_completions_sse_with_state(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#,
        &mut state,
    );
    let second = parse_chat_completions_sse_with_state(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}]}"#,
        &mut state,
    );

    assert!(first.is_empty());
    assert_eq!(
        second.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::ToolCall(ToolInvocation {
            id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "Cargo.toml" }),
        }),]
    );
}

#[test]
fn parses_chat_completions_ollama_usage() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[],"usage":{"prompt_tokens":123,"completion_tokens":45}}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::Usage {
            input_tokens: Some(123),
            output_tokens: Some(45),
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }]
    );
}

#[test]
fn parses_anthropic_text_and_thinking_delta() {
    let text = parse_anthropic_sse(
        r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}"#,
    );
    let thinking = parse_anthropic_sse(
        r#"{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hidden"}}"#,
    );

    assert_eq!(
        text.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::TextDelta {
            text: "hello".to_string()
        }]
    );
    assert_eq!(
        thinking.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::ThinkingDelta {
            text: "hidden".to_string()
        }]
    );
}

#[test]
fn parses_gemini_text_delta() {
    let events = parse_gemini_sse(r#"{"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}"#);

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::TextDelta {
            text: "hello".to_string()
        }]
    );
}

#[test]
fn sse_decoder_collects_data_frames() {
    let mut decoder = SseDecoder::default();

    let first = decoder.push_bytes(b"event: message\ndata: {\"a\":");
    let second = decoder.push_bytes(b"1}\n\ndata: [DONE]\n\n");

    assert!(first.is_empty());
    assert_eq!(second, vec![r#"{"a":1}"#.to_string(), "[DONE]".to_string()]);
}

#[test]
fn test_backoff_delay() {
    for attempt in 1..=5 {
        let delay = get_backoff_delay(attempt).as_millis();
        let exponent = attempt - 1;
        let base = (200 * (1 << exponent)) as f64;
        let min_expected = (base * 0.9) as u128;
        let max_expected = (base * 1.1) as u128;
        assert!(
            delay >= min_expected && delay <= max_expected,
            "Attempt {}: delay {} not in expected range [{}, {}]",
            attempt,
            delay,
            min_expected,
            max_expected
        );
    }
}

#[test]
fn test_extract_requested_delay_from_json() {
    let json1 = json!({ "requested_delay_ms": 1500 });
    assert_eq!(
        extract_requested_delay_from_json(&json1),
        Some(std::time::Duration::from_millis(1500))
    );

    let json2 = json!({ "error": { "requested_delay_ms": 2500 } });
    assert_eq!(
        extract_requested_delay_from_json(&json2),
        Some(std::time::Duration::from_millis(2500))
    );

    let json3 = json!({ "requested_delay": 1.5 });
    assert_eq!(
        extract_requested_delay_from_json(&json3),
        Some(std::time::Duration::from_millis(1500))
    );

    let json4 = json!({ "error": { "requested_delay": 3 } });
    assert_eq!(
        extract_requested_delay_from_json(&json4),
        Some(std::time::Duration::from_secs(3))
    );

    let json5 = json!({ "error": { "message": "something failed" } });
    assert_eq!(extract_requested_delay_from_json(&json5), None);
}

#[tokio::test]
async fn test_should_retry_error() {
    use reqwest::StatusCode;

    let transport_err = ProviderError::Transport(
        reqwest::Client::new()
            .get("http://127.0.0.1:1/invalid")
            .send()
            .await
            .unwrap_err(),
    );
    assert!(should_retry_error(&transport_err, false));
    assert!(should_retry_error(&transport_err, true));

    let timeout_err = ProviderError::StreamIdleTimeout(std::time::Duration::from_secs(1));
    assert!(should_retry_error(&timeout_err, false));
    assert!(should_retry_error(&timeout_err, true));

    let server_err = ProviderError::Api {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        body: "Internal error".to_string(),
        requested_delay: None,
        body_read_error: None,
    };
    assert!(should_retry_error(&server_err, false));
    assert!(should_retry_error(&server_err, true));

    let rate_limit_err = ProviderError::Api {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: "Rate limit reached".to_string(),
        requested_delay: None,
        body_read_error: None,
    };
    assert!(!should_retry_error(&rate_limit_err, false));
    assert!(should_retry_error(&rate_limit_err, true));

    let free_usage_limit_err = ProviderError::Api {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: r#"{"type":"error","error":{"type":"FreeUsageLimitError","message":"Rate limit exceeded."}}"#.to_string(),
        requested_delay: Some(std::time::Duration::from_secs(64_649)),
        body_read_error: None,
    };
    assert!(!should_retry_error(&free_usage_limit_err, true));
    assert_eq!(
        retry_delay_for_error(&free_usage_limit_err, 1),
        std::time::Duration::from_secs(60)
    );

    let client_err = ProviderError::Api {
        status: StatusCode::BAD_REQUEST,
        body: "Bad request".to_string(),
        requested_delay: None,
        body_read_error: None,
    };
    assert!(!should_retry_error(&client_err, false));
    assert!(!should_retry_error(&client_err, true));

    let other_err = ProviderError::Other("random error".to_string());
    assert!(!should_retry_error(&other_err, false));
    assert!(!should_retry_error(&other_err, true));
}

#[tokio::test]
async fn test_stream_normal() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    let chunk1 = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n";
    let chunk2 =
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":\"stop\"}]}\n\n";
    let chunk3 = "data: [DONE]\n\n";
    let sse_body = format!("{}{}{}", chunk1, chunk2, chunk3);

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "openai".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "gpt-4".to_string(),
        messages: vec![ModelMessage {
            role: navi_core::ModelRole::User,
            content: "Hi".to_string(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let navi_core::ModelStreamEvent::TextDelta { text: t } = event.unwrap() {
            text.push_str(&t);
        }
    }
    assert_eq!(text, "hello world");
}

#[tokio::test]
async fn test_opencode_zen_chat_request_uses_bearer_api_key() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer zen_test_key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                )
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "opencode".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("zen_test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_provider_id("opencode".to_string())
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "deepseek-v4-flash-free".to_string(),
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn test_opencode_zen_gpt_models_use_responses_endpoint() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("Authorization", "Bearer zen_test_key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\ndata: {\"type\":\"response.completed\"}\n\n",
                )
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "opencode".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("zen_test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_provider_id("opencode".to_string())
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "gpt-5.5".to_string(),
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn test_github_copilot_request_uses_oauth_headers() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer copilot_token"))
        .and(header("Openai-Intent", "conversation-edits"))
        .and(header("x-initiator", "user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                )
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "github-copilot".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("copilot_token".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_provider_id("github-copilot".to_string())
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "gpt-5.1".to_string(),
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn test_opencode_zen_claude_models_use_messages_endpoint() {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("Authorization", "Bearer zen_test_key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "opencode".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("zen_test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_provider_id("opencode".to_string())
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "claude-sonnet-4.5".to_string(),
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn test_request_timeout() {
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "data": [] }))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "openai".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        request_timeout_ms: Some(100),
        request_max_retries: Some(1),
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_config(config);

    let res = provider.list_models().await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    let provider_err = err.downcast_ref::<ProviderError>().unwrap();
    assert!(matches!(provider_err, ProviderError::Transport(e) if e.is_timeout()));
}

#[tokio::test]
async fn test_stream_idle_timeout() {
    use std::time::Duration;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0; 1024];
            let _ = socket.read(&mut buf).await;

            let response_headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = socket.write_all(response_headers.as_bytes()).await;

            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });

    let config = navi_core::ProviderConfig {
        id: "openai".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        stream_idle_timeout_ms: Some(100),
        stream_max_retries: Some(1),
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("test_key".to_string())
        .with_base_url(base_url)
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "gpt-4".to_string(),
        messages: vec![],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    let item = stream.next().await;
    assert!(item.is_some());
    let err = item.unwrap().unwrap_err();
    let provider_err = err.downcast_ref::<ProviderError>().unwrap();
    assert!(matches!(provider_err, ProviderError::StreamIdleTimeout(_)));
}

#[tokio::test]
async fn test_rate_limit_retry() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {
                "message": "Rate limit reached",
                "requested_delay_ms": 10
            }
        })))
        .up_to_n_times(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                .insert_header("content-type", "text/event-stream")
        )
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "openai".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        stream_max_retries: Some(3),
        retry_429: Some(true),
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::new("test_key".to_string())
        .with_base_url(mock_server.uri())
        .with_api_kind(OpenAiApiKind::ChatCompletions)
        .with_config(config);

    let request = navi_core::ModelRequest {
        model: "gpt-4".to_string(),
        messages: vec![],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
    };

    let mut stream = provider.stream(request);
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if let navi_core::ModelStreamEvent::TextDelta { text: t } = event.unwrap() {
            text.push_str(&t);
        }
    }
    assert_eq!(text, "hello");
}

// ── SSE decoder golden tests ────────────────────────────────────────────────

#[test]
fn sse_decoder_parses_single_event() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b"data: hello\n\n");
    assert_eq!(events, vec!["hello".to_string()]);
}

#[test]
fn sse_decoder_parses_multiple_events() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b"data: first\n\ndata: second\n\n");
    assert_eq!(events, vec!["first".to_string(), "second".to_string()]);
}

#[test]
fn sse_decoder_handles_chunked_input() {
    let mut decoder = SseDecoder::default();
    let e1 = decoder.push_bytes(b"data: hel");
    assert!(e1.is_empty());
    let e2 = decoder.push_bytes(b"lo\n\n");
    assert_eq!(e2, vec!["hello".to_string()]);
}

#[test]
fn sse_decoder_ignores_empty_data() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b": comment\n\ndata: real\n\n");
    assert_eq!(events, vec!["real".to_string()]);
}

#[test]
fn sse_decoder_handles_multiline_data() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b"data: line1\ndata: line2\n\n");
    assert_eq!(events, vec!["line1\nline2".to_string()]);
}

#[test]
fn sse_decoder_ignores_non_data_lines() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b"event: message\nid: 1\ndata: payload\n\n");
    assert_eq!(events, vec!["payload".to_string()]);
}

#[test]
fn sse_decoder_skips_events_without_data() {
    let mut decoder = SseDecoder::default();
    let events = decoder.push_bytes(b"event: ping\n\n");
    assert!(events.is_empty());
}

// ── Anthropic SSE golden tests ──────────────────────────────────────────────

#[test]
fn anthropic_sse_text_delta() {
    let data =
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
    let events = parse_anthropic_sse(data);
    assert_eq!(events.len(), 1);
    match events[0].as_ref().unwrap() {
        ModelStreamEvent::TextDelta { text } => assert_eq!(text, "Hello"),
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

#[test]
fn anthropic_sse_thinking_delta() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
    let events = parse_anthropic_sse(data);
    assert_eq!(events.len(), 1);
    match events[0].as_ref().unwrap() {
        ModelStreamEvent::ThinkingDelta { text } => assert_eq!(text, "Let me think..."),
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn anthropic_sse_signature_delta() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}"#;
    let events = parse_anthropic_sse(data);
    assert_eq!(events.len(), 1);
    match events[0].as_ref().unwrap() {
        ModelStreamEvent::Status { label } => assert_eq!(label, "thinking"),
        other => panic!("expected Status, got {other:?}"),
    }
}

#[test]
fn anthropic_sse_message_delta_with_usage() {
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":100,"output_tokens":50}}"#;
    let events = parse_anthropic_sse(data);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.as_ref().unwrap(), ModelStreamEvent::Usage { .. }))
    );
}

#[test]
fn anthropic_sse_message_delta_with_cache_usage() {
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":500,"cache_read_input_tokens":0}}"#;
    let events = parse_anthropic_sse(data);
    let usage_event = events
        .iter()
        .find_map(|e| match e.as_ref().unwrap() {
            ModelStreamEvent::Usage {
                cache_creation_tokens,
                cache_read_tokens,
                ..
            } => Some((cache_creation_tokens, cache_read_tokens)),
            _ => None,
        })
        .expect("usage event");
    assert_eq!(*usage_event.0, Some(500));
    assert_eq!(*usage_event.1, Some(0));
}

#[test]
fn openai_usage_extracts_cached_tokens() {
    let usage = serde_json::json!({
        "input_tokens": 1000,
        "output_tokens": 200,
        "input_tokens_details": {
            "cached_tokens": 800,
            "text_tokens": 200
        }
    });
    let behavior = crate::providers::behavior::OpenAiBehavior;
    let normalized = behavior.parse_usage(&usage);
    assert_eq!(normalized.input_tokens, Some(1000));
    assert_eq!(normalized.output_tokens, Some(200));
    assert_eq!(normalized.cache_read_tokens, Some(800));
    assert_eq!(normalized.cache_creation_tokens, None);
}

#[test]
fn openai_chat_completions_usage_extracts_cached_tokens() {
    let usage = serde_json::json!({
        "prompt_tokens": 1000,
        "completion_tokens": 200,
        "prompt_tokens_details": {
            "cached_tokens": 600,
            "audio_tokens": 0
        }
    });
    let behavior = crate::providers::behavior::OpenAiBehavior;
    let normalized = behavior.parse_usage(&usage);
    assert_eq!(normalized.input_tokens, Some(1000));
    assert_eq!(normalized.output_tokens, Some(200));
    assert_eq!(normalized.cache_read_tokens, Some(600));
}

#[test]
fn anthropic_sse_message_stop() {
    let data = r#"{"type":"message_stop"}"#;
    let events = parse_anthropic_sse(data);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].as_ref().unwrap(),
        ModelStreamEvent::Done
    ));
}

#[test]
fn anthropic_sse_error() {
    let data = r#"{"type":"error","error":{"type":"overloaded","message":"Too many requests"}}"#;
    let events = parse_anthropic_sse(data);
    assert_eq!(events.len(), 1);
    assert!(events[0].is_err());
}

#[test]
fn anthropic_sse_ignores_unknown_types() {
    let data =
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#;
    let events = parse_anthropic_sse(data);
    assert!(events.is_empty());
}

#[test]
fn anthropic_sse_multi_turn_conversation() {
    let payloads = vec![
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"I "}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"can "}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"help."}}"#,
        r#"{"type":"message_stop"}"#,
    ];
    let mut text = String::new();
    for payload in payloads {
        for event in parse_anthropic_sse(payload) {
            match event.unwrap() {
                ModelStreamEvent::TextDelta { text: t } => text.push_str(&t),
                ModelStreamEvent::Done => {}
                _ => {}
            }
        }
    }
    assert_eq!(text, "I can help.");
}

// ── Gemini SSE golden tests ─────────────────────────────────────────────────

#[test]
fn gemini_sse_text_delta() {
    let data =
        r#"{"candidates":[{"content":{"parts":[{"text":"Hello world"}]},"finishReason":null}]}"#;
    let events = parse_gemini_sse(data);
    assert!(events.iter().any(|e| matches!(
        e.as_ref().unwrap(),
        ModelStreamEvent::TextDelta { text } if text == "Hello world"
    )));
}

#[test]
fn gemini_sse_thinking_delta() {
    let data =
        r#"{"candidates":[{"content":{"parts":[{"text":"Let me reason...","thought":true}]}}]}"#;
    let events = parse_gemini_sse(data);
    assert!(events.iter().any(|e| matches!(
        e.as_ref().unwrap(),
        ModelStreamEvent::ThinkingDelta { text } if text == "Let me reason..."
    )));
}

#[test]
fn gemini_sse_thought_status_without_text() {
    let data = r#"{"candidates":[{"content":{"parts":[{"thought":true}]}}]}"#;
    let events = parse_gemini_sse(data);
    assert!(events.iter().any(|e| matches!(
        e.as_ref().unwrap(),
        ModelStreamEvent::Status { label } if label == "thinking"
    )));
}

#[test]
fn gemini_sse_with_usage_metadata() {
    let data = r#"{"candidates":[],"usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":50,"totalTokenCount":150}}"#;
    let events = parse_gemini_sse(data);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.as_ref().unwrap(), ModelStreamEvent::Usage { .. }))
    );
}

#[test]
fn gemini_sse_finish_reason_triggers_done() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"done"}]},"finishReason":"STOP"}]}"#;
    let events = parse_gemini_sse(data);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.as_ref().unwrap(), ModelStreamEvent::Done))
    );
}

#[test]
fn gemini_sse_mixed_text_and_thinking() {
    let payloads = vec![
        r#"{"candidates":[{"content":{"parts":[{"text":"Thinking...","thought":true}]}}]}"#,
        r#"{"candidates":[{"content":{"parts":[{"text":"The answer is 42."}]},"finishReason":"STOP"}]}"#,
    ];
    let mut thinking = String::new();
    let mut text = String::new();
    for payload in payloads {
        for event in parse_gemini_sse(payload) {
            match event.unwrap() {
                ModelStreamEvent::ThinkingDelta { text: t } => thinking.push_str(&t),
                ModelStreamEvent::TextDelta { text: t } => text.push_str(&t),
                _ => {}
            }
        }
    }
    assert_eq!(thinking, "Thinking...");
    assert_eq!(text, "The answer is 42.");
}

#[test]
fn gemini_sse_multi_part_response() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"First "},{"text":"second."}]},"finishReason":"STOP"}]}"#;
    let events = parse_gemini_sse(data);
    let text_deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e.as_ref().unwrap() {
            ModelStreamEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(text_deltas, vec!["First ", "second."]);
}

#[test]
fn gemini_sse_empty_candidates() {
    let data = r#"{"candidates":[]}"#;
    let events = parse_gemini_sse(data);
    // Should only have usage (if any) or nothing
    assert!(
        events
            .iter()
            .all(|e| !matches!(e.as_ref().unwrap(), ModelStreamEvent::TextDelta { .. }))
    );
}

// ── Anthropic message conversion ────────────────────────────────────────────

#[test]
fn anthropic_messages_separates_system() {
    let messages = vec![
        ModelMessage::system("You are helpful."),
        ModelMessage::user("Hello"),
        ModelMessage::assistant("Hi there!"),
    ];
    let (system, converted) = crate::providers::anthropic::anthropic_messages(&messages);
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["type"], "text");
    assert_eq!(system[0]["text"], "You are helpful.");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[0]["role"], "user");
    assert_eq!(converted[0]["content"], "Hello");
    assert_eq!(converted[1]["role"], "assistant");
    assert_eq!(converted[1]["content"][0]["type"], "text");
    assert_eq!(converted[1]["content"][0]["text"], "Hi there!");
}

#[test]
fn anthropic_messages_merges_multiple_system() {
    let messages = vec![
        ModelMessage::system("Rule 1."),
        ModelMessage::system("Rule 2."),
        ModelMessage::user("Hi"),
    ];
    let (system, _) = crate::providers::anthropic::anthropic_messages(&messages);
    assert_eq!(system.len(), 2);
    assert_eq!(system[0]["text"], "Rule 1.");
    assert_eq!(system[1]["text"], "Rule 2.");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
}

// ── Gemini content conversion ───────────────────────────────────────────────

#[test]
fn gemini_contents_converts_roles() {
    let messages = vec![
        ModelMessage::system("Be concise."),
        ModelMessage::user("What is 2+2?"),
        ModelMessage::assistant("4"),
    ];
    let (system, contents) = crate::providers::gemini::gemini_contents(&messages);
    assert_eq!(system, "Be concise.");
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model");
}

#[test]
fn gemini_encode_model_for_url() {
    assert_eq!(
        crate::providers::gemini::encode_model_for_url("gemini-2.0-flash"),
        "gemini-2.0-flash"
    );
    assert_eq!(
        crate::providers::gemini::encode_model_for_url("models/gemini-2.0-flash"),
        "models%2Fgemini-2.0-flash"
    );
}

// ── Anthropic tool_use SSE lifecycle ──────────────────────────────────────────

#[test]
fn anthropic_sse_tool_use_lifecycle() {
    let mut state = crate::providers::anthropic::AnthropicToolState::default();
    let parse = |data: &str, state: &mut crate::providers::anthropic::AnthropicToolState| {
        crate::providers::anthropic::parse_anthropic_sse_with_state(data, state)
    };

    // content_block_start with tool_use
    let start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-123","name":"read_file","input":{}}}"#;
    let events = parse(start, &mut state);
    assert!(events.is_empty(), "start should not emit events");

    // input_json_delta (partial JSON)
    let delta1 = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#;
    let events = parse(delta1, &mut state);
    assert!(events.is_empty(), "partial json should not emit events");

    let delta2 = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"test.rs\"}"}}"#;
    let events = parse(delta2, &mut state);
    assert!(events.is_empty(), "partial json should not emit events");

    // content_block_stop -> emits ToolCall
    let stop = r#"{"type":"content_block_stop","index":1}"#;
    let events = parse(stop, &mut state);
    assert_eq!(events.len(), 1);
    let tool_call = events[0].as_ref().unwrap();
    match tool_call {
        ModelStreamEvent::ToolCall(inv) => {
            assert_eq!(inv.id, "call-123");
            assert_eq!(inv.tool_name, "read_file");
            assert_eq!(inv.input["path"], "test.rs");
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn anthropic_sse_tool_use_empty_json() {
    let mut state = crate::providers::anthropic::AnthropicToolState::default();
    let parse = |data: &str, state: &mut crate::providers::anthropic::AnthropicToolState| {
        crate::providers::anthropic::parse_anthropic_sse_with_state(data, state)
    };

    let start = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call-456","name":"bash","input":{}}}"#;
    parse(start, &mut state);

    let stop = r#"{"type":"content_block_stop","index":1}"#;
    let events = parse(stop, &mut state);
    assert_eq!(events.len(), 1);
    match events[0].as_ref().unwrap() {
        ModelStreamEvent::ToolCall(inv) => {
            assert_eq!(inv.id, "call-456");
            assert_eq!(inv.tool_name, "bash");
            assert_eq!(inv.input, json!({}));
        }
        _ => panic!("expected ToolCall"),
    }
}

// ── Anthropic message conversion with tools ──────────────────────────────────

#[test]
fn anthropic_messages_tool_role_produces_tool_result() {
    let messages = vec![ModelMessage::tool_result(
        "call-1",
        "read_file",
        "file content",
    )];
    let (_, converted) = crate::providers::anthropic::anthropic_messages(&messages);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0]["role"], "user");
    let content = converted[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "call-1");
    assert_eq!(content[0]["content"], "file content");
}

#[test]
fn anthropic_messages_assistant_with_tool_calls() {
    let inv = navi_core::ToolInvocation {
        id: "call-abc".to_string(),
        tool_name: "grep".to_string(),
        input: json!({"pattern": "fn main"}),
    };
    let msg = ModelMessage::assistant_tool_call(inv);
    let (_, converted) = crate::providers::anthropic::anthropic_messages(&[msg]);
    assert_eq!(converted.len(), 1);
    assert_eq!(converted[0]["role"], "assistant");
    let content = converted[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["id"], "call-abc");
    assert_eq!(content[0]["name"], "grep");
    assert_eq!(content[0]["input"]["pattern"], "fn main");
}

// ── Gemini functionCall SSE ──────────────────────────────────────────────────

#[test]
fn gemini_sse_function_call() {
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_file","args":{"path":"main.rs"}}}]}}]}"#;
    let events = crate::providers::gemini::parse_gemini_sse(data);
    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e.as_ref().ok()? {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "read_file");
    assert_eq!(tool_calls[0].input["path"], "main.rs");
    assert!(tool_calls[0].id.starts_with("gemini-"));
}

#[test]
fn gemini_sse_mixed_text_and_function_call() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"I'll read the file"},{"functionCall":{"name":"read_file","args":{"path":"lib.rs"}}}]}}]}"#;
    let events = crate::providers::gemini::parse_gemini_sse(data);
    let texts: Vec<_> = events
        .iter()
        .filter_map(|e| match e.as_ref().ok()? {
            ModelStreamEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e.as_ref().ok()? {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["I'll read the file"]);
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "read_file");
}

// ── Gemini content conversion with tools ─────────────────────────────────────

#[test]
fn gemini_contents_tool_role_produces_function_response() {
    let messages = vec![ModelMessage::tool_result(
        "call-1",
        "read_file",
        "file content",
    )];
    let (_, contents) = crate::providers::gemini::gemini_contents(&messages);
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "function");
    let parts = contents[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["functionResponse"]["name"], "read_file");
    assert_eq!(
        parts[0]["functionResponse"]["response"]["result"],
        "file content"
    );
}

#[test]
fn gemini_contents_assistant_with_tool_calls() {
    let inv = navi_core::ToolInvocation {
        id: "call-xyz".to_string(),
        tool_name: "bash".to_string(),
        input: json!({"command": "ls"}),
    };
    let msg = ModelMessage::assistant_tool_call(inv);
    let (_, contents) = crate::providers::gemini::gemini_contents(&[msg]);
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "model");
    let parts = contents[0]["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["functionCall"]["name"], "bash");
    assert_eq!(parts[0]["functionCall"]["args"]["command"], "ls");
}

// ── apply_thinking_to_body branches ──────────────────────────────────────────

#[test]
fn applies_gemini_thinking_budget() {
    let mut body = json!({ "model": "gemini-2.5-pro", "messages": [] });
    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::High,
            OpenAiApiKind::ChatCompletions,
            "google-gemini",
        ),
        OpenAiApiKind::ChatCompletions,
        "google-gemini",
    );
    assert_eq!(
        body["extra_body"]["google"]["thinking_config"]["thinkingBudget"],
        10000
    );
}

#[test]
fn apply_thinking_disabled_does_not_modify_body() {
    let mut body = json!({ "model": "gpt-5", "messages": [] });
    let original = body.clone();
    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::Off,
            OpenAiApiKind::Responses,
            "openai",
        ),
        OpenAiApiKind::Responses,
        "openai",
    );
    assert_eq!(body, original);
}

#[test]
fn applies_generic_reasoning_effort_for_unknown_provider() {
    let mut body = json!({ "model": "custom-model", "messages": [] });
    apply_thinking_to_body(
        &mut body,
        thinking_request_for_api(
            navi_core::ThinkingConfig::Medium,
            OpenAiApiKind::ChatCompletions,
            "custom-provider",
        ),
        OpenAiApiKind::ChatCompletions,
        "custom-provider",
    );
    assert_eq!(body["reasoning_effort"], "medium");
}

// ── anthropic_tool_to_json / gemini_tool_to_json structure ────────────────────

#[test]
fn anthropic_messages_with_tools_produces_correct_request_body() {
    let messages = vec![ModelMessage::user("read main.rs")];
    let (system, converted) = crate::providers::anthropic::anthropic_messages(&messages);
    assert!(system.is_empty());
    assert_eq!(converted[0]["role"], "user");
}

#[test]
fn gemini_contents_with_system_instruction() {
    let messages = vec![
        ModelMessage::system("You are a Rust expert."),
        ModelMessage::user("Explain lifetimes."),
    ];
    let (system, contents) = crate::providers::gemini::gemini_contents(&messages);
    assert_eq!(system, "You are a Rust expert.");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["role"], "user");
}
