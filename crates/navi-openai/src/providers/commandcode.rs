use crate::errors::ProviderError;
use crate::mapping::text_delta;
use crate::sse::SseDecoder;
use crate::transport::ensure_success;
use anyhow::Result;
use async_stream::try_stream;
use futures_util::StreamExt;
use navi_core::{ContentPart, ModelRequest, ModelStream, ModelStreamEvent, ToolInvocation};
use serde_json::{Value, json};
use std::time::Duration;

const COMMANDCODE_ALPHA_GENERATE: &str = "/alpha/generate";
const DEFAULT_CLI_VERSION: &str = "0.38.2";
const DEFAULT_CLI_USER_AGENT: &str = "command-code";

impl crate::provider::OpenAiProvider {
    pub(crate) fn stream_commandcode_alpha_generate(&self, request: ModelRequest) -> ModelStream {
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let provider_id = self.provider_id.clone();
        let stream_idle_timeout_ms = self.config.stream_idle_timeout_ms();

        Box::pin(try_stream! {
            let model = request.model.clone();
            tracing::info!(provider = %provider_id, model = %model, api = "commandcode-alpha-generate", tools = request.tools.len(), "provider stream started");

            let body = build_alpha_generate_body(&request);
            let url = format!(
                "{COMMANDCODE_ALPHA_GENERATE}",
            );
            let full_url = format!("{}{}", commandcode_api_base(&base_url), url);

            let cli_version = detect_commandcode_cli_version();
            let headers = build_commandcode_headers(&api_key, &cli_version);

            let response = client
                .post(full_url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(ProviderError::Transport)?;

            tracing::debug!(provider = %provider_id, model = %model, status = %response.status(), "provider stream response received");
            let response = ensure_success(response).await?;
            let mut decoder = SseDecoder::default();
            let mut text_accumulator = CommandCodeTextAccumulator::default();
            let mut chunks = response.bytes_stream();

            let idle_timeout = Duration::from_millis(stream_idle_timeout_ms);
            loop {
                let next_chunk = tokio::time::timeout(idle_timeout, chunks.next()).await;
                match next_chunk {
                    Ok(Some(chunk_res)) => {
                        let bytes = chunk_res.map_err(ProviderError::Transport)?;
                        for data in decoder.push_bytes(bytes.as_ref()) {
                            for event in parse_commandcode_sse_with_state(&data, &mut text_accumulator) {
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
            for data in decoder.drain() {
                for event in parse_commandcode_sse_with_state(&data, &mut text_accumulator) {
                    yield event?;
                }
            }
            for event in text_accumulator.drain_pending_text() {
                yield event?;
            }
            tracing::info!(provider = %provider_id, model = %model, "provider stream completed");
            yield ModelStreamEvent::Done;
        })
    }
}

fn detect_commandcode_cli_version() -> String {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return DEFAULT_CLI_VERSION.to_string(),
    };
    let pkg_path = format!("{home}/.bun/install/global/node_modules/command-code/package.json");
    let content = match std::fs::read_to_string(&pkg_path) {
        Ok(c) => c,
        Err(_) => return DEFAULT_CLI_VERSION.to_string(),
    };
    let pkg: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return DEFAULT_CLI_VERSION.to_string(),
    };
    pkg.get("version")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_CLI_VERSION)
        .to_string()
}

fn commandcode_api_base(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .strip_suffix("/provider/v1")
        .unwrap_or_else(|| base_url.trim_end_matches('/'))
        .to_string()
}

fn build_commandcode_headers(api_key: &str, cli_version: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}")).expect("valid auth header"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        "Accept",
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    let ua = format!("{DEFAULT_CLI_USER_AGENT}/{cli_version}");
    headers.insert(
        "User-Agent",
        HeaderValue::from_str(&ua).expect("valid user-agent"),
    );
    headers.insert(
        "x-command-code-version",
        HeaderValue::from_str(cli_version).expect("valid cli version"),
    );
    headers.insert(
        "x-session-id",
        HeaderValue::from_str(&format!("navi-{}", uuid_v4())).expect("valid session id"),
    );
    headers
}

fn build_alpha_generate_body(request: &ModelRequest) -> Value {
    let (system_prompt, messages) = commandcode_messages(&request.messages);

    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let date = chrono_now_date();
    let environment = format!("{}, Node.js {}", std::env::consts::OS, "navi");
    let (is_git_repo, current_branch, main_branch, git_status, recent_commits) =
        detect_git_context();

    let mut params = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": 32000,
        "stream": true,
    });

