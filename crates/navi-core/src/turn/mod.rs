use crate::cancel::CancelToken;
use crate::compact::CompactState;
use crate::context::ContextPacket;
use crate::event::{AgentEvent, RepetitionWarningKind};
use crate::harness::{
    AgentRunState, HarnessPolicy, HarnessStop, HarnessStopReason, ToolLoopDecision,
    tool_error_result, trace_request_summary,
};
use crate::model::{
    AttachmentKind, ContentPart, ModelMessage, ModelProvider, ModelRequest, ModelRole,
    ModelStreamEvent, ThinkingConfig,
};
use crate::prompt::{PromptCache, SystemPromptInput};
use crate::runtime_components::RuntimeComponents;
use crate::security::SecurityDecision;
use crate::skills::SkillManifest;
use crate::tool::{ToolExecutor, ToolParallelism, take_tool_content_parts};
use anyhow::Result;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

const QUESTION_TOOL_NAME: &str = "question";
const PLAN_TOOL_NAME: &str = "plan";

struct ModelTurnOutput {
    text: String,
    thinking: String,
    tool_calls: Vec<crate::tool::ToolInvocation>,
    harness_stop: Option<HarnessStop>,
}

type ToolExecutionResult = (
    crate::tool::ToolInvocation,
    crate::tool::ToolResult,
    String,
    Vec<ContentPart>,
);

pub struct TurnContext {
    pub model_provider: Arc<RwLock<Arc<dyn ModelProvider>>>,
    pub tool_executor: Arc<ToolExecutor>,
    pub project_dir: PathBuf,
    pub data_dir: PathBuf,
    pub model_name: Arc<RwLock<String>>,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    pub approval_resolver: crate::runtime::ApprovalResolver,
    pub question_resolver: crate::runtime::QuestionResolver,
    pub plan_review_resolver: crate::runtime::PlanReviewResolver,
    pub sudo_password_resolver: crate::runtime::SudoPasswordResolver,
    pub compact_state: Arc<tokio::sync::Mutex<CompactState>>,
    pub harness_config: crate::config::HarnessConfig,
    pub include_tool_prompt_manifest: bool,
    pub context_packets: Arc<std::sync::Mutex<Vec<ContextPacket>>>,
    pub available_skills: Arc<std::sync::Mutex<Vec<SkillManifest>>>,
    pub active_skills: Arc<std::sync::Mutex<Vec<SkillManifest>>>,
    pub prompt_cache: Arc<PromptCache>,
    pub components: RuntimeComponents,
    pub cancel_token: CancelToken,
    /// Stable base instructions for the `instructions` field of the provider
    /// request, separated from developer messages for prompt cache efficiency.
    /// It is populated once, when the session prompt prefix is frozen.
    pub instructions: Arc<RwLock<Option<String>>>,
    /// Immutable prompt prefix for this session. Context packets, skills and
    /// memory must not rewrite the provider-visible system/developer prefix in
    /// the middle of a conversation: doing so fragments prompt caches and can
    /// subtly change the model's operating instructions.
    pub prompt_prefix: Arc<std::sync::Mutex<Option<Vec<ModelMessage>>>>,
    /// Snapshot of the active `NaviConfig` taken at turn start. Used by
    /// `ensure_system_prompt` so the model sees the user-configured harness
    /// profile, model and provider rather than the defaults.
    pub config: Arc<RwLock<crate::config::NaviConfig>>,
    /// Optional previous-session memory loaded at session startup.
    pub memory_injection: Option<String>,
    /// Optional separate provider for compaction/summarization. When set,
    /// auto_compact uses this instead of the main model provider.
    pub compaction_provider: Option<Arc<dyn ModelProvider>>,
    /// Current agent mode (Default or Plan). In Plan mode, only read-only
    /// tools are available to the model.
    pub agent_mode: crate::plan_mode::AgentMode,
    /// Model name for the compaction provider.
    pub compaction_model_name: Option<String>,
    pub session_id: String,
    /// Optional set of tool names the subagent is allowed to call.
    pub allowed_tool_names: Option<Vec<String>>,
    /// Session-scoped memory manager. Lazily opened once and reused for history
    /// sync, auto-memory index, checkpoints, and rebuilds (avoids reopening
    /// three SQLite DBs every tool-loop iteration).
    pub memory_manager: Arc<std::sync::Mutex<Option<Arc<crate::memory::MemoryManager>>>>,
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

