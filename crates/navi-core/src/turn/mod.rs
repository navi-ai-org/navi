use crate::cancel::CancelToken;
use crate::compact::{self, CompactState};
use crate::context::ContextPacket;
use crate::event::{AgentEvent, RepetitionWarningKind};
use crate::harness::{
    AgentRunState, HarnessPolicy, HarnessStop, HarnessStopReason, ToolLoopDecision,
    compact_tool_observation, record_tool_call, record_tool_result, tool_error_result,
    trace_request_summary,
};
use crate::model::{
    ModelMessage, ModelProvider, ModelRequest, ModelRole, ModelStreamEvent, ThinkingConfig,
};
use crate::prompt::{PromptCache, SystemPromptInput, SystemPromptRenderer};
use crate::security::SecurityDecision;
use crate::skills::SkillManifest;
use crate::tool::ToolExecutor;
use anyhow::Result;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const QUESTION_TOOL_NAME: &str = "question";

struct ModelTurnOutput {
    text: String,
    thinking: String,
    tool_calls: Vec<crate::tool::ToolInvocation>,
}

type ToolExecutionResult = (crate::tool::ToolInvocation, crate::tool::ToolResult, String);

pub struct TurnContext {
    pub model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    pub tool_executor: Arc<ToolExecutor>,
    pub project_dir: PathBuf,
    pub model_name: Arc<RwLock<String>>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    pub approval_resolver: crate::runtime::ApprovalResolver,
    pub question_resolver: crate::runtime::QuestionResolver,
    pub compact_state: Arc<tokio::sync::Mutex<CompactState>>,
    pub harness_config: crate::config::HarnessConfig,
    pub include_tool_prompt_manifest: bool,
    pub context_packets: Arc<std::sync::Mutex<Vec<ContextPacket>>>,
    pub active_skills: Arc<std::sync::Mutex<Vec<SkillManifest>>>,
    pub prompt_cache: Arc<PromptCache>,
    pub cancel_token: CancelToken,
    /// Snapshot of the active `NaviConfig` taken at turn start. Used by
    /// `ensure_system_prompt` so the model sees the user-configured harness
    /// profile, model and provider rather than the defaults.
    pub config: Arc<RwLock<crate::config::NaviConfig>>,
    /// Optional separate provider for compaction/summarization. When set,
    /// auto_compact uses this instead of the main model provider.
    pub compaction_provider: Option<Arc<dyn ModelProvider>>,
    /// Model name for the compaction provider.
    pub compaction_model_name: Option<String>,
    pub session_id: String,
}