    if !system_prompt.is_empty() {
        params["system"] = json!(system_prompt);
    }

    if !request.tools.is_empty() {
        params["tools"] = json!(commandcode_tools(&request.tools));
    }

    json!({
        "config": {
            "workingDir": cwd,
            "date": date,
            "environment": environment,
            "structure": [],
            "isGitRepo": is_git_repo,
            "currentBranch": current_branch,
            "mainBranch": main_branch,
            "gitStatus": git_status,
            "recentCommits": recent_commits,
        },
        "memory": "",
        "taste": "",
        "skills": "",
        "permissionMode": "standard",
        "params": params,
        "threadId": uuid_v4(),
    })
}

fn commandcode_tools(tools: &[navi_core::ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.input_schema,
            })
        })
        .collect()
}

fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }
    let leap = is_leap(y);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1;
    for md in month_days {
        if days < md {
            break;
        }
        days -= md;
        m += 1;
    }
    (y, m, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let a: u64 = 6364136223846793005;
    let c: u64 = 1442695040888963407;
    let seed = nanos as u64;
    let r1 = seed.wrapping_mul(a).wrapping_add(c);
    let r2 = r1.wrapping_mul(a).wrapping_add(c);

    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&r1.to_be_bytes());
    bytes[8..].copy_from_slice(&r2.to_be_bytes());
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn commandcode_messages(messages: &[navi_core::ModelMessage]) -> (String, Vec<Value>) {
    let mut system_parts = Vec::new();
    let mut converted = Vec::new();

    for message in messages {
        match message.role {
            navi_core::ModelRole::System => {
                if !message.content.trim().is_empty() {
                    system_parts.push(message.content.clone());
                }
            }
            navi_core::ModelRole::User => {
                if !message.content_parts.is_empty() {
                    let content: Vec<Value> = message
                        .content_parts
                        .iter()
                        .map(|part| match part {
                            ContentPart::Text { text } => {
                                json!({ "type": "text", "text": text })
                            }
                            ContentPart::Image { media_type, data } => {
                                json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{media_type};base64,{data}")
                                    }
                                })
                            }
                        })
                        .collect();
                    converted.push(json!({ "role": "user", "content": content }));
                } else {
                    converted.push(json!({ "role": "user", "content": message.content }));
                }
            }
            navi_core::ModelRole::Assistant => {
                if message.tool_calls.is_empty() {
                    converted.push(json!({ "role": "assistant", "content": message.content }));
                } else {
                    let mut content = Vec::new();
                    for tool_call in &message.tool_calls {
                        content.push(json!({
                            "type": "tool-call",
                            "toolCallId": tool_call.id,
                            "toolName": tool_call.tool_name,
                            "input": tool_call.input,
                        }));
                    }
                    if !message.content.is_empty() {
                        content.push(json!({ "type": "text", "text": message.content }));
                    }
                    converted.push(json!({ "role": "assistant", "content": content }));
                }
            }
            navi_core::ModelRole::Tool => {
                converted.push(json!({
                    "role": "tool",
                    "content": [{
                        "type": "tool-result",
                        "toolCallId": message.tool_call_id.as_deref().unwrap_or("unknown"),
                        "toolName": message.tool_name.as_deref().unwrap_or("unknown"),
                        "output": { "type": "text", "value": message.content },
                    }],
                }));
            }
        }
    }

    if converted.is_empty() && !system_parts.is_empty() {
        converted.push(json!({ "role": "user", "content": system_parts.join("\n\n") }));
    }

    (system_parts.join("\n\n"), converted)
}