    /// Returns a shared [`MemoryManager`] for this session when memory is enabled.
    /// Opens the underlying SQLite stores at most once per slot.
    pub fn get_or_init_memory_manager(&self) -> Result<Option<Arc<crate::memory::MemoryManager>>> {
        let memory_config = self.active_config().memory;
        if !memory_config.enabled {
            return Ok(None);
        }
        let mut guard = self
            .memory_manager
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(manager) = guard.as_ref() {
            return Ok(Some(manager.clone()));
        }
        let manager = Arc::new(crate::memory::MemoryManager::new(
            self.project_dir.clone(),
            self.data_dir.clone(),
            &memory_config,
        )?);
        *guard = Some(manager.clone());
        Ok(Some(manager))
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
    let final_text = loop {
        ensure_not_cancelled(ctx)?;
        maintain_context_budget(ctx, messages).await;
        ensure_not_cancelled(ctx)?;

        let request = build_model_request(ctx, messages);
        emit_request_trace(ctx, &request, policy);

        let output = collect_model_output(ctx, request).await?;
        ensure_not_cancelled(ctx)?;

        if let Some(stop) = output.harness_stop.clone() {
            let text = finalize_harness_stop(ctx, messages, stop);
            break text;
        }

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

    let _ = sync_messages_to_history(ctx, messages).await;
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
    if let Some(prefix) = ctx
        .prompt_prefix
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        replace_prompt_prefix(messages, prefix);
        return;
    }

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
    let available_skills = ctx
        .available_skills
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let memory_injection = combined_memory_injection(ctx).await;
    let mut tools = ctx.tool_executor.definitions();

    // In Plan mode, filter to read-only tools only.
    if ctx.agent_mode.restricts_tools() {
        tools.retain(|t| crate::plan_mode::is_tool_allowed_in_plan_mode(t.kind));
    }

    let input = SystemPromptInput {
        config: ctx.active_config(),
        project_dir: ctx.project_dir.clone(),
        memory_injection,
        tools: ctx
            .components
            .harness
            .filter_tools(tools, ctx.allowed_tool_names.as_deref()),
        include_tool_prompt_manifest: ctx.include_tool_prompt_manifest,
        context_packets,
        available_skills,
        active_skills,
    };
    let prompt = ctx.components.prompt.clone();
    let prompt_cache = ctx.prompt_cache.clone();
    let rendered = tokio::task::spawn_blocking(move || prompt.build(input, prompt_cache))
        .await
        .unwrap_or_else(|_| crate::prompt::RenderedPrompt {
            instructions: "Default NAVI base instructions".to_string(),
            developer_messages: Vec::new(),
        });

    // Store the stable base instructions on the turn context so
    // `build_model_request` can place them in the provider's
    // `instructions` field.
    *ctx.instructions.write().unwrap_or_else(|e| e.into_inner()) =
        Some(rendered.instructions.clone());

    // Build the prefix once. It deliberately remains unchanged for the rest
    // of the session so provider prompt-cache keys stay stable after the first
    // request, even if memory, skill or external-context state changes later.
    let mut prefix = Vec::with_capacity(2 + rendered.developer_messages.len());
    prefix.push(ModelMessage::system(rendered.instructions));
    // In Plan mode, inject a developer message instructing the model to
    // propose a plan via <proposed_plan> tags instead of executing.
    if ctx.agent_mode.restricts_tools() {
        prefix.push(ModelMessage::developer(
            "You are in Plan mode (host-restricted).\n\
             - Only read-only tools are available. Do NOT write files or run commands.\n\
             - Propose work with XML only (not the `plan` tool):\n\
             <proposed_plan title=\"Plan title\">\n\
             1. Step one\n\
             2. Step two\n\
             </proposed_plan>\n\
             - Do not call plan(action='create') in this mode; the host presents the proposal for review.\n\
             - After the user approves and leaves Plan mode, implement in normal mode."
                .to_string(),
        ));
    }
    prefix.extend(rendered.developer_messages);
    *ctx.prompt_prefix.lock().unwrap_or_else(|e| e.into_inner()) = Some(prefix.clone());
    replace_prompt_prefix(messages, prefix);
}

fn replace_prompt_prefix(messages: &mut Vec<ModelMessage>, prefix: Vec<ModelMessage>) {
    while matches!(
        messages.first(),
        Some(m) if m.role == ModelRole::System || m.role == ModelRole::Developer
    ) {
        messages.remove(0);
    }
    messages.splice(0..0, prefix);
}

async fn maintain_context_budget(ctx: &TurnContext, messages: &mut Vec<ModelMessage>) {
    let _ = sync_messages_to_history(ctx, messages).await;

    // Micro-compact and auto-compact run *before* long-horizon memory rebuild.
    // Previously rebuild short-circuited this path at ~85% usage, emitted a hard
    // Error event ("physical context rebuild"), and skipped model summarization —
    // so the turn never recovered and the compact text never appeared in chat.
    let cleared = ctx
        .components
        .compaction
        .micro_compact(messages, ctx.harness_config.micro_compact_gap_minutes);
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
    if should_autocompact {
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::AutoCompactStarted);
        }
        // Always use the session's own model — not a background/subagent provider.
        let provider = ctx.active_model_provider();
        let model = ctx.active_model_name();
        let mut state = ctx.compact_state.lock().await;
        match ctx
            .components
            .compaction
            .auto_compact(
                &mut state,
                messages,
                provider.as_ref(),
                &model,
                &ctx.harness_config,
            )
            .await
        {
            Ok(Some(outcome)) => {
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::AutoCompactCompleted {
                        tokens_saved: outcome.tokens_saved,
                        summary: outcome.summary,
                        kept_recent_messages: outcome.kept_recent_messages,
                    });
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

    // Checkpoints + rebuild fallback. After a successful auto-compact,
    // token usage is reset so rebuild will not fire. Rebuild only acts when
    // context is still critically full (e.g. auto-compact failed).
    let _ = evaluate_memory_triggers(ctx, messages).await;
}

fn build_model_request(ctx: &TurnContext, messages: &[ModelMessage]) -> ModelRequest {
    let config = ctx.active_config();
    // Fixed effort from config/session preference — never re-scored mid-turn so
    // provider prefix/KV cache stays stable across tool-loop iterations.
    let mut thinking = ThinkingConfig::from_config_str(&config.tui.thinking_level);

    // Clamp to registry reasoning_levels for the active model when available.
    // Models without reasoning support are forced to Off automatically.
    let model_name = ctx.active_model_name();
    let provider_id = config.model.provider.clone();
    if let Some(provider) = crate::config::resolve_provider_config(&config, &provider_id) {
        if let Some(model) = provider
            .models
            .iter()
            .find(|m| m.name == model_name || m.name.eq_ignore_ascii_case(&model_name))
        {
            thinking = crate::resolve_model_thinking_level(
                thinking,
                model.supports_thinking,
                &model.reasoning_levels,
                model.default_reasoning_effort.as_deref(),
            );
        }
    }

    ModelRequest {
        model: model_name,
        instructions: ctx
            .instructions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        messages: rewrite_unsupported_attachments(ctx, messages),
        thinking,
        tools: match crate::config::effective_tool_calling_mode(&config) {
            crate::config::ToolCallingMode::Native => {
                let all_tools = ctx.tool_executor.definitions();
                let mut tools = ctx
                    .components
                    .harness
                    .filter_tools(all_tools, ctx.allowed_tool_names.as_deref());
                // definitions() already returns name-sorted tools; re-sort only
                // when a filter may have disordered a filtered subset (no-op
                // when already sorted — keeps prefix-cache order stable).
                if tools.windows(2).any(|w| w[0].name > w[1].name) {
                    tools.sort_by(|a, b| a.name.cmp(&b.name));
                }
                tools
            }
            crate::config::ToolCallingMode::TextExtracted
            | crate::config::ToolCallingMode::ManifestOnly
            | crate::config::ToolCallingMode::Disabled => Vec::new(),
        },
        // Pin provider KV-cache affinity to this agent session (Charm Hyper, etc.).
        session_id: Some(ctx.session_id.clone()),
    }
}

fn rewrite_unsupported_attachments(
    ctx: &TurnContext,
    messages: &[ModelMessage],
) -> Vec<ModelMessage> {
    // Fast path: no multimodal attachments on user or tool messages.
    // ModelRequest still needs an owned list, so we clone once without mapping.
    let has_attachments = messages.iter().any(|m| {
        matches!(m.role, ModelRole::User | ModelRole::Tool) && !m.content_parts.is_empty()
    });
    if !has_attachments {
        return messages.to_vec();
    }

    let config = ctx.active_config();
    let provider_id = config.model.provider.clone();
    let model_name = config.model.name.clone();

    messages
        .iter()
        .cloned()
        .map(|mut message| {
            if !matches!(message.role, ModelRole::User | ModelRole::Tool)
                || message.content_parts.is_empty()
            {
                return message;
            }

            let mut rewritten = Vec::with_capacity(message.content_parts.len());
            for part in message.content_parts {
                let Some(kind) = part.attachment_kind() else {
                    rewritten.push(part);
                    continue;
                };

                if crate::config::model_supports_attachment(
                    &config,
                    &provider_id,
                    &model_name,
                    kind,
                ) {
                    rewritten.push(part);
                } else {
                    // Never dump base64 into the prompt for non-vision models.
                    rewritten.push(ContentPart::Text {
                        text: unsupported_attachment_tool_instruction(kind, &part),
                    });
                }
            }
            message.content_parts = rewritten;
            message
        })
        .collect()
}

fn unsupported_attachment_tool_instruction(kind: AttachmentKind, part: &ContentPart) -> String {
    let media_type = part.media_type().unwrap_or("application/octet-stream");
    // Never inline base64 attachment bytes into the prompt. Free/small models
    // hang or rate-limit when the rewritten text carries multi-MB payloads, and
    // models cannot reliably re-emit base64 into analyze_attachment anyway.
    let byte_len = part
        .data()
        .map(|data| approx_decoded_attachment_bytes(data))
        .unwrap_or(0);
    let name = part
        .name()
        .map(|n| format!(" name={n:?}"))
        .unwrap_or_default();
    format!(
        "[NAVI attachment unavailable to this chat model]\n\
         kind={kind} media_type={media_type} approx_bytes={byte_len}{name}\n\
         This model cannot view {kind} attachments directly. Tell the user to:\n\
         1) switch to a vision-capable model (registry supports_images), or\n\
         2) configure attachment_models.{kind} so analyze_attachment can run on a specialized model.\n\
         Do not invent the contents of the attachment.",
        kind = kind.as_str(),
    )
}

/// Base64 payload size estimate (decoded). Avoids allocating a decoded buffer.
fn approx_decoded_attachment_bytes(b64: &str) -> usize {
    let trimmed = b64.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let padding = trimmed.chars().rev().take_while(|c| *c == '=').count();
    trimmed.len().saturating_mul(3) / 4 - padding.min(2)
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
        "Stopped the run because the harness detected `{}`.\n\n{}",
        stop.reason.as_str(),
        stop.message
    );
    if let Some(tool_name) = &stop.tool_name {
        text.push_str(&format!("\n\nLast tool: `{tool_name}`."));
    }
    text.push_str(
        "\n\nTry again with a smaller instruction, or switch to a model/provider with more stable tool calling.",
    );
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
        harness_stop: None,
    };
    let mut think_tags = ThinkTagSplitter::default();
    let mut repetition_detector = crate::repetition::RepetitionDetector::default();

    // Race the provider stream against cancel. Checking only *after* each
    // `stream.next()` leaves the session loop parked on a hung/slow HTTP body
    // after Esc-cancel; the next user turn then queues forever and the TUI
    // shows "Waiting for model" until process restart.
    loop {
        let event = tokio::select! {
            biased;
            _ = ctx.cancel_token.notified() => {
                return Err(anyhow::anyhow!("turn cancelled"));
            }
            event = stream.next() => event,
        };

        let Some(event) = event else {
            break;
        };
        ensure_not_cancelled(ctx)?;
        match event? {
            ModelStreamEvent::TextDelta { text } => {
                let warning = repetition_detector.feed_text(&text);
                emit_split_text(ctx, &mut output, think_tags.push(&text));
                if let Some(warning) = warning {
                    output.harness_stop = Some(stop_for_repetition(ctx, warning));
                    break;
                }
            }
            ModelStreamEvent::ThinkingDelta { text } => {
                let warning = repetition_detector.feed_thinking(&text);
                output.thinking.push_str(&text);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ModelThinkingDelta { text });
                }
                if let Some(warning) = warning {
                    output.harness_stop = Some(stop_for_repetition(ctx, warning));
                    break;
                }
            }
            ModelStreamEvent::ToolCall(invocation) => {
                if invocation.tool_name.is_empty() {
                    tracing::warn!(
                        invocation_id = %invocation.id,
                        "skipping tool call with empty tool name from model"
                    );
                    continue;
                }
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
                let out_tok = output_tokens.unwrap_or(0);
                let cache_create = cache_creation_tokens.unwrap_or(0);
                let cache_read = cache_read_tokens.unwrap_or(0);
                // Context meter must count cached prompt tokens. Some aggregators
                // report only non-cached prompt (e.g. 430) with a large cache_read
                // (e.g. 63k) — without summing, the UI shows a bogus ~430 / 1M.
                let context_in = crate::compact::context_tokens_for_meter(
                    input_tokens,
                    cache_create,
                    cache_read,
                );
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::UsageReported {
                        // Prefer full context size for session accounting.
                        input_tokens: context_in.unwrap_or(input_tokens.unwrap_or(0)),
                        output_tokens: out_tok,
                        cache_creation_tokens: cache_create,
                        cache_read_tokens: cache_read,
                    });
                }
                if let Some(in_tok) = context_in {
                    // Never clobber a real prior reading with a zero/empty partial.
                    if in_tok > 0 {
                        let mut state = ctx.compact_state.lock().await;
                        state.update_usage_full(in_tok, out_tok);
                    }
                }
            }
            ModelStreamEvent::Done => {
                emit_split_text(ctx, &mut output, think_tags.drain_pending());
                break;
            }
            ModelStreamEvent::Status { label } => {
                if label == "resuming" {
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx.send(AgentEvent::StreamResuming {
                            accumulated_chars: output.text.len(),
                            attempt: 0,
                        });
                    }
                }
            }
        }
    }

    ensure_not_cancelled(ctx)?;
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
        ctx.components.hooks.on_tool_call(&invocation);
        match ctx
            .components
            .harness
            .record_tool_call(run_state, policy, &invocation)
        {
            ToolLoopDecision::Continue => executable_calls.push(invocation),
            ToolLoopDecision::Stop(stop) => {
                let result = tool_error_result(&invocation, &stop.message);
                let observation =
                    ctx.components
                        .harness
                        .compact_tool_observation(&invocation, &result, policy);
                immediate_results.push((invocation, result, observation, Vec::new()));
                for (invocation, _result, observation, content_parts) in immediate_results {
                    messages.push(ModelMessage::tool_result_with_parts(
                        invocation.id,
                        invocation.tool_name,
                        observation,
                        content_parts,
                    ));
                }
                let text = finalize_harness_stop(ctx, messages, stop);
                return Some(text);
            }
        }
    }

    let mut all_results = immediate_results;
    let execution_lock = Arc::new(tokio::sync::RwLock::new(()));
    for chunk in executable_calls.chunks(policy.max_parallel_tool_calls.max(1)) {
        let tool_futures = chunk.iter().cloned().map(|invocation| {
            execute_tool_call_with_parallelism(ctx, policy, invocation, execution_lock.clone())
        });
        all_results.extend(futures_util::future::join_all(tool_futures).await);
    }

    for (invocation, result, observation, content_parts) in all_results {
        ctx.components.hooks.on_tool_result(&result);
        let stop =
            match ctx
                .components
                .harness
                .record_tool_result(run_state, policy, &invocation, &result)
            {
                ToolLoopDecision::Continue => None,
                ToolLoopDecision::Stop(stop) => Some(stop),
            };
        messages.push(ModelMessage::tool_result_with_parts(
            invocation.id,
            invocation.tool_name,
            observation,
            content_parts,
        ));
        if let Some(summary) = manual_context_summary(&result) {
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactStarted);
            }
            let outcome = {
                let mut state = ctx.compact_state.lock().await;
                state.apply_manual_summary(messages, summary)
            };
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::AutoCompactCompleted {
                    tokens_saved: outcome.tokens_saved,
                    summary: outcome.summary,
                    kept_recent_messages: outcome.kept_recent_messages,
                });
            }
            return Some("Context compacted.".to_string());
        }
        if let Some(stop) = stop {
            let text = finalize_harness_stop(ctx, messages, stop);
            return Some(text);
        }
    }

    None
}