impl TurnContext {
    pub fn active_model_provider(&self) -> Arc<dyn ModelProvider> {
        self.model_provider
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn active_model_name(&self) -> String {
        self.model_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn active_config(&self) -> crate::config::NaviConfig {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn cancellation_requested(&self) -> bool {
        self.cancel_token.is_requested()
    }

    pub fn resolve_approval(&self, decision: crate::event::ApprovalDecision) -> bool {
        self.approval_resolver.resolve(decision)
    }
}

pub struct Prompt {
    pub input: Vec<ModelMessage>,
    pub tools: Vec<crate::tool::ToolDefinition>,
    pub base_instructions: String,
}

pub async fn run_turn(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
    policy: HarnessPolicy,
) -> Result<String> {
    ensure_not_cancelled(ctx)?;
    ensure_system_prompt(ctx, messages).await;

    let mut run_state = AgentRunState::default();
    let mut loop_count = 0;
    let final_text = loop {
        loop_count += 1;
        if let Some(limit) = policy.max_turn_loops {
            if loop_count > limit {
                let stop = HarnessStop {
                    reason: HarnessStopReason::TurnLoopLimit,
                    message: format!(
                        "Loop execution limit reached ({} iterations); stopping to avoid an uncontrolled turn",
                        limit
                    ),
                    tool_name: None,
                };
                let text = finalize_harness_stop(ctx, messages, stop);
                break text;
            }
        }
        ensure_not_cancelled(ctx)?;
        maintain_context_budget(ctx, messages).await;
        ensure_not_cancelled(ctx)?;

        let request = build_model_request(ctx, messages);
        emit_request_trace(ctx, &request, policy);

        let output = collect_model_output(ctx, request).await?;
        ensure_not_cancelled(ctx)?;

        if !output.tool_calls.is_empty() {
            if let Some(text) =
                handle_tool_calls(ctx, messages, &mut run_state, policy, output).await
            {
                break text;
            }
            continue;
        }

        persist_final_model_output(ctx, messages, &output);
        break output.text;
    };

    let _ = sync_messages_to_history(ctx, messages);
    Ok(final_text)
}

fn ensure_not_cancelled(ctx: &TurnContext) -> Result<()> {
    if ctx.cancellation_requested() {
        Err(anyhow::anyhow!("turn cancelled"))
    } else {
        Ok(())
    }
}

async fn ensure_system_prompt(ctx: &TurnContext, messages: &mut Vec<ModelMessage>) {
    let context_packets = ctx
        .context_packets
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let active_skills = ctx
        .active_skills
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let input = SystemPromptInput {
        config: ctx.active_config(),
        project_dir: ctx.project_dir.clone(),
        memory_injection: None,
        tools: ctx.tool_executor.definitions(),
        include_tool_prompt_manifest: ctx.include_tool_prompt_manifest,
        context_packets,
        active_skills,
    };
    let renderer = SystemPromptRenderer::new(ctx.prompt_cache.clone());
    let system_content = tokio::task::spawn_blocking(move || renderer.render(input))
        .await
        .unwrap_or_else(|_| "Default NAVI base instructions".to_string());

    if messages.is_empty() {
        messages.push(ModelMessage::system(system_content));
    } else if messages[0].role == ModelRole::System {
        messages[0].content = system_content;
    } else {
        messages.insert(0, ModelMessage::system(system_content));
    }
}

async fn maintain_context_budget(ctx: &TurnContext, messages: &mut Vec<ModelMessage>) {
    let _ = sync_messages_to_history(ctx, messages);
    if let Ok(true) = evaluate_memory_triggers(ctx, messages).await {
        return;
    }

    let cleared = compact::micro_compact(messages, ctx.harness_config.micro_compact_gap_minutes);
    if cleared > 0 {
        tracing::info!(cleared, "micro-compact applied");
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::MicroCompactApplied {
                messages_cleared: cleared,
            });
        }
    }

    let should_autocompact = {
        let state = ctx.compact_state.lock().await;
        state.should_autocompact(ctx.harness_config.autocompact_buffer_tokens)
    };
    if !should_autocompact {
        return;
    }

    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::AutoCompactStarted);
    }
    let mut state = ctx.compact_state.lock().await;
    let compaction_provider: Arc<dyn ModelProvider>;
    let compaction_model: String;
    if let Some(ref cp) = ctx.compaction_provider {
        compaction_provider = cp.clone();
        compaction_model = ctx
            .compaction_model_name
            .clone()
            .unwrap_or_else(|| ctx.active_model_name());
    } else {
        compaction_provider = ctx.active_model_provider();
        compaction_model = ctx.active_model_name();
    }
    match state
        .auto_compact(
            messages,
            compaction_provider.as_ref(),
            &compaction_model,
            &ctx.harness_config,
        )
        .await
    {
        Ok(Some(tokens_saved)) => {
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactCompleted { tokens_saved });
            }
        }
        Ok(None) => {}
        Err(e) => {
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactFailed {
                    reason: e.to_string(),
                });
            }
        }
    }
}

fn build_model_request(ctx: &TurnContext, messages: &[ModelMessage]) -> ModelRequest {
    let config = ctx.active_config();
    let thinking_level = config.tui.thinking_level.trim().to_lowercase();

    let thinking = match thinking_level.as_str() {
        "adaptive" => {
            let tool_names: Vec<String> = ctx.tool_executor.tool_names();
            ThinkingConfig::resolve_adaptive(messages, &tool_names, 0)
        }
        "max" => ThinkingConfig::Max,
        "high" => ThinkingConfig::High,
        "medium" => ThinkingConfig::Medium,
        "low" => ThinkingConfig::Low,
        "off" => ThinkingConfig::Off,
        _ => ThinkingConfig::Adaptive,
    };

    ModelRequest {
        model: ctx.active_model_name(),
        messages: messages.to_vec(),
        thinking,
        tools: match crate::config::effective_tool_calling_mode(&config) {
            crate::config::ToolCallingMode::Native => ctx.tool_executor.definitions(),
            crate::config::ToolCallingMode::TextExtracted
            | crate::config::ToolCallingMode::ManifestOnly
            | crate::config::ToolCallingMode::Disabled => Vec::new(),
        },
    }
}

