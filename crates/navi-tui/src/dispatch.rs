use navi_sdk::{
    AgentEvent, ApprovalDecision, LoadedConfig, ModelMessage, available_model_options,
    canonical_provider_id, compact_tool_observation,
};

use crate::app::TuiApp;
use crate::chat::{
    ensure_tail_model_response, finalize_active_assistant, remove_active_tool_placeholder,
    update_active_assistant_status,
};
use crate::errors::handle_model_error;
use crate::notifications::{push_diagnostic, show_notification};
use crate::providers::rebuild_provider;
use crate::runtime::spawn_runtime_task;
use crate::state::{ChatMessage, ChatRole};
use crate::stream::start_streaming_request;
use crate::tools::record_tool_requested;

// ─── async bridge ──────────────────────────────────────────────────────────────
pub(crate) enum AsyncEvent {
    SyncCompleted {
        loaded_config: LoadedConfig,
        message: String,
    },
    OAuthDeviceStarted {
        provider_id: String,
        verification_uri: String,
        user_code: String,
    },
    OAuthCompleted {
        provider_id: String,
        result: std::result::Result<(), String>,
    },
    Agent(AgentEvent),
    TurnCompleted(std::result::Result<String, String>),
    RetryModel,
}

pub(crate) fn handle_async_event(app: &mut TuiApp, event: AsyncEvent) {
    match event {
        AsyncEvent::Agent(agent_event) => match agent_event {
            AgentEvent::ModelDelta { text } => {
                let message = ensure_tail_model_response(app);
                message.content.push_str(&text);
                message.status = Some("receiving".to_string());
                app.scroll_offset = 0;
            }
            AgentEvent::ModelThinkingDelta { text } => {
                let message = ensure_tail_model_response(app);
                message.thinking_content.push_str(&text);
                message.status = Some("thinking".to_string());
                app.scroll_offset = 0;
            }
            AgentEvent::ToolRequested(invocation) => {
                record_tool_requested(app, invocation);
            }
            AgentEvent::ToolCompleted(result) => {
                app.running_tools.remove(&result.invocation_id);
                if let Some(invocation) = app.tool_invocations.get(&result.invocation_id).cloned() {
                    remove_active_tool_placeholder(app);
                    app.messages.push(ChatMessage {
                        status: Some("tool result".to_string()),
                        tool_invocation: Some(invocation.clone()),
                        tool_result: Some(result.clone()),
                        ..ChatMessage::new(ChatRole::Assistant, String::new())
                    });
                    let observation =
                        compact_tool_observation(&invocation, &result, app.harness_policy());
                    app.compact_state.add_unsent_bytes(observation.len());
                    app.conversation_history.push(ModelMessage::tool_result(
                        invocation.id.clone(),
                        invocation.tool_name.clone(),
                        observation,
                    ));
                }
                app.events.push(AgentEvent::ToolCompleted(result));
                update_active_assistant_status(app);
            }
            AgentEvent::ApprovalRequested(request) => {
                if app.yolo_mode {
                    let engine = app.engine();
                    let session_id = app.session_id.as_str().to_string();
                    let decision = ApprovalDecision::Approved {
                        id: request.id.clone(),
                    };
                    spawn_runtime_task(async move {
                        let _ = engine.resolve_approval(&session_id, decision).await;
                    });
                } else {
                    app.pending_approvals.push(request.clone());
                    app.events.push(AgentEvent::ApprovalRequested(request));
                    update_active_assistant_status(app);
                }
            }
            AgentEvent::ApprovalResolved(decision) => {
                let id = match &decision {
                    ApprovalDecision::Approved { id } => id,
                    ApprovalDecision::Denied { id } => id,
                };
                app.pending_approvals.retain(|r| &r.id != id);
                app.events.push(AgentEvent::ApprovalResolved(decision));
                update_active_assistant_status(app);
            }
            AgentEvent::Error { message } => {
                handle_model_error(app, message);
            }
            AgentEvent::HarnessTrace(value) => {
                app.events.push(AgentEvent::HarnessTrace(value));
            }
            AgentEvent::PatchProposed(patch) => {
                app.events.push(AgentEvent::PatchProposed(patch));
            }
            AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
            } => {
                app.compact_state.update_usage(input_tokens);
                if let Some(msg) = app.messages.last_mut() {
                    if msg.role == ChatRole::Assistant && msg.usage_label.is_none() {
                        msg.usage_label = Some(format!(
                            "{}k in · {}k out",
                            input_tokens / 1000,
                            output_tokens / 1000,
                        ));
                    }
                }
                app.events.push(AgentEvent::UsageReported {
                    input_tokens,
                    output_tokens,
                });
            }
            AgentEvent::MicroCompactApplied { messages_cleared } => {
                show_notification(
                    app,
                    "Micro-Compact",
                    format!(
                        "{} old tool results cleared (60+ min gap)",
                        messages_cleared
                    ),
                );
                app.events
                    .push(AgentEvent::MicroCompactApplied { messages_cleared });
            }
            AgentEvent::AutoCompactStarted => {
                push_diagnostic(
                    app,
                    "Auto-compact: context threshold reached, summarizing...".to_string(),
                );
                app.events.push(AgentEvent::AutoCompactStarted);
            }
            AgentEvent::AutoCompactCompleted { tokens_saved } => {
                show_notification(
                    app,
                    "Auto-Compact",
                    format!("Context compacted ({}k tokens saved)", tokens_saved / 1000),
                );
                app.compact_state.consecutive_failures = 0;
                if let Some(summary) = &app.compact_state.summary {
                    app.messages.push(ChatMessage {
                        status: Some("compacted".to_string()),
                        is_compact_summary: true,
                        content: format!(
                            "[Context compacted — {}k tokens saved]\n\n{}",
                            tokens_saved / 1000,
                            summary,
                        ),
                        ..ChatMessage::new(ChatRole::Assistant, String::new())
                    });
                }
                app.events
                    .push(AgentEvent::AutoCompactCompleted { tokens_saved });
            }
            AgentEvent::AutoCompactFailed { reason } => {
                push_diagnostic(app, format!("Auto-compact failed: {reason}"));
                app.compact_state.consecutive_failures =
                    app.compact_state.consecutive_failures.saturating_add(1);
                app.events.push(AgentEvent::AutoCompactFailed { reason });
            }
            AgentEvent::UserTaskSubmitted { text: _ } => {}
            AgentEvent::ModelOutput {
                text: _,
                thinking: _,
            } => {}
        },
        AsyncEvent::TurnCompleted(res) => {
            let elapsed_ms = app
                .loading_start
                .map(|start| start.elapsed().as_millis() as u64)
                .unwrap_or(0);
            match res {
                Ok(text) => {
                    finalize_active_assistant(app, elapsed_ms, &text);
                    app.is_loading = false;
                    app.loading_start = None;
                    app.clear_stream_task();
                    app.scroll_offset = 0;
                    app.running_tools.clear();
                    app.pending_approvals.clear();
                }
                Err(err) => {
                    app.is_loading = false;
                    app.loading_start = None;
                    app.clear_stream_task();
                    app.scroll_offset = 0;
                    app.running_tools.clear();
                    app.pending_approvals.clear();
                    handle_model_error(app, err);
                }
            }
        }
        AsyncEvent::RetryModel => {
            app.clear_stream_task();
            if app.is_loading {
                start_streaming_request(app);
            }
        }
        AsyncEvent::SyncCompleted {
            loaded_config,
            message,
        } => {
            app.loaded_config = loaded_config;
            app.models = available_model_options(&app.loaded_config.config);
            let selected_name = app.loaded_config.config.model.name.clone();
            let selected_provider = canonical_provider_id(&app.loaded_config.config.model.provider);
            app.selected_model = app
                .models
                .iter()
                .position(|model| {
                    model.name == selected_name
                        && canonical_provider_id(&model.provider_id) == selected_provider
                })
                .unwrap_or(0);
            rebuild_provider(app);
            app.messages.push(ChatMessage {
                status: Some("synced".to_string()),
                ..ChatMessage::new(ChatRole::Assistant, message)
            });
            app.is_loading = false;
            app.loading_start = None;
            app.clear_stream_task();
            app.scroll_offset = 0;
        }
        AsyncEvent::OAuthDeviceStarted {
            provider_id,
            verification_uri,
            user_code,
        } => {
            show_notification(
                app,
                "OAuth",
                format!("{provider_id}: open {verification_uri} and enter {user_code}"),
            );
            app.messages.push(ChatMessage {
                status: Some("oauth".to_string()),
                ..ChatMessage::new(
                    ChatRole::Assistant,
                    format!(
                        "OAuth started for {provider_id}.\nOpen {verification_uri}\nEnter code: {user_code}"
                    ),
                )
            });
        }
        AsyncEvent::OAuthCompleted {
            provider_id,
            result,
        } => {
            app.is_loading = false;
            app.loading_start = None;
            app.clear_stream_task();
            match result {
                Ok(()) => {
                    rebuild_provider(app);
                    show_notification(app, "OAuth", format!("{provider_id} connected."));
                }
                Err(err) => {
                    show_notification(app, "OAuth", format!("{provider_id} failed: {err}"));
                }
            }
        }
    }
}
