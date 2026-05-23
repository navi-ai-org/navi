use crate::agent::AgentControl;
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
use crate::tool::ToolExecutor;
use anyhow::Result;
use futures_util::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;

pub struct TurnContext {
    pub model_provider: Arc<dyn ModelProvider>,
    pub tool_executor: Arc<ToolExecutor>,
    pub agent_control: AgentControl,
    pub project_dir: PathBuf,
    pub model_name: String,
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    pub pending_approvals: Arc<
        std::sync::Mutex<
            std::collections::HashMap<
                String,
                tokio::sync::oneshot::Sender<crate::event::ApprovalDecision>,
            >,
        >,
    >,
    pub compact_state: Arc<tokio::sync::Mutex<CompactState>>,
    pub harness_config: crate::config::HarnessConfig,
    pub include_tool_prompt_manifest: bool,
    pub agent_mode: Option<crate::agent::AgentMode>,
    pub context_packets: Vec<ContextPacket>,
}

impl TurnContext {
    pub fn resolve_approval(&self, decision: crate::event::ApprovalDecision) -> bool {
        let id = match &decision {
            crate::event::ApprovalDecision::Approved { id } => id,
            crate::event::ApprovalDecision::Denied { id } => id,
        };
        if let Some(tx) = self.pending_approvals.lock().unwrap().remove(id) {
            let _ = tx.send(decision);
            true
        } else {
            false
        }
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
    // 1. Resolve base instructions from AGENTS.md if it exists, otherwise fall back to build_system_prompt
    let agents_md_path = ctx.project_dir.join("AGENTS.md");
    let base_instructions = if agents_md_path.exists() {
        std::fs::read_to_string(&agents_md_path)
            .unwrap_or_else(|_| "Default NAVI base instructions".to_string())
    } else {
        "Default NAVI base instructions".to_string()
    };

    // Ensure first message is the combined system prompt if it exists, or insert it.
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

    if messages.is_empty() {
        messages.push(ModelMessage::system(system_content));
    } else if messages[0].role == ModelRole::System {
        messages[0].content = system_content;
    } else {
        messages.insert(0, ModelMessage::system(system_content));
    }

    // Keep loop execution bounded
    let mut loop_count = 0;
    let mut run_state = AgentRunState::default();
    let final_text = loop {
        loop_count += 1;
        if loop_count > 10 {
            break "Loop execution limit reached".to_string();
        }

        // Micro-compact: clear old tool results if gap exceeds threshold
        {
            let cleared =
                compact::micro_compact(messages, ctx.harness_config.micro_compact_gap_minutes);
            if cleared > 0 {
                tracing::info!(cleared, "micro-compact applied");
                if let Some(ref tx) = ctx.event_tx {
                    let _ = tx.send(AgentEvent::MicroCompactApplied {
                        messages_cleared: cleared,
                    });
                }
            }
        }

        // Auto-compact: summarize if context threshold reached
        {
            let should = {
                let state = ctx.compact_state.lock().await;
                state.should_autocompact(ctx.harness_config.autocompact_buffer_tokens)
            };
            if should {
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
        }

        // Prompt Construction
        let request = ModelRequest {
            model: ctx.model_name.clone(),
            messages: messages.clone(),
            thinking: ThinkingConfig::High,
            tools: ctx.tool_executor.definitions(),
        };

        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::HarnessTrace(trace_request_summary(
                &request, policy,
            )));
        }

        tracing::info!(
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.len(),
            "turn request started"
        );

        let mut stream = ctx.model_provider.stream(request);
        let mut text = String::new();
        let mut thinking = String::new();
        let mut tool_calls = Vec::new();

        while let Some(event) = stream.next().await {
            match event? {
                ModelStreamEvent::TextDelta { text: delta } => {
                    text.push_str(&delta);
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx.send(AgentEvent::ModelDelta { text: delta });
                    }
                }
                ModelStreamEvent::ThinkingDelta { text: delta } => {
                    thinking.push_str(&delta);
                    if let Some(ref tx) = ctx.event_tx {
                        let _ = tx.send(AgentEvent::ModelThinkingDelta { text: delta });
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
                    tool_calls.push(invocation);
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
                    {
                        let mut state = ctx.compact_state.lock().await;
                        state.update_usage(in_tok);
                    }
                }
                ModelStreamEvent::Done => break,
                ModelStreamEvent::Status { .. } => {}
            }
        }

        if !tool_calls.is_empty() {
            // Append assistant response if any
            if !text.trim().is_empty() {
                messages.push(ModelMessage::assistant(text.clone()));
            }

            // Record tool calls in message history
            for invocation in &tool_calls {
                messages.push(ModelMessage::assistant_tool_call(invocation.clone()));
            }

            let mut executable_calls = Vec::new();
            let mut immediate_results = Vec::new();
            for invocation in tool_calls {
                match record_tool_call(&mut run_state, policy, &invocation) {
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

            // Parallel tool execution pipeline with pre/post-execution hooks
            let tool_futures = executable_calls.into_iter().map(|invocation| {
                let tool_executor = ctx.tool_executor.clone();
                let event_tx = ctx.event_tx.clone();
                let pending_approvals = ctx.pending_approvals.clone();
                async move {
                    if let Err(invalid) = tool_executor.validate_arguments(&invocation) {
                        let result = tool_executor.invalid_tool_result(&invocation, invalid);
                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
                        }
                        let observation = compact_tool_observation(&invocation, &result, policy);
                        return (invocation, result, observation);
                    }

                    // Pre-execution hook: Security & Approvals
                    let decision = tool_executor.validate(&invocation);
                    let result = match decision {
                        SecurityDecision::Allow => {
                            // Execute tool invocation
                            tool_executor.invoke(invocation.clone()).await
                        }
                        SecurityDecision::NeedsApproval(risk) => {
                            let approval_risk = match risk {
                                crate::security::SecurityRisk::Write => {
                                    crate::event::ApprovalRisk::Write
                                }
                                crate::security::SecurityRisk::Command => {
                                    crate::event::ApprovalRisk::Command
                                }
                                crate::security::SecurityRisk::ExternalPlugin => {
                                    crate::event::ApprovalRisk::ExternalPlugin
                                }
                            };

                            if let Some(ref tx) = event_tx {
                                let (approve_tx, approve_rx) = tokio::sync::oneshot::channel();
                                {
                                    let mut lock = pending_approvals.lock().unwrap();
                                    lock.insert(invocation.id.clone(), approve_tx);
                                }

                                let _ = tx.send(AgentEvent::ApprovalRequested(
                                    crate::event::ApprovalRequest {
                                        id: invocation.id.clone(),
                                        summary: format!("Run tool `{}`", invocation.tool_name),
                                        risk: approval_risk,
                                    },
                                ));

                                match approve_rx.await {
                                    Ok(decision) => {
                                        let is_approved = matches!(
                                            decision,
                                            crate::event::ApprovalDecision::Approved { .. }
                                        );
                                        let _ = tx.send(AgentEvent::ApprovalResolved(decision));
                                        if is_approved {
                                            tool_executor.invoke(invocation.clone()).await
                                        } else {
                                            tool_error_result(
                                                &invocation,
                                                "user denied tool execution",
                                            )
                                        }
                                    }
                                    _ => {
                                        let _ = tx.send(AgentEvent::ApprovalResolved(
                                            crate::event::ApprovalDecision::Denied {
                                                id: invocation.id.clone(),
                                            },
                                        ));
                                        tool_error_result(&invocation, "user denied tool execution")
                                    }
                                }
                            } else {
                                tool_error_result(
                                    &invocation,
                                    "approval required in headless mode; rerun in TUI",
                                )
                            }
                        }
                        SecurityDecision::Deny(reason) => tool_error_result(&invocation, reason),
                    };

                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(AgentEvent::ToolCompleted(result.clone()));
                    }

                    // Post-execution hook: compaction and tracking
                    let observation = compact_tool_observation(&invocation, &result, policy);
                    (invocation, result, observation)
                }
            });

            let mut executed_results = immediate_results;
            executed_results.extend(futures_util::future::join_all(tool_futures).await);

            // Feed results back to history
            for (invocation, _result, observation) in executed_results {
                messages.push(ModelMessage::tool_result(
                    invocation.id,
                    invocation.tool_name,
                    observation,
                ));
            }

            // Loop back to prompt the model with tool results
            continue;
        }

        if let Some(ref tx) = ctx.event_tx {
            let _ = tx.send(AgentEvent::ModelOutput {
                text: text.clone(),
                thinking: (!thinking.is_empty()).then(|| thinking.clone()),
            });
        }
        break text;
    };