fn emit_request_trace(ctx: &TurnContext, request: &ModelRequest, policy: HarnessPolicy) {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::HarnessTrace(trace_request_summary(
            request, policy,
        )));
    }

    tracing::info!(
        model = %request.model,
        messages = request.messages.len(),
        tools = request.tools.len(),
        "turn request started"
    );
}

fn finalize_harness_stop(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
    stop: HarnessStop,
) -> String {
    emit_harness_stop(ctx, &stop);
    let text = persist_harness_stop_output(messages, &stop);
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::ModelOutput {
            text: text.clone(),
            thinking: None,
        });
    }
    text
}

fn emit_harness_stop(ctx: &TurnContext, stop: &HarnessStop) {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::HarnessStopped {
            reason: stop.reason.as_str().to_string(),
            message: stop.message.clone(),
            tool_name: stop.tool_name.clone(),
        });
    }
}

fn persist_harness_stop_output(messages: &mut Vec<ModelMessage>, stop: &HarnessStop) -> String {
    let mut text = format!(
        "Interrompi a execução porque o harness detectou `{}`.\n\n{}",
        stop.reason.as_str(),
        stop.message
    );
    if let Some(tool_name) = &stop.tool_name {
        text.push_str(&format!("\n\nÚltima ferramenta: `{tool_name}`."));
    }
    text.push_str("\n\nTente novamente com uma instrução menor ou troque para um modelo/provider com tool calling mais estável.");
    messages.push(ModelMessage::assistant(text.clone()));
    text
}

async fn collect_model_output(ctx: &TurnContext, request: ModelRequest) -> Result<ModelTurnOutput> {
    let provider = ctx.active_model_provider();
    let mut stream = provider.stream(request);
    let mut output = ModelTurnOutput {
        text: String::new(),
        thinking: String::new(),
        tool_calls: Vec::new(),
    };
    let mut think_tags = ThinkTagSplitter::default();
    let mut repetition_detector = crate::repetition::RepetitionDetector::default();

    while let Some(event) = stream.next().await {
        ensure_not_cancelled(ctx)?;
        match event? {
            ModelStreamEvent::TextDelta { text } => {
                if let Some(warning) = repetition_detector.feed_text(&text) {
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx.send(AgentEvent::RepetitionDetected {
                            kind: map_repetition_kind(&warning.kind),
                            message: warning.message,
                        });
                    }
                }
                emit_split_text(ctx, &mut output, think_tags.push(&text));
            }
            ModelStreamEvent::ThinkingDelta { text } => {
                if let Some(warning) = repetition_detector.feed_thinking(&text) {
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx.send(AgentEvent::RepetitionDetected {
                            kind: map_repetition_kind(&warning.kind),
                            message: warning.message,
                        });
                    }
                }
                output.thinking.push_str(&text);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ModelThinkingDelta { text });
                }
            }
            ModelStreamEvent::ToolCall(invocation) => {
                tracing::info!(
                    tool = %invocation.tool_name,
                    invocation_id = %invocation.id,
                    "turn requested tool call"
                );
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ToolRequested(invocation.clone()));
                }
                output.tool_calls.push(invocation);
            }
            ModelStreamEvent::Usage {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            } => {
                let in_tok = input_tokens.unwrap_or(0);
                let out_tok = output_tokens.unwrap_or(0);
                let cache_create = cache_creation_tokens.unwrap_or(0);
                let cache_read = cache_read_tokens.unwrap_or(0);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::UsageReported {
                        input_tokens: in_tok,
                        output_tokens: out_tok,
                        cache_creation_tokens: cache_create,
                        cache_read_tokens: cache_read,
                    });
                }
                let mut state = ctx.compact_state.lock().await;
                state.update_usage(in_tok);
            }
            ModelStreamEvent::Done => {
                emit_split_text(ctx, &mut output, think_tags.drain_pending());
                break;
            }
            ModelStreamEvent::Status { .. } => {}
        }
    }

    Ok(output)
}