fn detect_git_context() -> (bool, String, String, String, Vec<String>) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let git_dir = cwd.join(".git");
    let is_git = git_dir.exists();
    if !is_git {
        return (
            false,
            String::new(),
            "main".to_string(),
            String::new(),
            vec![],
        );
    }
    let current_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "main".to_string());
    let git_status = std::process::Command::new("git")
        .args(["status", "--short"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let recent_commits: Vec<String> = std::process::Command::new("git")
        .args(["log", "--oneline", "-5"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default();
    (
        true,
        current_branch,
        "main".to_string(),
        git_status,
        recent_commits,
    )
}

#[allow(dead_code)]
fn parse_commandcode_sse(data: &str) -> Vec<Result<ModelStreamEvent>> {
    parse_commandcode_sse_with_state(data, &mut CommandCodeTextAccumulator::default())
}

fn parse_commandcode_sse_with_state(
    data: &str,
    text_accumulator: &mut CommandCodeTextAccumulator,
) -> Vec<Result<ModelStreamEvent>> {
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let value = match serde_json::from_str::<Value>(trimmed) {
        Ok(v) => v,
        Err(err) => return vec![Err(err.into())],
    };

    match value.get("type").and_then(Value::as_str) {
        Some("text-delta") => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                if text.is_empty() {
                    Vec::new()
                } else {
                    text_accumulator.push_text(text)
                }
            } else {
                Vec::new()
            }
        }
        Some("reasoning-delta") => {
            if let Some(text) = value.get("text").and_then(Value::as_str) {
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![Ok(ModelStreamEvent::ThinkingDelta {
                        text: text.to_string(),
                    })]
                }
            } else {
                Vec::new()
            }
        }
        Some("tool-call") => {
            let mut events = text_accumulator.drain_pending_text();
            let tool_call_id = value
                .get("toolCallId")
                .or_else(|| value.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let tool_name = value
                .get("toolName")
                .or_else(|| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let input = value
                .get("input")
                .or_else(|| value.get("args"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            events.push(Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                id: tool_call_id,
                tool_name,
                input,
            })));
            events
        }
        Some("tool-result") => Vec::new(),
        Some("text-start")
        | Some("text-end")
        | Some("reasoning-start")
        | Some("reasoning-end")
        | Some("start")
        | Some("start-step")
        | Some("provider-metadata") => Vec::new(),
        Some("finish-step") => {
            let events = parse_usage_from_finish(&value);
            events
        }
        Some("finish") => {
            let mut events = text_accumulator.drain_pending_text();
            events.extend(parse_usage_from_finish(&value));
            events.push(Ok(ModelStreamEvent::Done));
            events
        }
        Some("error") => vec![Err(commandcode_stream_error(&value).into())],
        _ => Vec::new(),
    }
}

fn commandcode_stream_error(value: &Value) -> ProviderError {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| value.get("error").and_then(Value::as_str))
        .unwrap_or("Command Code stream error");
    let status = error
        .get("statusCode")
        .or_else(|| error.get("status"))
        .or_else(|| value.get("statusCode"))
        .and_then(Value::as_u64);
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .or_else(|| value.get("code"))
        .and_then(Value::as_str);

    let mut detail = format!("Command Code stream error: {message}");
    if let Some(code) = code {
        detail.push_str(&format!(" ({code})"));
    }
    if let Some(status) = status {
        detail.push_str(&format!(" [status {status}]"));
    }

    ProviderError::Other(detail)
}

#[derive(Default)]
struct CommandCodeTextAccumulator {
    pending: String,
    active_tool_call: Option<ActiveTextToolCall>,
    next_tool_call_index: u64,
}

struct ActiveTextToolCall {
    tool_name: Option<String>,
    end_tag: String,
}

impl CommandCodeTextAccumulator {
    fn push_text(&mut self, text: &str) -> Vec<Result<ModelStreamEvent>> {
        self.pending.push_str(text);
        self.drain(false)
    }

    fn drain_pending_text(&mut self) -> Vec<Result<ModelStreamEvent>> {
        self.drain(true)
    }