fn manual_context_summary(result: &crate::tool::ToolResult) -> Option<String> {
    if !result.ok || result.output.get("new_context_requested")?.as_bool()? != true {
        return None;
    }
    let summary = result.output.get("summary")?.as_str()?.trim();
    (!summary.is_empty()).then(|| summary.to_string())
}

async fn execute_tool_call_with_parallelism(
    ctx: &TurnContext,
    policy: HarnessPolicy,
    invocation: crate::tool::ToolInvocation,
    execution_lock: Arc<tokio::sync::RwLock<()>>,
) -> ToolExecutionResult {
    match ctx.tool_executor.parallelism_for(&invocation.tool_name) {
        ToolParallelism::Shared => {
            let _guard = execution_lock.read().await;
            execute_tool_call(ctx, policy, invocation).await
        }
        ToolParallelism::Exclusive => {
            let _guard = execution_lock.write().await;
            execute_tool_call(ctx, policy, invocation).await
        }
    }
}

async fn execute_tool_call(
    ctx: &TurnContext,
    policy: HarnessPolicy,
    invocation: crate::tool::ToolInvocation,
) -> ToolExecutionResult {
    if ctx.cancellation_requested() {
        let result = tool_error_result(&invocation, "turn cancelled");
        let observation =
            ctx.components
                .harness
                .compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation, Vec::new());
    }

    // Check allowed tool names for subagent tool filtering.
    if let Some(ref allowed) = ctx.allowed_tool_names {
        if !allowed.contains(&invocation.tool_name) {
            let result = tool_error_result(
                &invocation,
                format!(
                    "tool `{}` is not in the allowed tool set for this subagent",
                    invocation.tool_name
                ),
            );
            if let Some(ref tx) = ctx.event_tx {
                let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
            }
            let observation =
                ctx.components
                    .harness
                    .compact_tool_observation(&invocation, &result, policy);
            return (invocation, result, observation, Vec::new());
        }
    }

    if let Err(invalid) = ctx.tool_executor.validate_arguments(&invocation) {
        let result = ctx.tool_executor.invalid_tool_result(&invocation, invalid);
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
        }
        let observation =
            ctx.components
                .harness
                .compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation, Vec::new());
    }

    if invocation.tool_name == QUESTION_TOOL_NAME {
        let result = ask_user_question(ctx, &invocation).await;
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
        }
        let observation =
            ctx.components
                .harness
                .compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation, Vec::new());
    }

    let tool_ctx = crate::tool::ToolInvocationContext {
        event_tx: ctx.event_tx.clone(),
        sudo_password_resolver: Some(ctx.sudo_password_resolver.clone()),
        cancel_token: Some(ctx.cancel_token.clone()),
    };

    let mut result = match ctx.tool_executor.validate(&invocation) {
        SecurityDecision::Allow => {
            ctx.tool_executor
                .invoke_with_full_context(invocation.clone(), tool_ctx, false)
                .await
        }
        SecurityDecision::NeedsApproval(risk) => {
            approve_and_invoke_tool(ctx, &invocation, risk).await
        }
        SecurityDecision::Deny(reason) => tool_error_result(&invocation, reason),
    };

    // Plan create: block the turn until the user finishes the review modal.
    if result.ok
        && invocation.tool_name == PLAN_TOOL_NAME
        && result.output.get("needs_review").and_then(|v| v.as_bool()) == Some(true)
    {
        result = wait_for_plan_review(ctx, &invocation, result).await;
    }

    if ctx.cancellation_requested() {
        let result = tool_error_result(&invocation, "turn cancelled");
        let observation =
            ctx.components
                .harness
                .compact_tool_observation(&invocation, &result, policy);
        return (invocation, result, observation, Vec::new());
    }

    // Lift multimodal parts (e.g. view_image) before ToolCompleted/observation
    // so base64 never enters the text transcript or event payload.
    let content_parts = take_tool_content_parts(&mut result);

    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
    }

    let observation = ctx
        .components
        .harness
        .compact_tool_observation(&invocation, &result, policy);
    (invocation, result, observation, content_parts)
}