fn emit_split_text(ctx: &TurnContext, output: &mut ModelTurnOutput, parts: Vec<SplitTextPart>) {
    for part in parts {
        match part {
            SplitTextPart::Text(text) => {
                output.text.push_str(&text);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ModelDelta { text });
                }
            }
            SplitTextPart::Thinking(text) => {
                output.thinking.push_str(&text);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ModelThinkingDelta { text });
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SplitTextPart {
    Text(String),
    Thinking(String),
}

#[derive(Default)]
struct ThinkTagSplitter {
    in_think: bool,
    pending: String,
}

impl ThinkTagSplitter {
    fn push(&mut self, content: &str) -> Vec<SplitTextPart> {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(content);
        self.split(&input, false)
    }

    fn drain_pending(&mut self) -> Vec<SplitTextPart> {
        let pending = std::mem::take(&mut self.pending);
        let tag = if self.in_think { "</think>" } else { "<think>" };
        if is_partial_tag_prefix(&pending, tag) {
            return Vec::new();
        }
        self.split(&pending, true)
    }

    fn split(&mut self, input: &str, final_chunk: bool) -> Vec<SplitTextPart> {
        let mut parts = Vec::new();
        let mut remaining = input;

        while !remaining.is_empty() {
            let tag = if self.in_think { "</think>" } else { "<think>" };
            if let Some(pos) = find_ascii_case_insensitive(remaining, tag) {
                self.push_segment(&mut parts, &remaining[..pos]);
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
            self.push_segment(&mut parts, &remaining[..emit_len]);
            self.pending.push_str(&remaining[emit_len..]);
            break;
        }

        parts
    }

    fn push_segment(&self, parts: &mut Vec<SplitTextPart>, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.in_think {
            parts.push(SplitTextPart::Thinking(text.to_string()));
        } else {
            parts.push(SplitTextPart::Text(text.to_string()));
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

async fn handle_tool_calls(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
    run_state: &mut AgentRunState,
    policy: HarnessPolicy,
    mut output: ModelTurnOutput,
) -> Option<String> {
    let tool_call_content = std::mem::take(&mut output.text);
    let tool_call_thinking =
        (!output.thinking.is_empty()).then(|| std::mem::take(&mut output.thinking));
    messages.push(ModelMessage::assistant_tool_calls_with_context(
        output.tool_calls.clone(),
        tool_call_content,
        tool_call_thinking,
    ));

    let mut executable_calls = Vec::new();
    let mut immediate_results = Vec::new();
    for invocation in output.tool_calls {
        match record_tool_call(run_state, policy, &invocation) {
            ToolLoopDecision::Continue => executable_calls.push(invocation),
            ToolLoopDecision::Stop(stop) => {
                let result = tool_error_result(&invocation, &stop.message);
                let observation = compact_tool_observation(&invocation, &result, policy);
                immediate_results.push((invocation, result, observation));
                for (invocation, _result, observation) in immediate_results {
                    messages.push(ModelMessage::tool_result(
                        invocation.id,
                        invocation.tool_name,
                        observation,
                    ));
                }
                let text = finalize_harness_stop(ctx, messages, stop);
                return Some(text);
            }
        }
    }

    let mut all_results = immediate_results;
    for chunk in executable_calls.chunks(policy.max_parallel_tool_calls.max(1)) {
        let tool_futures = chunk
            .iter()
            .cloned()
            .map(|invocation| execute_tool_call(ctx, policy, invocation));
        all_results.extend(futures_util::future::join_all(tool_futures).await);
    }

    for (invocation, result, observation) in all_results {
        let stop = match record_tool_result(run_state, policy, &invocation, &result) {
            ToolLoopDecision::Continue => None,
            ToolLoopDecision::Stop(stop) => Some(stop),
        };
        messages.push(ModelMessage::tool_result(
            invocation.id,
            invocation.tool_name,
            observation,
        ));
        if let Some(stop) = stop {
            let text = finalize_harness_stop(ctx, messages, stop);
            return Some(text);
        }
    }

    None
}

async fn execute_tool_call(
    ctx: &TurnContext,
    policy: HarnessPolicy,
    invocation: crate::tool::ToolInvocation,
) -> ToolExecutionResult {
    if ctx.cancellation_requested() {
        let result = tool_error_result(&invocation, "turn cancelled");
        let observation = compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation);
    }

    if let Err(invalid) = ctx.tool_executor.validate_arguments(&invocation) {
        let result = ctx.tool_executor.invalid_tool_result(&invocation, invalid);
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
        }
        let observation = compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation);
    }

    if invocation.tool_name == QUESTION_TOOL_NAME {
        let result = ask_user_question(ctx, &invocation).await;
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
        }
        let observation = compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation);
    }

    let result = match ctx.tool_executor.validate(&invocation) {
        SecurityDecision::Allow => ctx.tool_executor.invoke(invocation.clone()).await,
        SecurityDecision::NeedsApproval(risk) => {
            approve_and_invoke_tool(ctx, &invocation, risk).await
        }
        SecurityDecision::Deny(reason) => tool_error_result(&invocation, reason),
    };

    if ctx.cancellation_requested() {
        let result = tool_error_result(&invocation, "turn cancelled");
        let observation = compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation);
    }

    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
    }

    let observation = compact_tool_observation(&invocation, &result, policy);
    (invocation, result, observation)
}

