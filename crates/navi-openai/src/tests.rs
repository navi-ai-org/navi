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
use navi_core::{ContentPart, ModelMessage, ModelProvider, ModelStreamEvent, ToolInvocation};
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
fn responses_tool_result_with_image_emits_followup_input_image() {
    let msg = ModelMessage::tool_result_with_parts(
        "call-img",
        "view_image",
        "image attached",
        vec![ContentPart::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }],
    );
    let items = responses_input_item_to_json(&msg);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["type"], "function_call_output");
    assert_eq!(items[0]["call_id"], "call-img");
    assert_eq!(items[1]["type"], "message");
    assert_eq!(items[1]["role"], "user");
    let content = items[1]["content"].as_array().expect("content array");
    assert!(
        content.iter().any(|p| p["type"] == "input_image"
            && p["image_url"]
                .as_str()
                .is_some_and(|u| u.starts_with("data:image/png;base64,"))),
        "expected input_image follow-up, got {content:?}"
    );
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
fn parses_chat_completions_inline_think_tags_as_thinking() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"hello <think>hidden reasoning</think> world"},"finish_reason":null}]}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![
            ModelStreamEvent::TextDelta {
                text: "hello ".to_string(),
            },
            ModelStreamEvent::ThinkingDelta {
                text: "hidden reasoning".to_string(),
            },
            ModelStreamEvent::TextDelta {
                text: " world".to_string(),
            },
        ]
    );
}

#[test]
fn parses_chat_completions_think_tags_split_across_chunks() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = Vec::new();
    for data in [
        r#"{"choices":[{"delta":{"content":"<thi"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"nk>hidden</thi"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"nk>visible"},"finish_reason":null}]}"#,
    ] {
        events.extend(
            parse_chat_completions_sse_with_state(data, &mut state)
                .into_iter()
                .map(Result::unwrap),
        );
    }

    assert_eq!(
        events,
        vec![
            ModelStreamEvent::ThinkingDelta {
                text: "hidden".to_string(),
            },
            ModelStreamEvent::TextDelta {
                text: "visible".to_string(),
            },
        ]
    );
}

#[test]
fn parses_chat_completions_unclosed_think_tag_as_thinking_until_done() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = parse_chat_completions_sse_with_state(
        r#"{"choices":[{"delta":{"content":"<think>hidden"},"finish_reason":null}]}"#,
        &mut state,
    )
    .into_iter()
    .map(Result::unwrap)
    .collect::<Vec<_>>();
    events.extend(
        parse_chat_completions_sse_with_state("[DONE]", &mut state)
            .into_iter()
            .map(Result::unwrap),
    );

    assert_eq!(
        events,
        vec![
            ModelStreamEvent::ThinkingDelta {
                text: "hidden".to_string(),
            },
            ModelStreamEvent::Done,
        ]
    );
}

#[test]
fn parses_chat_completions_inline_tool_call_xml_in_text() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"I will <tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"main.rs\"}}</tool_call> read the file"},"finish_reason":null}]}"#,
    );

    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "read_file");
    assert_eq!(tool_calls[0].input["path"], "main.rs");
    let text_deltas: Vec<&str> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    // In single-chunk processing, text before and after tool call is
    // concatenated into one TextDelta.
    let combined: String = text_deltas.iter().copied().collect();
    assert!(combined.contains("I will"), "combined text: {combined:?}");
    assert!(
        combined.contains("read the file"),
        "combined text: {combined:?}"
    );
}

#[test]
fn parses_chat_completions_tool_call_xml_with_minimal_prefix() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"<|minimal|>[<tool_call>{\"name\":\"bash\",\"args\":{\"command\":\"ls\"}}</tool_call>done"},"finish_reason":null}]}"#,
    );

    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    assert!(results.iter().any(|e| matches!(
        e,
        ModelStreamEvent::ToolCall(inv)
        if inv.tool_name == "bash" && inv.input["command"] == "ls"
    )));
    assert!(results.iter().any(|e| matches!(
        e,
        ModelStreamEvent::TextDelta { text } if text == "done"
    )));
}