    fn drain(&mut self, final_chunk: bool) -> Vec<Result<ModelStreamEvent>> {
        let mut events = Vec::new();

        loop {
            if let Some(active) = &self.active_tool_call {
                let end_tag = active.end_tag.clone();
                if let Some(end) = find_ascii_case_insensitive(&self.pending, &end_tag) {
                    let block = self.pending[..end].to_string();
                    self.pending.drain(..end + end_tag.len());
                    let active = self.active_tool_call.take().expect("active tool call");
                    events.extend(self.parse_tool_call_block(&block, active.tool_name.as_deref()));
                    continue;
                }

                if final_chunk {
                    let block = std::mem::take(&mut self.pending);
                    let active = self.active_tool_call.take().expect("active tool call");
                    events.extend(self.parse_tool_call_block(&block, active.tool_name.as_deref()));
                }
                break;
            }

            if let Some(start) = find_tool_call_start(&self.pending) {
                if start.position > 0 {
                    events.push(text_delta(&self.pending[..start.position]));
                }
                self.pending.drain(..start.position + start.marker_len);
                self.active_tool_call = Some(ActiveTextToolCall {
                    tool_name: start.tool_name,
                    end_tag: start.end_tag,
                });
                continue;
            }

            let keep = if final_chunk {
                0
            } else {
                partial_tool_call_start_suffix_len(&self.pending)
            };
            let emit_len = self.pending.len().saturating_sub(keep);
            if emit_len > 0 {
                events.push(text_delta(&self.pending[..emit_len]));
                self.pending.drain(..emit_len);
            }
            break;
        }

        events
    }

    fn parse_tool_call_block(
        &mut self,
        block: &str,
        tool_name: Option<&str>,
    ) -> Vec<Result<ModelStreamEvent>> {
        if let Some(tool_name) = tool_name
            && let Some(invocation) = self.named_tool_invocation(tool_name, block)
        {
            return vec![Ok(ModelStreamEvent::ToolCall(invocation))];
        }

        parse_tool_call_values(block)
            .into_iter()
            .filter_map(|value| self.tool_invocation_from_value(value))
            .map(|invocation| Ok(ModelStreamEvent::ToolCall(invocation)))
            .collect()
    }

    fn named_tool_invocation(&mut self, tool_name: &str, block: &str) -> Option<ToolInvocation> {
        let input = parse_named_tool_input(tool_name, block)?;
        let id = format!("commandcode-text-tool-{}", self.next_tool_call_index);
        self.next_tool_call_index += 1;

        Some(ToolInvocation {
            id,
            tool_name: tool_name.to_string(),
            input,
        })
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
                let id = format!("commandcode-text-tool-{}", self.next_tool_call_index);
                self.next_tool_call_index += 1;
                id
            });
        let input = value
            .get("arguments")
            .or_else(|| value.get("input"))
            .or_else(|| value.get("args"))
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

fn parse_named_tool_input(tool_name: &str, block: &str) -> Option<Value> {
    let trimmed = block.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(match value {
            Value::Object(_) => value,
            value => json!({ "value": value }),
        });
    }

    if tool_name == "tool_workflow"
        && let Some(script) = parse_script_field(trimmed)
    {
        return Some(json!({ "script": script }));
    }

    None
}

fn parse_script_field(text: &str) -> Option<String> {
    let script_pos = find_ascii_case_insensitive(text, "script")?;
    let mut rest = &text[script_pos + "script".len()..];
    rest = rest.trim_start();
    rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='))?;
    rest = rest.trim_start();
    parse_quoted_value(rest).or_else(|| (!rest.is_empty()).then(|| rest.trim().to_string()))
}

fn parse_quoted_value(text: &str) -> Option<String> {
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in text[quote.len_utf8()..].chars() {
        if escaped {
            if ch == quote || ch == '\\' {
                value.push(ch);
            } else {
                value.push('\\');
                value.push(ch);
            }
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return Some(value);
        }
        value.push(ch);
    }

    None
}

struct ToolCallStart {
    position: usize,
    marker_len: usize,
    tool_name: Option<String>,
    end_tag: String,
}

fn find_tool_call_start(text: &str) -> Option<ToolCallStart> {
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
            find_ascii_case_insensitive(text, marker).map(|pos| ToolCallStart {
                position: pos,
                marker_len: marker.len(),
                tool_name: None,
                end_tag: "</tool_call>".to_string(),
            })
        })
        .chain(named_tool_tags().iter().filter_map(|tag| {
            find_ascii_case_insensitive(text, tag.start_tag).map(|pos| ToolCallStart {
                position: pos,
                marker_len: tag.start_tag.len(),
                tool_name: Some(tag.tool_name.to_string()),
                end_tag: tag.end_tag.to_string(),
            })
        }))
        .min_by_key(|start| start.position)
    {
        return Some(result);
    }

    find_generic_bracket_tool_call_prefix(text)
}

struct NamedToolTag {
    tool_name: &'static str,
    start_tag: &'static str,
    end_tag: &'static str,
}