async fn ask_user_question(
    ctx: &TurnContext,
    invocation: &crate::tool::ToolInvocation,
) -> crate::tool::ToolResult {
    let Some(ref tx) = ctx.event_tx else {
        return tool_error_result(invocation, "question requires an interactive client");
    };

    let request = match question_request_from_invocation(invocation) {
        Ok(request) => request,
        Err(message) => return tool_error_result(invocation, message),
    };

    let answer_rx = ctx.question_resolver.register(invocation.id.clone());
    let _ = tx.send(AgentEvent::QuestionRequested(request));

    let response = tokio::select! {
        response = answer_rx => response.ok(),
        _ = ctx.cancel_token.notified() => None,
    };

    match response {
        Some(crate::event::QuestionResponse::Answered { id, answers }) => {
            let response = crate::event::QuestionResponse::Answered {
                id,
                answers: answers.clone(),
            };
            let _ = tx.send(AgentEvent::QuestionResolved(response));
            crate::tool::ToolResult {
                invocation_id: invocation.id.clone(),
                ok: true,
                output: json!({
                    "schema_version": 1,
                    "answers": answers,
                    "answer": answers.join("\n"),
                }),
            }
        }
        Some(response @ crate::event::QuestionResponse::Dismissed { .. }) => {
            let _ = tx.send(AgentEvent::QuestionResolved(response));
            tool_error_result(invocation, "user dismissed question")
        }
        None => {
            let response = crate::event::QuestionResponse::Dismissed {
                id: invocation.id.clone(),
            };
            let _ = tx.send(AgentEvent::QuestionResolved(response));
            tool_error_result(invocation, "turn cancelled")
        }
    }
}