    Ok(final_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelResponse;
    use crate::tool::{Tool, ToolDefinition, ToolKind};
    use crate::{ModelStream, SecurityConfig, SecurityPolicy, ToolInvocation, ToolResult};
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockTool;
    #[async_trait]
    impl Tool for MockTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "test_tool".to_string(),
                description: "mock tool".to_string(),
                kind: ToolKind::Custom,
                input_schema: json!({}),
            }
        }
        async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
            Ok(ToolResult {
                invocation_id: invocation.id,
                ok: true,
                output: json!({ "result": "mock ok" }),
            })
        }
    }

    struct MockProvider {
        calls: Mutex<usize>,
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        fn stream(&self, _request: ModelRequest) -> ModelStream {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            let call_count = *calls;
            if call_count == 1 {
                Box::pin(stream::iter(vec![
                    Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "call-1".to_string(),
                        tool_name: "test_tool".to_string(),
                        input: json!({}),
                    })),
                    Ok(ModelStreamEvent::ToolCall(ToolInvocation {
                        id: "call-2".to_string(),
                        tool_name: "test_tool".to_string(),
                        input: json!({}),
                    })),
                    Ok(ModelStreamEvent::Done),
                ]))
            } else {
                Box::pin(stream::iter(vec![
                    Ok(ModelStreamEvent::TextDelta {
                        text: "done".to_string(),
                    }),
                    Ok(ModelStreamEvent::Done),
                ]))
            }
        }

        async fn complete(&self, request: ModelRequest) -> Result<ModelResponse> {
            ModelProvider::complete(self, request).await
        }
    }

    #[tokio::test]
    async fn test_turn_loop_with_parallel_tools() {
        let tempdir = tempfile::tempdir().unwrap();
        let security_policy = SecurityPolicy::new(
            tempdir.path().to_path_buf(),
            tempdir.path().to_path_buf(),
            SecurityConfig::default(),
        )
        .unwrap();
        let mut executor = ToolExecutor::new(security_policy);
        executor.register_tool(Arc::new(MockTool));

        let ctx = TurnContext {
            model_provider: Arc::new(MockProvider {
                calls: Mutex::new(0),
            }),
            tool_executor: Arc::new(executor),
            agent_control: AgentControl::new(),
            project_dir: tempdir.path().to_path_buf(),
            model_name: "gpt-4".to_string(),
            event_tx: None,
            pending_approvals: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            compact_state: Arc::new(tokio::sync::Mutex::new(CompactState::new(128_000))),
            harness_config: crate::config::HarnessConfig::default(),
            include_tool_prompt_manifest: false,
            agent_mode: None,
            context_packets: Vec::new(),
        };

        let mut messages = vec![];
        let policy = HarnessPolicy {
            profile: crate::config::HarnessProfile::Small,
            observation_max_bytes: 1000,
        };

        let result = run_turn(&ctx, &mut messages, policy).await.unwrap();
        assert_eq!(result, "done");
        let tool_results: Vec<_> = messages
            .iter()
            .filter(|m| m.role == ModelRole::Tool)
            .collect();
        assert_eq!(tool_results.len(), 2);
    }
}