/// Block after `plan(create)` until the TUI resolves the review modal.
async fn wait_for_plan_review(
    ctx: &TurnContext,
    invocation: &crate::tool::ToolInvocation,
    created: crate::tool::ToolResult,
) -> crate::tool::ToolResult {
    let Some(ref tx) = ctx.event_tx else {
        // Headless: return create result without blocking.
        return created;
    };

    let plan_id = created
        .output
        .get("plan_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = created
        .output
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Plan")
        .to_string();
    let description = created
        .output
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let steps: Vec<String> = created
        .output
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    s.get("description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();

    let request = crate::event::PlanReviewRequest {
        id: invocation.id.clone(),
        plan_id: plan_id.clone(),
        title,
        description,
        steps,
    };

    let answer_rx = ctx.plan_review_resolver.register(invocation.id.clone());
    let _ = tx.send(AgentEvent::PlanReviewRequested(request));

    let response = tokio::select! {
        response = answer_rx => response.ok(),
        _ = ctx.cancel_token.notified() => None,
    };

    match response {
        Some(resp) => {
            let _ = tx.send(AgentEvent::PlanReviewResolved(resp.clone()));
            let decision = match resp.decision {
                crate::event::PlanReviewDecision::Approve => "approve",
                crate::event::PlanReviewDecision::RequestChanges => "request_changes",
                crate::event::PlanReviewDecision::Quit => "quit",
            };
            let comments_json: Vec<Value> = resp
                .comments
                .iter()
                .map(|c| {
                    json!({
                        "start_line": c.start_line,
                        "end_line": c.end_line,
                        "text": c.text,
                    })
                })
                .collect();
            let ok = !matches!(resp.decision, crate::event::PlanReviewDecision::Quit);
            let mut output = created.output;
            if let Some(obj) = output.as_object_mut() {
                obj.insert("decision".into(), json!(decision));
                obj.insert("comments".into(), json!(comments_json));
                obj.insert("freeform".into(), json!(resp.freeform));
                obj.insert(
                    "message".into(),
                    json!(match resp.decision {
                        crate::event::PlanReviewDecision::Approve =>
                            "User approved the plan. Proceed with implementation.",
                        crate::event::PlanReviewDecision::RequestChanges =>
                            "User requested changes to the plan. Revise based on comments.",
                        crate::event::PlanReviewDecision::Quit =>
                            "User abandoned the plan. Do not implement it.",
                    }),
                );
                // Still true that review finished; model should not open another modal.
                obj.insert("needs_review".into(), json!(false));
                obj.insert("review_complete".into(), json!(true));
            }
            crate::tool::ToolResult {
                invocation_id: invocation.id.clone(),
                ok,
                output,
            }
        }
        None => {
            let mut output = created.output;
            if let Some(obj) = output.as_object_mut() {
                obj.insert("decision".into(), json!("cancelled"));
                obj.insert("needs_review".into(), json!(false));
                obj.insert(
                    "message".into(),
                    json!("Plan review cancelled (turn cancelled)."),
                );
            }
            crate::tool::ToolResult {
                invocation_id: invocation.id.clone(),
                ok: false,
                output,
            }
        }
    }
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
        crate::security::SecurityRisk::Tool => crate::event::ApprovalRisk::Tool,
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
                ctx.tool_executor
                    .invoke_with_full_context(
                        invocation.clone(),
                        crate::tool::ToolInvocationContext {
                            event_tx: ctx.event_tx.clone(),
                            sudo_password_resolver: Some(ctx.sudo_password_resolver.clone()),
                            cancel_token: Some(ctx.cancel_token.clone()),
                        },
                        true,
                    )
                    .await
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

fn stop_for_repetition(
    ctx: &TurnContext,
    warning: crate::repetition::RepetitionWarning,
) -> HarnessStop {
    if let Some(ref tx) = ctx.event_tx {
        let _ = tx.send(AgentEvent::RepetitionDetected {
            kind: map_repetition_kind(&warning.kind),
            message: warning.message.clone(),
        });
    }
    HarnessStop {
        reason: HarnessStopReason::DegenerateModelOutput,
        message: warning.message,
        tool_name: None,
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
async fn combined_memory_injection(ctx: &TurnContext) -> Option<String> {
    let rebuild_context = {
        let state = ctx.compact_state.lock().await;
        state.rebuild_context.clone()
    };

    let auto_memory_index = load_auto_memory_index(ctx);

    let parts: Vec<String> = Vec::new();
    let mut parts = parts;

    if let Some(ref idx) = auto_memory_index {
        if !idx.trim().is_empty() {
            parts.push(format!("=== AUTO-MEMORY INDEX ===\n{}", idx));
        }
    }

    match (ctx.memory_injection.clone(), rebuild_context) {
        (Some(session_memory), Some(rebuild_context)) => {
            parts.push(session_memory);
            parts.push(format!("Rebuilt session context:\n\n{rebuild_context}"));
        }
        (Some(session_memory), None) => {
            parts.push(session_memory);
        }
        (None, Some(rebuild_context)) => {
            parts.push(format!("Rebuilt session context:\n\n{rebuild_context}"));
        }
        (None, None) => {}
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Loads the auto-memory index for system prompt injection.
fn load_auto_memory_index(ctx: &TurnContext) -> Option<String> {
    let manager = ctx.get_or_init_memory_manager().ok().flatten()?;
    let store = manager.auto_memory.clone();
    let index = store.build_prompt_context(2000);
    if index.trim().is_empty() {
        None
    } else {
        Some(index)
    }
}

pub async fn sync_messages_to_history(ctx: &TurnContext, messages: &[ModelMessage]) -> Result<()> {
    let Some(manager) = ctx.get_or_init_memory_manager()? else {
        return Ok(());
    };
    manager
        .history
        .record_session_start(&ctx.session_id, &ctx.project_dir.to_string_lossy())?;

    let pending: Vec<(u64, &ModelMessage)> = {
        let state = ctx.compact_state.lock().await;
        messages
            .iter()
            .filter_map(|msg| {
                let key = history_message_key(msg);
                (!state.history_synced_message_keys.contains(&key)).then_some((key, msg))
            })
            .collect()
    };

    let mut recorded_keys = Vec::new();
    for (key, msg) in pending {
        let role_str = match msg.role {
            crate::model::ModelRole::User => "user",
            crate::model::ModelRole::Assistant => "assistant",
            crate::model::ModelRole::Tool => "tool",
            crate::model::ModelRole::System => "system",
            crate::model::ModelRole::Developer => "developer",
        };

        let tool_name = msg.tool_name.clone();
        let tool_input: Option<String> = None;
        let mut tool_output = None;

        if msg.role == crate::model::ModelRole::Tool {
            tool_output = Some(msg.content.clone());
        }

        manager.history.record_event(
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
        recorded_keys.push(key);
    }

    if !recorded_keys.is_empty() {
        let mut state = ctx.compact_state.lock().await;
        state.history_synced_message_keys.extend(recorded_keys);
    }
    Ok(())
}

fn history_message_key(msg: &ModelMessage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::mem::discriminant(&msg.role).hash(&mut hasher);
    msg.content.hash(&mut hasher);
    msg.tool_call_id.hash(&mut hasher);
    msg.tool_name.hash(&mut hasher);
    msg.created_at.hash(&mut hasher);
    serde_json::to_string(&msg.content_parts)
        .unwrap_or_default()
        .hash(&mut hasher);
    serde_json::to_string(&msg.tool_calls)
        .unwrap_or_default()
        .hash(&mut hasher);
    msg.thinking_content.hash(&mut hasher);
    hasher.finish()
}

/// Evaluates memory system checkpoint and rebuild thresholds based on context utilization.
pub(crate) async fn evaluate_memory_triggers(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
) -> Result<bool> {
    let memory_config = ctx.active_config().memory;
    let Some(manager) = ctx.get_or_init_memory_manager()? else {
        return Ok(false);
    };

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
        manager
            .history
            .record_session_start(&ctx.session_id, &ctx.project_dir.to_string_lossy())?;

        let provider = ctx.active_model_provider();
        let model_name = ctx.active_model_name();

        crate::memory::run_checkpoint_writer(
            &ctx.session_id,
            messages,
            &manager.auto_memory,
            provider.as_ref(),
            &model_name,
        )
        .await?;

        // Mark thresholds as crossed
        {
            let mut state = ctx.compact_state.lock().await;
            for t in thresholds_to_trigger {
                state.crossed_thresholds.push(t);
                let cp_path = manager.auto_memory.db_path.to_string_lossy().to_string();
                manager.history.record_checkpoint(
                    &ctx.session_id,
                    state.crossed_thresholds.len() as i64,
                    percentage,
                    &cp_path,
                )?;
            }
        }
    }

    // 2. Rebuild threshold — last-resort fallback when auto-compact did not
    // reclaim enough budget (or the circuit breaker is open). Never emit a hard
    // Error: rebuild is recovery, not a turn failure, and the agent loop should
    // continue the active task with the rebuilt prompt.
    if percentage >= memory_config.rebuild_threshold {
        tracing::info!(
            "Rebuild threshold reached ({}% >= {}%) — applying long-horizon context rebuild fallback",
            percentage * 100.0,
            memory_config.rebuild_threshold * 100.0
        );

        let context_window = {
            let state = ctx.compact_state.lock().await;
            state.context_window
        };

        // Rebuild context!
        let boot_context = crate::memory::build_rebuild_context(
            messages,
            &manager.auto_memory,
            &manager.global_memory,
            context_window,
            memory_config.injected_context_token_budget,
        );

        // Record rebuild in SQLite
        let cycle_num = {
            let mut state = ctx.compact_state.lock().await;
            state.crossed_thresholds.clear(); // reset thresholds for the new cycle!
            state.rebuild_context = Some(boot_context.clone());
            1 // Default cycle sequence number
        };
        manager
            .history
            .record_rebuild(&ctx.session_id, cycle_num, cycle_num + 1, &boot_context)?;

        // Re-assemble conversation messages
        messages.clear();
        ensure_system_prompt(ctx, messages).await;

        // Reset compaction state token usage
        {
            let mut state = ctx.compact_state.lock().await;
            state.last_input_tokens = None;
            state.clear_unsent_bytes();
        }

        // Surface as a compact-style recovery notice so the TUI/chat show
        // continuity instead of a red error that looks like a hard failure.
        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::AutoCompactCompleted {
                tokens_saved: 1,
                summary: format!(
                    "Context was near the model limit. Session history was rebuilt from long-horizon memory.\n\n{}",
                    boot_context
                ),
                kept_recent_messages: 0,
            });
        }

        return Ok(true);
    }

    Ok(false)
}

#[cfg(test)]
mod tests;