#[test]
fn parses_chat_completions_tool_call_xml_array_of_calls() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"<tool_call>[{\"name\":\"read_file\",\"arguments\":{\"path\":\"a.rs\"}},{\"name\":\"grep\",\"args\":\"{\\\"pattern\\\":\\\"fn\\\"}\"}]</tool_call>"},"finish_reason":null}]}"#,
    );

    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(tool_calls[0].tool_name, "read_file");
    assert_eq!(tool_calls[1].tool_name, "grep");
    assert_eq!(tool_calls[1].input["pattern"], "fn");
}

#[test]
fn parses_chat_completions_tool_call_xml_split_across_chunks() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = Vec::new();
    for data in [
        r#"{"choices":[{"delta":{"content":"text <tool_ca"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"ll>{\"name\":\"bash\",\"args\":{\"command\":\"ls\"}}</tool_call> more"},"finish_reason":null}]}"#,
    ] {
        events.extend(
            parse_chat_completions_sse_with_state(data, &mut state)
                .into_iter()
                .map(Result::unwrap),
        );
    }

    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "bash");
    assert_eq!(tool_calls[0].input["command"], "ls");
    assert!(events.iter().any(|e| matches!(
        e,
        ModelStreamEvent::TextDelta { text } if text == "text "
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ModelStreamEvent::TextDelta { text } if text == " more"
    )));
}

#[test]
fn parses_chat_completions_tool_call_inside_think_tags() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = Vec::new();
    for data in [
        r#"{"choices":[{"delta":{"content":"<think>Let me use <tool_call>{\"name\":\"grep\",\"args\":{\"pattern\":\"x\"}}</tool_call></think> result"},"finish_reason":null}]}"#,
    ] {
        events.extend(
            parse_chat_completions_sse_with_state(data, &mut state)
                .into_iter()
                .map(Result::unwrap),
        );
    }

    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1, "tool call extracted from think block");
    assert_eq!(tool_calls[0].tool_name, "grep");
    assert!(events.iter().any(|e| matches!(
        e,
        ModelStreamEvent::ThinkingDelta { text } if text == "Let me use "
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        ModelStreamEvent::TextDelta { text } if text.contains("result")
    )));
}

#[test]
fn parses_chat_completions_unclosed_tool_call_drained_on_done() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = parse_chat_completions_sse_with_state(
        r#"{"choices":[{"delta":{"content":"<tool_call>{\"name\":\"bash\",\"args\":{\"command\":\"pwd\"}}"},"finish_reason":null}]}"#,
        &mut state,
    )
    .into_iter()
    .map(Result::unwrap)
    .collect::<Vec<_>>();
    events.extend(
        parse_chat_completions_sse_with_state("[DONE]", &mut state)
            .into_iter()
            .map(Result::unwrap),
    );

    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1, "unclosed tool_call drained on [DONE]");
    assert_eq!(tool_calls[0].tool_name, "bash");
}

#[test]
fn parses_chat_completions_tool_call_xml_with_minimax_bracket_prefix() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"before ]<]minimax[>[<tool_call>{\"name\":\"bash\",\"args\":{\"command\":\"ls\"}}</tool_call> after"},"finish_reason":null}]}"#,
    );

    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1, "tool call with minimax bracket prefix");
    assert_eq!(tool_calls[0].tool_name, "bash");
    assert_eq!(tool_calls[0].input["command"], "ls");
    let text_deltas: Vec<&str> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let combined: String = text_deltas.iter().copied().collect();
    assert!(combined.contains("before"), "combined: {combined:?}");
    assert!(combined.contains("after"), "combined: {combined:?}");
}