fn named_tool_tags() -> &'static [NamedToolTag] {
    &[NamedToolTag {
        tool_name: "tool_workflow",
        start_tag: "<tool_workflow>",
        end_tag: "</tool_workflow>",
    }]
}

fn find_generic_bracket_tool_call_prefix(text: &str) -> Option<ToolCallStart> {
    let tc_pos = find_ascii_case_insensitive(text, "<tool_call>")?;
    let before = &text[..tc_pos];
    let bracket_end = before.rfind(">[")?;
    if bracket_end > before.len().saturating_sub(64) && bracket_end >= 1 {
        let candidate = &before[..bracket_end];
        let openers = [']', '<', '|'];
        if let Some(prefix_start) = candidate.rfind(|c: char| openers.contains(&c)) {
            let full_len = tc_pos + "<tool_call>".len() - prefix_start;
            return Some(ToolCallStart {
                position: prefix_start,
                marker_len: full_len,
                tool_name: None,
                end_tag: "</tool_call>".to_string(),
            });
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
        "<tool_workflow>",
    ];

    let specific = patterns
        .iter()
        .map(|marker| partial_ascii_suffix_len(text, marker))
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

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

fn partial_ascii_suffix_len(text: &str, marker: &str) -> usize {
    let bytes = text.as_bytes();
    let marker_bytes = marker.as_bytes();
    let max_len = bytes.len().min(marker_bytes.len().saturating_sub(1));
    for len in (1..=max_len).rev() {
        if bytes[bytes.len() - len..].eq_ignore_ascii_case(&marker_bytes[..len]) {
            return len;
        }
    }
    0
}

fn parse_usage_from_finish(value: &Value) -> Vec<Result<ModelStreamEvent>> {
    let usage = value.get("usage").or_else(|| value.get("totalUsage"));
    let Some(usage) = usage else {
        return Vec::new();
    };
    let input_tokens = usage
        .get("inputTokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("outputTokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(Value::as_u64);
    let cache_read_tokens = usage
        .get("inputTokenDetails")
        .and_then(|d| d.get("cacheReadTokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
        })
        .and_then(Value::as_u64);
    if input_tokens.is_some() || output_tokens.is_some() {
        vec![Ok(ModelStreamEvent::Usage {
            input_tokens,
            output_tokens,
            cache_creation_tokens: None,
            cache_read_tokens,
        })]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_delta_event() {
        let events = parse_commandcode_sse(r#"{"type":"text-delta","id":"txt-0","text":"hello"}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(ModelStreamEvent::TextDelta { text }) => assert_eq!(text, "hello"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_reasoning_delta_event() {
        let events =
            parse_commandcode_sse(r#"{"type":"reasoning-delta","id":"r-0","text":"thinking..."}"#);
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(ModelStreamEvent::ThinkingDelta { text }) => assert_eq!(text, "thinking..."),
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_tool_call_event() {
        let events = parse_commandcode_sse(
            r#"{"type":"tool-call","toolCallId":"tc-1","toolName":"read_file","input":{"absolutePath":"/tmp/x"}}"#,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(ModelStreamEvent::ToolCall(inv)) => {
                assert_eq!(inv.id, "tc-1");
                assert_eq!(inv.tool_name, "read_file");
                assert_eq!(inv.input["absolutePath"], "/tmp/x");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_textual_tool_call_block() {
        let events = parse_commandcode_sse(
            r#"{"type":"text-delta","text":"I'll check them.]<|minimal|>[<tool_call>\n{\"name\":\"bash\",\"arguments\":{\"command\":\"opencode mcp list\"}}\n{\"name\":\"bash\",\"arguments\":{\"command\":\"navi --help\"}}\n</tool_call>"}"#,
        )
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();

        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            ModelStreamEvent::TextDelta {
                text: "I'll check them.".to_string(),
            }
        );
        match &events[1] {
            ModelStreamEvent::ToolCall(invocation) => {
                assert_eq!(invocation.id, "commandcode-text-tool-0");
                assert_eq!(invocation.tool_name, "bash");
                assert_eq!(invocation.input["command"], "opencode mcp list");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        match &events[2] {
            ModelStreamEvent::ToolCall(invocation) => {
                assert_eq!(invocation.id, "commandcode-text-tool-1");
                assert_eq!(invocation.tool_name, "bash");
                assert_eq!(invocation.input["command"], "navi --help");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn parse_textual_tool_call_block_split_across_chunks() {
        let mut state = CommandCodeTextAccumulator::default();
        let mut events = Vec::new();

        for data in [
            r#"{"type":"text-delta","text":"Before]<|mini"}"#,
            r#"{"type":"text-delta","text":"mal|>[<tool_call>\n{\"name\":\"bash\",\"arguments\":{\"command\":\"whoami\"}}"}"#,
            r#"{"type":"text-delta","text":"\n</tool_call> after"}"#,
            r#"{"type":"finish","finishReason":"stop"}"#,
        ] {
            events.extend(
                parse_commandcode_sse_with_state(data, &mut state)
                    .into_iter()
                    .map(Result::unwrap),
            );
        }

        assert_eq!(events.len(), 4);
        assert_eq!(
            events[0],
            ModelStreamEvent::TextDelta {
                text: "Before".to_string(),
            }
        );
        match &events[1] {
            ModelStreamEvent::ToolCall(invocation) => {
                assert_eq!(invocation.tool_name, "bash");
                assert_eq!(invocation.input["command"], "whoami");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        assert_eq!(
            events[2],
            ModelStreamEvent::TextDelta {
                text: " after".to_string(),
            }
        );
        assert!(matches!(events[3], ModelStreamEvent::Done));
    }

    #[test]
    fn parse_named_tool_workflow_block() {
        let data = json!({
            "type": "text-delta",
            "text": "Before <tool_workflow>\nscript: \"\ndef workflow():\n    thumb = read_file('psyche-api/src/thumbnails.rs')\n    return thumb\nworkflow()\n\"\n</tool_workflow> after",
        })
        .to_string();
        let events = parse_commandcode_sse(&data)
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 3);
        assert_eq!(
            events[0],
            ModelStreamEvent::TextDelta {
                text: "Before ".to_string(),
            }
        );
        match &events[1] {
            ModelStreamEvent::ToolCall(invocation) => {
                assert_eq!(invocation.id, "commandcode-text-tool-0");
                assert_eq!(invocation.tool_name, "tool_workflow");
                assert!(
                    invocation.input["script"]
                        .as_str()
                        .unwrap()
                        .contains("def workflow()")
                );
                assert!(
                    invocation.input["script"]
                        .as_str()
                        .unwrap()
                        .contains("read_file('psyche-api/src/thumbnails.rs')")
                );
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
        assert_eq!(
            events[2],
            ModelStreamEvent::TextDelta {
                text: " after".to_string(),
            }
        );
    }

    #[test]
    fn parse_finish_event_with_usage() {
        let events = parse_commandcode_sse(
            r#"{"type":"finish","finishReason":"stop","totalUsage":{"inputTokens":100,"outputTokens":20}}"#,
        );
        assert_eq!(events.len(), 2);
        match &events[0] {
            Ok(ModelStreamEvent::Usage {
                input_tokens,
                output_tokens,
                ..
            }) => {
                assert_eq!(*input_tokens, Some(100));
                assert_eq!(*output_tokens, Some(20));
            }
            other => panic!("expected Usage, got {other:?}"),
        }
        assert!(matches!(events[1], Ok(ModelStreamEvent::Done)));
    }

    #[test]
    fn parse_error_event_returns_provider_error() {
        let events = parse_commandcode_sse(
            r#"{"type":"error","error":{"message":"quota exhausted","code":"rate_limit","statusCode":429}}"#,
        );
        assert_eq!(events.len(), 1);
        let err = events.into_iter().next().unwrap().unwrap_err().to_string();
        assert!(err.contains("quota exhausted"));
        assert!(err.contains("rate_limit"));
        assert!(err.contains("429"));
    }

    #[test]
    fn parse_finish_step_with_usage() {
        let events = parse_commandcode_sse(
            r#"{"type":"finish-step","finishReason":"stop","usage":{"inputTokens":50,"outputTokens":10}}"#,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Ok(ModelStreamEvent::Usage {
                input_tokens,
                output_tokens,
                ..
            }) => {
                assert_eq!(*input_tokens, Some(50));
                assert_eq!(*output_tokens, Some(10));
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn parse_start_and_metadata_are_noop() {
        assert!(parse_commandcode_sse(r#"{"type":"start"}"#).is_empty());
        assert!(parse_commandcode_sse(r#"{"type":"start-step","request":{}}"#).is_empty());
        assert!(
            parse_commandcode_sse(r#"{"type":"provider-metadata","providerMetadata":{}}"#)
                .is_empty()
        );
    }

    #[test]
    fn parse_empty_and_whitespace_are_noop() {
        assert!(parse_commandcode_sse("").is_empty());
        assert!(parse_commandcode_sse("  \n  ").is_empty());
    }

    #[test]
    fn build_body_includes_required_config_fields() {
        use navi_core::{ModelMessage, ThinkingConfig};
        let request = ModelRequest {
            model: "xiaomi/mimo-v2.5-pro".to_string(),
            messages: vec![ModelMessage::user("test")],
            thinking: ThinkingConfig::Off,
            tools: vec![],
        };
        let body = build_alpha_generate_body(&request);
        assert_eq!(body["params"]["model"], "xiaomi/mimo-v2.5-pro");
        assert_eq!(body["params"]["stream"], true);
        assert!(body["config"]["workingDir"].is_string());
        assert!(body["config"]["date"].is_string());
        assert!(body["config"]["environment"].is_string());
        assert!(body["config"]["isGitRepo"].is_boolean());
        assert!(body["threadId"].is_string());
    }

    #[test]
    fn build_body_converts_system_prompt_and_tool_calls() {
        use navi_core::{ModelMessage, ThinkingConfig, ToolInvocation};
        use serde_json::json;

        let request = ModelRequest {
            model: "xiaomi/mimo-v2.5-pro".to_string(),
            messages: vec![
                ModelMessage::system("be helpful"),
                ModelMessage::user("hi"),
                ModelMessage::assistant_tool_calls_with_context(
                    vec![ToolInvocation {
                        id: "call1".to_string(),
                        tool_name: "bash".to_string(),
                        input: json!({"command": "echo ok"}),
                    }],
                    "",
                    None,
                ),
                ModelMessage::tool_result("call1", "bash", r#"{"stdout":"ok"}"#),
            ],
            thinking: ThinkingConfig::Off,
            tools: vec![],
        };

        let body = build_alpha_generate_body(&request);
        let messages = body["params"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool-call");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["content"][0]["type"], "tool-result");
    }

    #[test]
    fn build_body_includes_native_tool_definitions() {
        use navi_core::{ModelMessage, ThinkingConfig, ToolDefinition, ToolKind};
        use serde_json::json;

        let request = ModelRequest {
            model: "claude-sonnet-4-6".to_string(),
            messages: vec![ModelMessage::user("test")],
            thinking: ThinkingConfig::Off,
            tools: vec![ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                kind: ToolKind::Read,
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" }
                    },
                    "required": ["path"]
                }),
                ..Default::default()
            }],
        };

        let body = build_alpha_generate_body(&request);
        let tools = body["params"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "read_file");
        assert_eq!(tools[0]["description"], "Read a file");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
        assert_eq!(tools[0]["input_schema"]["required"][0], "path");
    }

    #[test]
    fn build_body_uses_commandcode_image_parts() {
        use navi_core::{ContentPart, ModelMessage, ThinkingConfig};

        let request = ModelRequest {
            model: "minimax-m3".to_string(),
            messages: vec![ModelMessage::user_multimodal(
                "describe this image",
                vec![
                    ContentPart::Text {
                        text: "describe this image".to_string(),
                    },
                    ContentPart::Image {
                        media_type: "image/png".to_string(),
                        data: "abc123".to_string(),
                    },
                ],
            )],
            thinking: ThinkingConfig::Off,
            tools: vec![],
        };

        let body = build_alpha_generate_body(&request);
        let content = body["params"]["messages"][0]["content"].as_array().unwrap();

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "describe this image");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,abc123"
        );
    }

    #[test]
    fn commandcode_api_base_strips_legacy_provider_path() {
        assert_eq!(
            commandcode_api_base("https://api.commandcode.ai/provider/v1"),
            "https://api.commandcode.ai"
        );
        assert_eq!(
            commandcode_api_base("https://api.commandcode.ai"),
            "https://api.commandcode.ai"
        );
    }

    #[test]
    fn detect_cli_version_falls_back_to_default() {
        let version = detect_commandcode_cli_version();
        assert!(!version.is_empty());
    }
}