fn question_request_from_invocation(
    invocation: &crate::tool::ToolInvocation,
) -> std::result::Result<crate::event::QuestionRequest, String> {
    let question = invocation
        .input
        .get("question")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "question must include a non-empty `question` string".to_string())?
        .trim()
        .to_string();
    let options_value = invocation
        .input
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| "question must include an `options` array".to_string())?;
    let mut options = Vec::new();
    for option in options_value {
        if let Some(label) = option.as_str() {
            options.push(crate::event::QuestionOption {
                label: label.to_string(),
                description: None,
            });
            continue;
        }
        let Some(object) = option.as_object() else {
            return Err("question options must be strings or objects".to_string());
        };
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "question option objects need a non-empty `label`".to_string())?;
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        options.push(crate::event::QuestionOption {
            label: label.to_string(),
            description,
        });
    }
    if options.is_empty() {
        return Err("question must include at least one option".to_string());
    }
    Ok(crate::event::QuestionRequest {
        id: invocation.id.clone(),
        question,
        options,
        multiple: invocation
            .input
            .get("multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        allow_custom: invocation
            .input
            .get("custom")
            .or_else(|| invocation.input.get("allow_custom"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

async fn approve_and_invoke_tool(
    ctx: &TurnContext,
    invocation: &crate::tool::ToolInvocation,
    risk: crate::security::SecurityRisk,
) -> crate::tool::ToolResult {
    let Some(ref tx) = ctx.event_tx else {
        return tool_error_result(
            invocation,
            "approval required in headless mode; rerun in TUI",
        );
    };

    let approval_risk = match risk {
        crate::security::SecurityRisk::Write => crate::event::ApprovalRisk::Write,
        crate::security::SecurityRisk::Command => crate::event::ApprovalRisk::Command,
        crate::security::SecurityRisk::GuardedCommand => crate::event::ApprovalRisk::Guarded,
        crate::security::SecurityRisk::ExternalPlugin => crate::event::ApprovalRisk::ExternalPlugin,
    };
    let approve_rx = ctx.approval_resolver.register(invocation.id.clone());

    let _ = tx.send(AgentEvent::ApprovalRequested(
        crate::event::ApprovalRequest {
            id: invocation.id.clone(),
            summary: format!("Run tool `{}`", invocation.tool_name),
            risk: approval_risk,
        },
    ));

    let approved = tokio::select! {
        decision = approve_rx => decision.ok(),
        _ = ctx.cancel_token.notified() => None,
    };

    match approved {
        Some(decision) => {
            let is_approved = matches!(decision, crate::event::ApprovalDecision::Approved { .. });
            if is_approved {
                ctx.tool_executor.invoke(invocation.clone()).await
            } else {
                tool_error_result(invocation, "user denied tool execution")
            }
        }
        None => {
            let _ = tx.send(AgentEvent::ApprovalResolved(
                crate::event::ApprovalDecision::Denied {
                    id: invocation.id.clone(),
                },
            ));
            tool_error_result(invocation, "turn cancelled")
        }
    }
}

fn persist_final_model_output(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
    output: &ModelTurnOutput,
) {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::ModelOutput {
            text: output.text.clone(),
            thinking: (!output.thinking.is_empty()).then(|| output.thinking.clone()),
        });
    }

    if !output.text.trim().is_empty() || !output.thinking.is_empty() {
        messages.push(ModelMessage::assistant_with_thinking(
            output.text.clone(),
            (!output.thinking.is_empty()).then(|| output.thinking.clone()),
        ));
    }
}

fn map_repetition_kind(kind: &crate::repetition::RepetitionKind) -> RepetitionWarningKind {
    match kind {
        crate::repetition::RepetitionKind::CharRun { ch, count } => {
            RepetitionWarningKind::CharRun {
                ch: *ch,
                count: *count,
            }
        }
        crate::repetition::RepetitionKind::AlternatingPattern { pattern, cycles } => {
            RepetitionWarningKind::AlternatingPattern {
                pattern: pattern.clone(),
                cycles: *cycles,
            }
        }
    }
}

/// Synchronizes new messages in the session conversation history to SQLite.
pub fn sync_messages_to_history(ctx: &TurnContext, messages: &[ModelMessage]) -> Result<()> {
    let memory_config = ctx.active_config().memory;
    if !memory_config.enabled {
        return Ok(());
    }
    let manager = crate::memory::MemoryManager::new(ctx.project_dir.clone(), &memory_config)?;
    manager
        .history
        .record_session_start(&ctx.session_id, &ctx.project_dir.to_string_lossy())?;

    let conn = &manager.history;
    // Get count of existing messages in the DB for this session
    let existing_count = conn
        .get_event_count(&ctx.session_id, "message")
        .unwrap_or(0);

    // Slice messages to log only new ones
    if (existing_count as usize) < messages.len() {
        for msg in &messages[existing_count as usize..] {
            let role_str = match msg.role {
                crate::model::ModelRole::User => "user",
                crate::model::ModelRole::Assistant => "assistant",
                crate::model::ModelRole::Tool => "tool",
                crate::model::ModelRole::System => "system",
            };

            let tool_name = msg.tool_name.clone();
            let tool_input: Option<String> = None;
            let mut tool_output = None;

            if msg.role == crate::model::ModelRole::Tool {
                tool_output = Some(msg.content.clone());
            }

            conn.record_event(
                &ctx.session_id,
                "message",
                Some(role_str),
                Some(&msg.content),
                tool_name.as_deref(),
                tool_input.as_deref(),
                tool_output.as_deref(),
                None,
                None,
            )?;
        }
    }
    Ok(())
}

/// Evaluates memory system checkpoint and rebuild thresholds based on context utilization.
pub(crate) async fn evaluate_memory_triggers(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
) -> Result<bool> {
    let memory_config = ctx.active_config().memory;
    if !memory_config.enabled {
        return Ok(false);
    }

    let (percentage, _total_tokens) = {
        let state = ctx.compact_state.lock().await;
        (
            state.context_percentage(0) as f64 / 100.0,
            state.total_estimated_tokens(0),
        )
    };

    // 1. Checkpoint thresholds
    let mut thresholds_to_trigger = Vec::new();
    {
        let state = ctx.compact_state.lock().await;
        for &t in &memory_config.checkpoint_thresholds {
            if percentage >= t && !state.crossed_thresholds.contains(&t) {
                thresholds_to_trigger.push(t);
            }
        }
    }

    if !thresholds_to_trigger.is_empty() {
        let manager = crate::memory::MemoryManager::new(ctx.project_dir.clone(), &memory_config)?;
        manager
            .history
            .record_session_start(&ctx.session_id, &ctx.project_dir.to_string_lossy())?;

        let provider = ctx.active_model_provider();
        let model_name = ctx.active_model_name();

        crate::memory::run_checkpoint_writer(
            &ctx.session_id,
            messages,
            &manager.store,
            provider.as_ref(),
            &model_name,
        )
        .await?;

        // Mark thresholds as crossed
        {
            let mut state = ctx.compact_state.lock().await;
            for t in thresholds_to_trigger {
                state.crossed_thresholds.push(t);
                let cp_path = manager
                    .store
                    .checkpoint_path()
                    .to_string_lossy()
                    .to_string();
                manager.history.record_checkpoint(
                    &ctx.session_id,
                    state.crossed_thresholds.len() as i64,
                    percentage,
                    &cp_path,
                )?;
            }
        }
    }

    // 2. Rebuild threshold
    if percentage >= memory_config.rebuild_threshold {
        tracing::info!(
            "Rebuild threshold reached ({}% >= {}%)",
            percentage * 100.0,
            memory_config.rebuild_threshold * 100.0
        );
        let manager = crate::memory::MemoryManager::new(ctx.project_dir.clone(), &memory_config)?;

        let context_window = {
            let state = ctx.compact_state.lock().await;
            state.context_window
        };

        // Rebuild context!
        let boot_context = crate::memory::build_rebuild_context(
            messages,
            &manager.store,
            context_window,
            memory_config.injected_context_token_budget,
        );

        // Record rebuild in SQLite
        let cycle_num = {
            let mut state = ctx.compact_state.lock().await;
            state.crossed_thresholds.clear(); // reset thresholds for the new cycle!
            1 // Default cycle sequence number
        };
        manager
            .history
            .record_rebuild(&ctx.session_id, cycle_num, cycle_num + 1, &boot_context)?;

        // Re-assemble conversation messages
        messages.clear();
        messages.push(ModelMessage::system(boot_context));

        // Reset compaction state token usage
        {
            let mut state = ctx.compact_state.lock().await;
            state.last_input_tokens = None;
            state.clear_unsent_bytes();
        }

        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::Error {
                message: "Context limit approached. Initiated physical context rebuild cycle."
                    .to_string(),
            });
        }

        return Ok(true);
    }

    Ok(false)
}

#[cfg(test)]
mod tests;