#[test]
fn parses_chat_completions_tool_call_xml_with_generic_bracket_prefix() {
    let events = parse_chat_completions_sse(
        r#"{"choices":[{"delta":{"content":"text ]<]some-engine[>[<tool_call>{\"name\":\"read_file\",\"arguments\":{\"path\":\"x.rs\"}}</tool_call> more"},"finish_reason":null}]}"#,
    );

    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1, "generic bracket prefix detected");
    assert_eq!(tool_calls[0].tool_name, "read_file");
    assert_eq!(tool_calls[0].input["path"], "x.rs");
    let text_deltas: Vec<&str> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let combined: String = text_deltas.iter().copied().collect();
    assert!(combined.contains("text"), "combined: {combined:?}");
    assert!(combined.contains("more"), "combined: {combined:?}");
}

#[test]
fn parses_tencent_hy_v3_tool_call_in_text() {
    let content = concat!(
        "I will inspect the file.\n",
        "<tool_calls:opensource>\n",
        "<tool_call:opensource>read_file<tool_sep:opensource>\n",
        "<arg_key:opensource>path</arg_key:opensource>\n",
        "<arg_value:opensource>IDEA.md</arg_value:opensource>\n",
        "</tool_call:opensource>\n",
        "</tool_calls:opensource>\n",
        "after",
    );
    let payload = serde_json::json!({
        "choices": [{
            "delta": { "content": content },
            "finish_reason": null
        }]
    });
    let events = parse_chat_completions_sse(&payload.to_string());
    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_calls.len(),
        1,
        "hy_v3 tool call extracted: {results:?}"
    );
    assert_eq!(tool_calls[0].tool_name, "read_file");
    assert_eq!(tool_calls[0].input["path"], "IDEA.md");
    let combined: String = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        combined.contains("I will inspect"),
        "text before: {combined:?}"
    );
    assert!(combined.contains("after"), "text after: {combined:?}");
    assert!(
        !combined.contains("tool_call") && !combined.contains("tool_calls"),
        "hy tags must not leak into text: {combined:?}"
    );
}

#[test]
fn parses_tencent_hy_v3_fs_browser_list() {
    let content = concat!(
        "<tool_call:opensource>fs_browser<tool_sep:opensource>\n",
        "<arg_key:opensource>action</arg_key:opensource>\n",
        "<arg_value:opensource>list</arg_value:opensource>\n",
        "<arg_key:opensource>path</arg_key:opensource>\n",
        "<arg_value:opensource>.</arg_value:opensource>\n",
        "</tool_call:opensource>",
    );
    let payload = serde_json::json!({
        "choices": [{ "delta": { "content": content }, "finish_reason": null }]
    });
    let events = parse_chat_completions_sse(&payload.to_string());
    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let tool_calls: Vec<_> = results
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "fs_browser");
    assert_eq!(tool_calls[0].input["action"], "list");
    assert_eq!(tool_calls[0].input["path"], ".");
}

#[test]
fn parses_tencent_hy_v3_tool_call_split_across_chunks() {
    let mut state = ChatToolCallAccumulator::default();
    let mut events = Vec::new();
    for data in [
        r#"{"choices":[{"delta":{"content":"go <tool_call:opensou"},"finish_reason":null}]}"#,
        r#"{"choices":[{"delta":{"content":"rce>bash<tool_sep:opensource>\n<arg_key:opensource>command</arg_key:opensource>\n<arg_value:opensource>ls</arg_value:opensource>\n</tool_call:opensource> ok"},"finish_reason":null}]}"#,
    ] {
        events.extend(
            parse_chat_completions_sse_with_state(data, &mut state)
                .into_iter()
                .map(Result::unwrap),
        );
    }
    let tool_calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ModelStreamEvent::ToolCall(inv) => Some(inv),
            _ => None,
        })
        .collect();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].tool_name, "bash");
    assert_eq!(tool_calls[0].input["command"], "ls");
}

