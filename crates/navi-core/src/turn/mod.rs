use crate::agent::AgentControl;
use crate::cancel::CancelToken;
use crate::compact::{self, CompactState};
use crate::context::{ContextPacket, render_context_packets};
use crate::event::AgentEvent;
use crate::harness::{
    AgentRunState, HarnessPolicy, ToolLoopDecision, build_system_prompt_with_tools,
    compact_tool_observation, record_tool_call, tool_error_result, trace_request_summary,
};
use crate::model::{
    ModelMessage, ModelProvider, ModelRequest, ModelRole, ModelStreamEvent, ThinkingConfig,
};
use crate::security::SecurityDecision;
use crate::skills::{SkillManifest, render_active_skills};
use crate::tool::ToolExecutor;
use anyhow::Result;
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

const TURN_LOOP_LIMIT: usize = 10;

struct ModelTurnOutput {
    text: String,
    thinking: String,
    tool_calls: Vec<crate::tool::ToolInvocation>,
}

type ToolExecutionResult = (crate::tool::ToolInvocation, crate::tool::ToolResult, String);

pub struct TurnContext {
    pub model_provider: Arc<dyn ModelProvider>,
    pub tool_executor: Arc<ToolExecutor>,
    pub agent_control: AgentControl,
    pub project_dir: PathBuf,
    pub model_name: String,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    pub approval_resolver: crate::runtime::ApprovalResolver,
    pub compact_state: Arc<tokio::sync::Mutex<CompactState>>,
    pub harness_config: crate::config::HarnessConfig,
    pub include_tool_prompt_manifest: bool,
    pub agent_mode: Option<crate::agent::AgentMode>,
    pub context_packets: Vec<ContextPacket>,
    pub active_skills: Vec<SkillManifest>,
    pub cancel_token: CancelToken,
}

impl TurnContext {
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

    let mut loop_count = 0;
    let mut run_state = AgentRunState::default();
    let final_text = loop {
        loop_count += 1;
        if loop_count > TURN_LOOP_LIMIT {
            break "Loop execution limit reached".to_string();
        }
        ensure_not_cancelled(ctx)?;
        maintain_context_budget(ctx, messages).await;
        ensure_not_cancelled(ctx)?;

        let request = build_model_request(ctx, messages);
        emit_request_trace(ctx, &request, policy);

        let output = collect_model_output(ctx, request).await?;
        ensure_not_cancelled(ctx)?;

        if !output.tool_calls.is_empty() {
            handle_tool_calls(ctx, messages, &mut run_state, policy, output).await;
            continue;
        }

        persist_final_model_output(ctx, messages, &output);
        break output.text;
    };

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
    let agents_md_path = ctx.project_dir.join("AGENTS.md");
    // Read AGENTS.md without blocking the async runtime.
    let base_instructions = if agents_md_path.exists() {
        let path = agents_md_path.clone();
        match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await {
            Ok(Ok(content)) => content,
            _ => "Default NAVI base instructions".to_string(),
        }
    } else {
        "Default NAVI base instructions".to_string()
    };

    let mut system_content = format!(
        "{}\n\n=== AGENTS.md / Project Instructions ===\n{}",
        build_system_prompt_with_tools(
            &crate::config::NaviConfig::default(),
            &ctx.project_dir,
            None,
            &ctx.tool_executor.definitions(),
            ctx.include_tool_prompt_manifest,
        ),
        base_instructions
    );
    if let Some(mode) = ctx.agent_mode {
        system_content.push_str("\n\n=== Agent Mode ===\n");
        system_content.push_str(mode.runtime_instructions());
    }
    if let Some(context) = render_context_packets(&ctx.context_packets) {
        system_content.push_str("\n\n");
        system_content.push_str(&context);
    }
    if let Some(skills) = render_active_skills(&ctx.active_skills) {
        system_content.push_str("\n\n");
        system_content.push_str(&skills);
    }

    if messages.is_empty() {
        messages.push(ModelMessage::system(system_content));
    } else if messages[0].role == ModelRole::System {
        messages[0].content = system_content;
    } else {
        messages.insert(0, ModelMessage::system(system_content));
    }
}

async fn maintain_context_budget(ctx: &TurnContext, messages: &mut Vec<ModelMessage>) {
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
    match state
        .auto_compact(
            messages,
            ctx.model_provider.as_ref(),
            &ctx.model_name,
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
    ModelRequest {
        model: ctx.model_name.clone(),
        messages: messages.to_vec(),
        thinking: ThinkingConfig::High,
        tools: ctx.tool_executor.definitions(),
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

async fn collect_model_output(ctx: &TurnContext, request: ModelRequest) -> Result<ModelTurnOutput> {
    let mut stream = ctx.model_provider.stream(request);
    let mut output = ModelTurnOutput {
        text: String::new(),
        thinking: String::new(),
        tool_calls: Vec::new(),
    };

    while let Some(event) = stream.next().await {
        ensure_not_cancelled(ctx)?;
        match event? {
            ModelStreamEvent::TextDelta { text } => {
                output.text.push_str(&text);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ModelDelta { text });
                }
            }
            ModelStreamEvent::ThinkingDelta { text } => {
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
            } => {
                let in_tok = input_tokens.unwrap_or(0);
                let out_tok = output_tokens.unwrap_or(0);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::UsageReported {
                        input_tokens: in_tok,
                        output_tokens: out_tok,
                    });
                }
                let mut state = ctx.compact_state.lock().await;
                state.update_usage(in_tok);
            }
            ModelStreamEvent::Done => break,
            ModelStreamEvent::Status { .. } => {}
        }
    }

    Ok(output)
}

async fn handle_tool_calls(
    ctx: &TurnContext,
    messages: &mut Vec<ModelMessage>,
    run_state: &mut AgentRunState,
    policy: HarnessPolicy,
    mut output: ModelTurnOutput,
) {
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
            ToolLoopDecision::RepeatedCall(reason) => {
                let result = tool_error_result(&invocation, reason);
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
                }
                let observation = compact_tool_observation(&invocation, &result, policy);
                immediate_results.push((invocation, result, observation));
            }
        }
    }

    let tool_futures = executable_calls
        .into_iter()
        .map(|invocation| execute_tool_call(ctx, policy, invocation));
    let mut executed_results = immediate_results;
    executed_results.extend(futures_util::future::join_all(tool_futures).await);

    for (invocation, _result, observation) in executed_results {
        messages.push(ModelMessage::tool_result(
            invocation.id,
            invocation.tool_name,
            observation,
        ));
    }
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
            let _ = tx.send(AgentEvent::ApprovalResolved(decision));
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

#[cfg(test)]
mod tests;