#[test]
fn parses_tencent_hy_think_tags() {
    let content = "<think:opensource>planning next step</think:opensource>visible answer";
    let payload = serde_json::json!({
        "choices": [{ "delta": { "content": content }, "finish_reason": null }]
    });
    let events = parse_chat_completions_sse(&payload.to_string());
    let results: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    assert!(results.iter().any(|e| matches!(
        e,
        ModelStreamEvent::ThinkingDelta { text } if text.contains("planning next step")
    )));
    assert!(results.iter().any(|e| matches!(
        e,
        ModelStreamEvent::TextDelta { text } if text.contains("visible answer")
    )));
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
fn responses_tool_call_preserves_malformed_arguments() {
    let events = parse_openai_responses_sse(
        r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"read_file","arguments":"{\"path\":"}}"#,
    );

    assert_eq!(
        events.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
        vec![ModelStreamEvent::ToolCall(ToolInvocation {
            id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "raw_arguments": "{\"path\":" }),
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
fn sse_decoder_drains_final_ndjson_line() {
    let mut decoder = SseDecoder::default();

    let events = decoder.push_bytes(br#"{"type":"text-delta","text":"hello"}"#);
    let final_events = decoder.drain();

    assert!(events.is_empty());
    assert_eq!(
        final_events,
        vec![r#"{"type":"text-delta","text":"hello"}"#.to_string()]
    );
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

    let insufficient_quota_err = ProviderError::Api {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: r#"{"error":{"message":"You exceeded your current quota, please check your plan and billing details.","code":"insufficient_quota"}}"#.to_string(),
        requested_delay: None,
        body_read_error: None,
    };
    assert!(!should_retry_error(&insufficient_quota_err, true));

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
        instructions: None,
        messages: vec![ModelMessage {
            role: navi_core::ModelRole::User,
            content: "Hi".to_string(),
            content_parts: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: vec![],
            created_at: None,
            thinking_content: None,
        }],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
async fn chat_completions_omits_prompt_cache_fields_by_default() {
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_json(json!({
            "model": "minimaxai/minimax-m3",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true,
            "stream_options": {"include_usage": true}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "nvidia".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        base_url: Some(mock_server.uri()),
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::from_provider_config_with_key(&config, "test_key".to_string())
        .expect("provider");

    let request = navi_core::ModelRequest {
        model: "minimaxai/minimax-m3".to_string(),
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn opencode_zen_chat_completions_enables_parallel_tool_calls() {
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_json(json!({
            "model": "deepseek-v4-flash-free",
            "messages": [{"role": "user", "content": "Inspect files"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read a file",
                    "parameters": {"type": "object"}
                }
            }],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "stream": true,
            "stream_options": {"include_usage": true}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "opencode-zen".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        base_url: Some(mock_server.uri()),
        ..navi_core::ProviderConfig::default()
    };
    let provider = OpenAiProvider::from_provider_config_with_key(&config, "test_key".to_string())
        .expect("provider");
    let request = navi_core::ModelRequest {
        model: "deepseek-v4-flash-free".to_string(),
        instructions: None,
        messages: vec![ModelMessage::user("Inspect files".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![navi_core::ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            kind: navi_core::ToolKind::Read,
            input_schema: json!({"type": "object"}),
            metadata: navi_core::ToolMetadata::default(),
        }],
        session_id: None,
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
}

#[tokio::test]
async fn chat_completions_includes_configured_openai_prompt_cache_fields() {
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_json(json!({
            "model": "gpt-5",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": true,
            "stream_options": {"include_usage": true},
            "prompt_cache_key": "openai",
            "prompt_cache_retention": "24h"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                .insert_header("content-type", "text/event-stream"),
        )
        .expect(1)
        .mount(&mock_server)
        .await;

    let config = navi_core::ProviderConfig {
        id: "openai".to_string(),
        kind: navi_core::ProviderKind::OpenAiChatCompletions,
        base_url: Some(mock_server.uri()),
        request_options: Some(navi_core::ProviderRequestOptions {
            prompt_cache_key: Some("openai".to_string()),
            prompt_cache_retention: Some("24h".to_string()),
            ..Default::default()
        }),
        ..navi_core::ProviderConfig::default()
    };

    let provider = OpenAiProvider::from_provider_config_with_key(&config, "test_key".to_string())
        .expect("provider");

    let request = navi_core::ModelRequest {
        model: "gpt-5".to_string(),
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
    };

    let mut stream = provider.stream(request);
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
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
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
        instructions: None,
        messages: vec![ModelMessage::user("Hi".to_string())],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
        instructions: None,
        messages: vec![],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
        instructions: None,
        messages: vec![],
        thinking: navi_core::ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
fn parse_usage_accepts_float_and_string_token_counts() {
    // Aggregators (incl. some Charm Hyper / OpenAI-compat paths) emit floats.
    let usage = serde_json::json!({
        "prompt_tokens": 430.0,
        "completion_tokens": "12",
        "prompt_tokens_details": { "cached_tokens": 63570.0 }
    });
    let behavior = crate::providers::behavior::OpenAiBehavior;
    let normalized = behavior.parse_usage(&usage);
    assert_eq!(normalized.input_tokens, Some(430));
    assert_eq!(normalized.output_tokens, Some(12));
    assert_eq!(normalized.cache_read_tokens, Some(63_570));
}

#[test]
fn parse_usage_falls_back_to_total_tokens() {
    let usage = serde_json::json!({
        "total_tokens": 64100,
        "completion_tokens": 100
    });
    let behavior = crate::providers::behavior::OpenAiBehavior;
    let normalized = behavior.parse_usage(&usage);
    assert_eq!(normalized.input_tokens, Some(64_000));
    assert_eq!(normalized.output_tokens, Some(100));
}

#[test]
fn usage_from_value_caches_hypercredit_remaining() {
    let _ = crate::oauth::take_hypercredit_balance();
    let usage = serde_json::json!({
        "prompt_tokens": 430,
        "completion_tokens": 12,
        "remaining": { "hypercredits": 9876 }
    });
    let events = crate::mapping::usage_from_value(Some(&usage));
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].as_ref().unwrap(),
        ModelStreamEvent::Usage {
            input_tokens: Some(430),
            output_tokens: Some(12),
            ..
        }
    ));
    assert_eq!(crate::oauth::take_hypercredit_balance(), Some(9876.0));
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
    // User message is wrapped in array with cache_control (it's a stable user message).
    let user_content = converted[0]["content"].as_array().unwrap();
    assert_eq!(user_content[0]["type"], "text");
    assert_eq!(user_content[0]["text"], "Hello");
    assert_eq!(user_content[0]["cache_control"]["type"], "ephemeral");
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
    // Both get cache_control since we have budget for 2 system + 1 tool result + 1 user
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(system[1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn anthropic_messages_splits_runtime_context() {
    let messages = vec![ModelMessage::system(
        "Stable rules.\n\n=== Runtime Context ===\nCurrent project: /tmp/project",
    )];
    let (system, _) = crate::providers::anthropic::anthropic_messages(&messages);
    assert_eq!(system.len(), 2);
    assert_eq!(system[0]["text"], "Stable rules.");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(
        system[1]["text"],
        "=== Runtime Context ===\nCurrent project: /tmp/project"
    );
    assert!(system[1].get("cache_control").is_none());
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
    // Last tool result gets cache_control
    assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
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
fn anthropic_tool_definitions_cache_only_last_tool() {
    let tools = vec![
        navi_core::ToolDefinition {
            name: "a".to_string(),
            description: "A".to_string(),
            kind: navi_core::ToolKind::Read,
            input_schema: json!({"type":"object"}),
            metadata: Default::default(),
        },
        navi_core::ToolDefinition {
            name: "b".to_string(),
            description: "B".to_string(),
            kind: navi_core::ToolKind::Read,
            input_schema: json!({"type":"object"}),
            metadata: Default::default(),
        },
    ];
    let body = serde_json::to_value(crate::providers::anthropic::anthropic_tools_to_json(&tools))
        .expect("tools serialize");

    assert!(body[0].get("cache_control").is_none());
    assert_eq!(body[1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn anthropic_messages_last_stable_user_gets_cache_control() {
    let messages = vec![
        ModelMessage::system("Rules"),
        ModelMessage::user("Hello"),
        ModelMessage::assistant("Hi!"),
        ModelMessage::user("What is Rust?"),
    ];
    let (_, converted) = crate::providers::anthropic::anthropic_messages(&messages);
    // First user message (index 1) is stable — it has assistant messages after it.
    let content = converted[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Hello");
    assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
    // Last user message (index 3) is current turn — stays as plain string.
    assert!(
        converted[2]["content"].is_string(),
        "last user msg stays plain string"
    );
}

#[test]
fn anthropic_messages_breakpoint_budget_limits_to_four() {
    // 3 system messages + 1 tool result + 1 stable user = 5 candidates, but only 4 breakpoints.
    // Priority: system(3) + tool_result(1) = 4. User message skipped.
    let messages = vec![
        ModelMessage::system("Rule 1."),
        ModelMessage::system("Rule 2."),
        ModelMessage::system("Rule 3."),
        ModelMessage::user("Q1"),
        ModelMessage::assistant("A1"),
        ModelMessage::tool_result("call-1", "read_file", "content"),
        ModelMessage::user("Q2"),
    ];
    let (system, converted) = crate::providers::anthropic::anthropic_messages(&messages);

    // All 3 system blocks cached.
    let cached_system = system
        .iter()
        .filter(|s| s.get("cache_control").is_some())
        .count();
    assert_eq!(cached_system, 3, "all 3 system blocks should be cached");

    // The tool result should be cached (higher priority than user message).
    let tool_msg = converted
        .iter()
        .find(|m| {
            m["role"] == "user"
                && m["content"]
                    .as_array()
                    .map(|a| a.iter().any(|c| c["type"] == "tool_result"))
                    .unwrap_or(false)
        })
        .unwrap();
    let tool_content = tool_msg["content"].as_array().unwrap();
    assert_eq!(tool_content[0]["cache_control"]["type"], "ephemeral");

    // The stable user message (Q1) should NOT be cached (budget exhausted).
    let q1_msg = &converted[0];
    assert!(
        q1_msg["content"].is_string(),
        "Q1 stays plain string (no budget)"
    );
}

#[test]
fn model_supports_extended_cache_gpt5() {
    assert!(crate::providers::openai::model_supports_extended_cache(
        "gpt-5"
    ));
    assert!(crate::providers::openai::model_supports_extended_cache(
        "gpt-5.5"
    ));
    assert!(crate::providers::openai::model_supports_extended_cache(
        "gpt-5.5-pro"
    ));
    assert!(crate::providers::openai::model_supports_extended_cache(
        "gpt-5-codex"
    ));
    assert!(crate::providers::openai::model_supports_extended_cache(
        "gpt-4.1"
    ));
    assert!(!crate::providers::openai::model_supports_extended_cache(
        "gpt-4o"
    ));
    assert!(!crate::providers::openai::model_supports_extended_cache(
        "gpt-4o-mini"
    ));
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

#[test]
fn gemini_contents_serializes_audio_video_and_documents_inline() {
    let messages = vec![ModelMessage::user_multimodal(
        "Analyze media",
        vec![
            ContentPart::Text {
                text: "Analyze media".to_string(),
            },
            ContentPart::Audio {
                media_type: "audio/mpeg".to_string(),
                data: "audio-base64".to_string(),
                name: Some("clip.mp3".to_string()),
            },
            ContentPart::Video {
                media_type: "video/mp4".to_string(),
                data: "video-base64".to_string(),
                name: Some("clip.mp4".to_string()),
            },
            ContentPart::Document {
                media_type: "application/pdf".to_string(),
                data: "pdf-base64".to_string(),
                name: Some("paper.pdf".to_string()),
            },
        ],
    )];

    let (_, contents) = crate::providers::gemini::gemini_contents(&messages);
    let parts = contents[0]["parts"].as_array().expect("parts");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "audio/mpeg");
    assert_eq!(parts[1]["inlineData"]["data"], "audio-base64");
    assert_eq!(parts[2]["inlineData"]["mimeType"], "video/mp4");
    assert_eq!(parts[2]["inlineData"]["data"], "video-base64");
    assert_eq!(parts[3]["inlineData"]["mimeType"], "application/pdf");
    assert_eq!(parts[3]["inlineData"]["data"], "pdf-base64");
}
