use crate::state::SetupPhase;
use navi_sdk::{
    AgentEvent, ApprovalDecision, ApprovalRisk, BackgroundCommandSnapshot, LoadedConfig,
    ModelMessage, available_model_options, canonical_provider_id, compact_tool_observation,
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
use crate::state::{
    ChatMessage, ChatRole, ModalKind, Mode, OAuthUiState, QuestionUiState, SubagentTranscript,
};
use crate::stream::start_streaming_request;
use crate::tools::record_tool_requested;

// ─── async bridge ──────────────────────────────────────────────────────────────
#[allow(clippy::large_enum_variant)]
pub enum AsyncEvent {
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
    AgentForSession {
        session_id: String,
        event: AgentEvent,
    },
    TurnCompleted(std::result::Result<String, String>),
    TurnCompletedForSession {
        session_id: String,
        result: std::result::Result<String, String>,
    },
    RetryModel,
    PluginCatalogLoaded {
        entries: Vec<navi_plugin_manifest::PluginCatalogEntry>,
        error: Option<String>,
    },
    PluginStaged {
        plugin_id: String,
        staging_path: std::path::PathBuf,
        update: bool,
        error: Option<String>,
    },
    PluginsReloaded {
        error: Option<String>,
        warnings: Vec<String>,
    },
    PluginsReloadNeeded,
    ClearSyncMessages,
    BackgroundCommandsUpdated(Vec<BackgroundCommandSnapshot>),
}

pub(crate) fn handle_async_event(app: &mut TuiApp, event: AsyncEvent) {
    match event {
        AsyncEvent::Agent(agent_event) => handle_agent_event(app, agent_event),
        AsyncEvent::AgentForSession { session_id, event } => {
            if session_id == app.session_id.as_str() {
                handle_agent_event(app, event);
            } else {
                tracing::debug!(
                    event_session = %session_id,
                    current_session = %app.session_id.as_str(),
                    "ignored stale agent event"
                );
            }
        }
        AsyncEvent::TurnCompleted(res) => handle_turn_completed(app, res),
        AsyncEvent::TurnCompletedForSession { session_id, result } => {
            if session_id == app.session_id.as_str() {
                handle_turn_completed(app, result);
            } else {
                tracing::debug!(
                    event_session = %session_id,
                    current_session = %app.session_id.as_str(),
                    "ignored stale turn completion"
                );
            }
        }
        AsyncEvent::RetryModel => {
            app.clear_stream_task();
            if app.is_loading {
                start_streaming_request(app);
            }
        }
        AsyncEvent::PluginCatalogLoaded { entries, error } => {
            app.plugin_catalog_loading = false;
            app.plugin_catalog = entries;
            app.plugin_catalog_error = error.unwrap_or_default();
            if !app.plugin_catalog_error.is_empty() {
                show_notification(app, "Plugins", app.plugin_catalog_error.clone());
            }
        }
        AsyncEvent::PluginStaged {
            plugin_id,
            staging_path,
            update,
            error,
        } => {
            if let Some(err) = error {
                show_notification(
                    app,
                    "Plugins",
                    format!("Failed to fetch {plugin_id}: {err}"),
                );
                return;
            }
            if let Err(err) =
                crate::plugins::handle_plugin_staged(app, &plugin_id, &staging_path, update)
            {
                show_notification(app, "Plugins", format!("{err:#}"));
            }
        }
        AsyncEvent::PluginsReloadNeeded => {
            crate::plugins::reload_engine_plugins(app);
        }
        AsyncEvent::PluginsReloaded { error, warnings } => {
            if let Some(err) = error {
                show_notification(app, "Plugins", format!("Reload failed: {err}"));
            } else if warnings.is_empty() {
                show_notification(app, "Plugins", "Plugins reloaded.");
            } else {
                show_notification(
                    app,
                    "Plugins",
                    format!("Reloaded with {} warning(s).", warnings.len()),
                );
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
                status: Some("syncing".to_string()),
                ..ChatMessage::new(ChatRole::Assistant, message)
            });
            app.is_loading = false;
            app.loading_start = None;
            app.clear_stream_task();
            app.scroll_offset = 0;

            // Auto-clear sync messages after 3 seconds
            let sender = app.async_sender();
            spawn_runtime_task(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let _ = sender.send(AsyncEvent::ClearSyncMessages);
            });
        }
        AsyncEvent::OAuthDeviceStarted {
            provider_id,
            verification_uri,
            user_code,
        } => {
            app.oauth_state = Some(OAuthUiState {
                provider_id,
                verification_uri,
                user_code,
            });
            crate::keybindings::replace_modal(app, ModalKind::OAuth);
            show_notification(app, "OAuth", "Open the login link to continue.");
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
                    crate::providers::push_recent_provider(app, &provider_id);
                    rebuild_provider(app);
                    app.oauth_state = None;
                    if app.mode == Mode::OAuth {
                        crate::keybindings::close_active_modal(app);
                    }
                    crate::providers::maybe_start_setup_interview(app);
                    show_notification(
                        app,
                        "OAuth",
                        format!(
                            "{provider_id} credentials saved. API access depends on provider plan."
                        ),
                    );
                }
                Err(err) => {
                    show_notification(app, "OAuth", format!("{provider_id} failed: {err}"));
                }
            }
        }
        AsyncEvent::ClearSyncMessages => {
            app.messages
                .retain(|m| !matches!(m.status.as_deref(), Some("syncing")));
            app.scroll_offset = 0;
        }
        AsyncEvent::BackgroundCommandsUpdated(commands) => {
            app.background_commands = commands;
        }
    }
}

fn handle_agent_event(app: &mut TuiApp, event: AgentEvent) {
    match event {
        AgentEvent::ModelDelta { text } => {
            let message = ensure_tail_model_response(app);
            message.content.push_str(&text);
            message.status = Some("receiving".to_string());
        }
        AgentEvent::ModelThinkingDelta { text } => {
            let message = ensure_tail_model_response(app);
            message.thinking_content.push_str(&text);
            message.status = Some("thinking".to_string());
        }
        AgentEvent::ToolRequested(invocation) => {
            if invocation.tool_name == "subagent" {
                if !app.subagent_order.iter().any(|id| id == &invocation.id) {
                    app.subagent_order.push(invocation.id.clone());
                }
                app.subagent_transcripts
                    .entry(invocation.id.clone())
                    .or_insert_with(|| SubagentTranscript::new(subagent_title(&invocation)));
            }
            record_tool_requested(app, invocation);
        }
        AgentEvent::ToolCompleted(result) => {
            app.running_tools.remove(&result.invocation_id);
            app.subagent_activity.remove(&result.invocation_id);
            if let Some(invocation) = app.tool_invocations.get(&result.invocation_id).cloned() {
                // Check if this is a background bash command that's still running
                let is_background_running = invocation.tool_name == "bash"
                    && result.output.get("background").and_then(|v| v.as_bool()) == Some(true)
                    && result.output.get("status").and_then(|v| v.as_str()) == Some("running");

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

                // Start background poller if this is a running background command
                if is_background_running {
                    crate::background::start_background_poller(app);
                }
            }
            app.events.push(AgentEvent::ToolCompleted(result));
            update_active_assistant_status(app);
        }
        AgentEvent::SubagentActivity {
            invocation_id,
            message,
        } => {
            if app.running_tools.contains_key(&invocation_id) {
                app.subagent_activity.insert(invocation_id, message);
            }
        }
        AgentEvent::SubagentTranscript {
            invocation_id,
            item,
        } => {
            app.subagent_transcripts
                .entry(invocation_id.clone())
                .or_insert_with(|| SubagentTranscript::new("Subagent".to_string()))
                .items
                .push(item);
            if !app.subagent_order.iter().any(|id| id == &invocation_id) {
                app.subagent_order.push(invocation_id);
            }
            app.chat_render_cache.borrow_mut().signature_hash = 0;
        }
        AgentEvent::ApprovalRequested(request) => {
            let is_guarded = matches!(request.risk, ApprovalRisk::Guarded);
            if app.yolo_mode && !is_guarded {
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
        AgentEvent::CapabilityRecorded(entry) => {
            app.events.push(AgentEvent::CapabilityRecorded(entry));
        }
        AgentEvent::QuestionRequested(request) => {
            if !app
                .pending_questions
                .iter()
                .any(|question| question.request.id == request.id)
            {
                app.pending_questions
                    .push(QuestionUiState::new(request.clone()));
            }
            app.events.push(AgentEvent::QuestionRequested(request));
            crate::keybindings::replace_modal(app, ModalKind::Question);
            update_active_assistant_status(app);
        }
        AgentEvent::QuestionResolved(response) => {
            let id = response.id().to_string();
            app.pending_questions
                .retain(|question| question.request.id != id);
            app.events.push(AgentEvent::QuestionResolved(response));
            if app.mode == Mode::Question && app.pending_questions.is_empty() {
                crate::keybindings::close_active_modal(app);
            }
            update_active_assistant_status(app);
        }
        AgentEvent::Error { message } => {
            handle_model_error(app, message);
        }
        AgentEvent::HarnessTrace(value) => {
            app.events.push(AgentEvent::HarnessTrace(value));
        }
        AgentEvent::HarnessStopped {
            reason,
            message,
            tool_name,
        } => {
            show_notification(app, "Harness stopped", &message);
            push_diagnostic(app, format!("Harness stopped ({reason}): {message}"));
            app.events.push(AgentEvent::HarnessStopped {
                reason,
                message,
                tool_name,
            });
        }
        AgentEvent::PatchProposed(patch) => {
            app.events.push(AgentEvent::PatchProposed(patch));
        }
        AgentEvent::UsageReported {
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        } => {
            app.compact_state.update_usage(input_tokens);
            if let Some(msg) = app.messages.last_mut()
                && msg.role == ChatRole::Assistant
                && msg.usage_label.is_none()
            {
                msg.usage_label = Some(format!(
                    "{}k in · {}k out",
                    input_tokens / 1000,
                    output_tokens / 1000,
                ));
            }
            app.events.push(AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
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
        AgentEvent::UserTaskSubmitted {
            text: _,
            content_parts: _,
        } => {}
        AgentEvent::ModelOutput {
            text: _,
            thinking: _,
        } => {}
        AgentEvent::RepeatedToolCallWarning { tool_name, message } => {
            show_notification(app, format!("Repeated call: {tool_name}"), &message);
            push_diagnostic(app, message);
        }
        AgentEvent::RepetitionDetected { kind, message } => {
            let title = match &kind {
                navi_sdk::RepetitionWarningKind::CharRun { .. } => "Character run",
                navi_sdk::RepetitionWarningKind::AlternatingPattern { .. } => "Alternating pattern",
            };
            show_notification(app, title.to_string(), &message);
            push_diagnostic(app, message);
        }
    }
}

fn subagent_title(invocation: &navi_sdk::ToolInvocation) -> String {
    invocation
        .input
        .get("description")
        .and_then(|value| value.as_str())
        .or_else(|| {
            invocation
                .input
                .get("prompt")
                .and_then(|value| value.as_str())
        })
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Subagent".to_string())
}

fn handle_turn_completed(app: &mut TuiApp, res: std::result::Result<String, String>) {
    let elapsed_ms = app
        .loading_start
        .map(|start| start.elapsed().as_millis() as u64)
        .unwrap_or(0);
    match res {
        Ok(text) => {
            finalize_active_assistant(app, elapsed_ms, &text);

            // Detect setup interview completion
            if let Some(SetupPhase::Interview) = app.setup_phase {
                let lower = text.to_lowercase();
                if lower.contains("all set")
                    || lower.contains("onboarding complete")
                    || lower.contains("setup complete")
                    || lower.contains("welcome to navi")
                {
                    handle_setup_interview_done(app);
                    return;
                }
            }
        }
        Err(err) => {
            handle_model_error(app, err);
        }
    }
    app.is_loading = false;
    app.loading_start = None;
    app.clear_stream_task();
    app.running_tools.clear();
    app.subagent_activity.clear();
    app.pending_approvals.clear();
    app.pending_questions.clear();
    if app.mode == Mode::Question {
        crate::keybindings::close_active_modal(app);
    }
}

/// React to setup interview completion — exit setup wizard, mark onboarding done.
fn handle_setup_interview_done(app: &mut TuiApp) {
    use crate::persistence::save_global_config_for_app;

    app.setup_phase = None;
    app.mode = Mode::Normal;
    let _ = save_global_config_for_app(app);
    app.conversation_history = vec![navi_sdk::ModelMessage::system(
        navi_core::build_system_prompt(&app.loaded_config.config, &app.project_dir),
    )];
    app.messages.clear();
    app.messages.push(ChatMessage::new(
        ChatRole::Assistant,
        "Setup complete! You can now start using NAVI normally.".to_string(),
    ));
    app.events.clear();
    app.reset_run_state();
    show_notification(app, "Setup", "Onboarding complete. Welcome to NAVI!");
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{ChatMessage, ChatRole};
    use crate::tests::test_app; // function exists in tests.rs
    use navi_sdk::{ApprovalRequest, ModelRole, ToolInvocation, ToolResult};
    use std::time::Instant;

    fn sample_invocation(id: &str) -> ToolInvocation {
        ToolInvocation {
            id: id.to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/main.rs" }),
        }
    }

    fn sample_result(id: &str, ok: bool) -> ToolResult {
        ToolResult {
            invocation_id: id.to_string(),
            ok,
            output: serde_json::json!({ "content": "fn main() {}" }),
        }
    }

    fn sample_approval(id: &str) -> ApprovalRequest {
        ApprovalRequest {
            id: id.to_string(),
            summary: format!("run tool {id}"),
            risk: navi_sdk::ApprovalRisk::Command,
        }
    }

    // ── ModelDelta ────────────────────────────────────────────────────

    #[test]
    fn model_delta_appends_content_and_preserves_manual_scroll() {
        let mut app = test_app("");
        app.scroll_offset = 42;

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ModelDelta {
                text: "hello ".to_string(),
            }),
        );
        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ModelDelta {
                text: "world".to_string(),
            }),
        );

        let msg = app.messages.last().unwrap();
        assert_eq!(msg.content, "hello world");
        assert_eq!(msg.status.as_deref(), Some("receiving"));
        assert_eq!(app.scroll_offset, 42);
    }

    #[test]
    fn model_delta_keeps_tail_when_already_at_bottom() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ModelDelta {
                text: "hello".to_string(),
            }),
        );

        assert_eq!(app.scroll_offset, 0);
    }

    // ── ModelThinkingDelta ────────────────────────────────────────────

    #[test]
    fn model_thinking_delta_appends_to_thinking_content() {
        let mut app = test_app("");
        app.scroll_offset = 10;

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ModelThinkingDelta {
                text: "reasoning...".to_string(),
            }),
        );

        let msg = app.messages.last().unwrap();
        assert_eq!(msg.thinking_content, "reasoning...");
        assert_eq!(msg.status.as_deref(), Some("thinking"));
        assert_eq!(app.scroll_offset, 10);
    }

    // ── ToolCompleted ─────────────────────────────────────────────────

    #[test]
    fn tool_completed_removes_from_running_and_adds_message() {
        let mut app = test_app("");
        let invocation = sample_invocation("call-1");
        app.running_tools
            .insert("call-1".to_string(), invocation.clone());
        app.tool_invocations
            .insert("call-1".to_string(), invocation);

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ToolCompleted(sample_result("call-1", true))),
        );

        assert!(!app.running_tools.contains_key("call-1"));
        let last = app.messages.last().unwrap();
        assert_eq!(last.tool_invocation.as_ref().unwrap().id, "call-1");
        assert!(last.tool_result.as_ref().unwrap().ok);
    }

    #[test]
    fn tool_completed_appends_to_conversation_history() {
        let mut app = test_app("");
        let invocation = sample_invocation("call-2");
        app.tool_invocations
            .insert("call-2".to_string(), invocation);
        let history_before = app.conversation_history.len();

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ToolCompleted(sample_result("call-2", true))),
        );

        assert_eq!(app.conversation_history.len(), history_before + 1);
        let last_msg = app.conversation_history.last().unwrap();
        assert_eq!(last_msg.role, ModelRole::Tool);
        assert_eq!(last_msg.tool_call_id.as_deref(), Some("call-2"));
    }

    #[test]
    fn tool_completed_without_invocation_is_noop_for_messages() {
        let mut app = test_app("");
        let messages_before = app.messages.len();

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ToolCompleted(sample_result("unknown", true))),
        );

        assert_eq!(app.messages.len(), messages_before);
    }

    // ── ApprovalRequested ─────────────────────────────────────────────

    #[test]
    fn approval_requested_adds_to_pending_in_normal_mode() {
        let mut app = test_app("");
        app.yolo_mode = false;

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ApprovalRequested(sample_approval("ap-1"))),
        );

        assert_eq!(app.pending_approvals.len(), 1);
        assert_eq!(app.pending_approvals[0].id, "ap-1");
    }

    #[test]
    fn approval_resolved_removes_from_pending() {
        let mut app = test_app("");
        app.pending_approvals.push(sample_approval("ap-2"));

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ApprovalResolved(ApprovalDecision::Approved {
                id: "ap-2".to_string(),
            })),
        );

        assert!(app.pending_approvals.is_empty());
    }

    #[test]
    fn approval_resolved_denied_removes_from_pending() {
        let mut app = test_app("");
        app.pending_approvals.push(sample_approval("ap-3"));

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ApprovalResolved(ApprovalDecision::Denied {
                id: "ap-3".to_string(),
            })),
        );

        assert!(app.pending_approvals.is_empty());
    }

    // ── UsageReported ─────────────────────────────────────────────────

    #[test]
    fn usage_reported_updates_compact_state() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UsageReported {
                input_tokens: 5000,
                output_tokens: 1000,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
        );

        assert!(app.compact_state.last_input_tokens.is_some());
    }

    #[test]
    fn usage_reported_sets_usage_label_on_assistant_message() {
        let mut app = test_app("");
        app.messages.push(ChatMessage::new(
            ChatRole::Assistant,
            "response".to_string(),
        ));

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UsageReported {
                input_tokens: 3000,
                output_tokens: 1500,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
        );

        let msg = app.messages.last().unwrap();
        assert_eq!(msg.usage_label.as_deref(), Some("3k in · 1k out"));
    }

    // ── AutoCompact ───────────────────────────────────────────────────

    #[test]
    fn auto_compact_completed_resets_failures_and_adds_summary() {
        let mut app = test_app("");
        app.compact_state.consecutive_failures = 3;
        app.compact_state.summary = Some("Previous context summary".to_string());

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::AutoCompactCompleted { tokens_saved: 5000 }),
        );

        assert_eq!(app.compact_state.consecutive_failures, 0);
        let last = app.messages.last().unwrap();
        assert!(last.is_compact_summary);
        assert!(last.content.contains("5k tokens saved"));
        assert!(last.content.contains("Previous context summary"));
    }

    #[test]
    fn auto_compact_failed_increments_failures() {
        let mut app = test_app("");
        app.compact_state.consecutive_failures = 1;

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::AutoCompactFailed {
                reason: "model error".to_string(),
            }),
        );

        assert_eq!(app.compact_state.consecutive_failures, 2);
    }

    #[test]
    fn auto_compact_failed_saturates_at_max() {
        let mut app = test_app("");
        app.compact_state.consecutive_failures = u32::MAX;

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::AutoCompactFailed {
                reason: "overflow".to_string(),
            }),
        );

        assert_eq!(app.compact_state.consecutive_failures, u32::MAX);
    }

    // ── TurnCompleted ─────────────────────────────────────────────────

    #[test]
    fn turn_completed_ok_clears_loading_state() {
        let mut app = test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.scroll_offset = 10;
        app.running_tools
            .insert("t1".to_string(), sample_invocation("t1"));
        app.pending_approvals.push(sample_approval("ap-1"));

        handle_async_event(
            &mut app,
            AsyncEvent::TurnCompleted(Ok("answer".to_string())),
        );

        assert!(!app.is_loading);
        assert!(app.loading_start.is_none());
        assert_eq!(app.scroll_offset, 10);
        assert!(app.running_tools.is_empty());
        assert!(app.pending_approvals.is_empty());
    }

    #[test]
    fn turn_completed_err_clears_loading_state() {
        let mut app = test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.scroll_offset = 5;
        app.running_tools
            .insert("t1".to_string(), sample_invocation("t1"));

        handle_async_event(
            &mut app,
            AsyncEvent::TurnCompleted(Err("rate limited".to_string())),
        );

        assert!(!app.is_loading);
        assert!(app.loading_start.is_none());
        assert_eq!(app.scroll_offset, 5);
        assert!(app.running_tools.is_empty());
        assert!(app.pending_approvals.is_empty());
    }

    #[test]
    fn turn_completed_err_pushes_error_message() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::TurnCompleted(Err("model not found".to_string())),
        );

        // The error is recorded in events regardless of retry scheduling
        let has_error_event = app.events.iter().any(
            |e| matches!(e, AgentEvent::Error { message } if message.contains("model not found")),
        );
        assert!(has_error_event, "should push an error event");
    }

    // ── Cleanup deduplication regression ──────────────────────────────
    //
    // Both Ok and Err paths must execute the same cleanup.
    // This is a regression test for the refactor that extracted
    // shared cleanup after the match.

    #[test]
    fn turn_completed_ok_and_err_share_cleanup_path() {
        let mut app_ok = test_app("");
        app_ok.is_loading = true;
        app_ok.loading_start = Some(Instant::now());
        app_ok.scroll_offset = 99;
        app_ok
            .running_tools
            .insert("x".to_string(), sample_invocation("x"));
        app_ok.pending_approvals.push(sample_approval("y"));

        let mut app_err = test_app("");
        app_err.is_loading = true;
        app_err.loading_start = Some(Instant::now());
        app_err.scroll_offset = 99;
        app_err
            .running_tools
            .insert("x".to_string(), sample_invocation("x"));
        app_err.pending_approvals.push(sample_approval("y"));

        handle_async_event(&mut app_ok, AsyncEvent::TurnCompleted(Ok("ok".to_string())));
        handle_async_event(
            &mut app_err,
            AsyncEvent::TurnCompleted(Err("err".to_string())),
        );

        assert_eq!(app_ok.is_loading, app_err.is_loading);
        assert_eq!(
            app_ok.loading_start.is_some(),
            app_err.loading_start.is_some()
        );
        assert_eq!(app_ok.scroll_offset, app_err.scroll_offset);
        assert_eq!(app_ok.running_tools.len(), app_err.running_tools.len());
        assert_eq!(
            app_ok.pending_approvals.len(),
            app_err.pending_approvals.len()
        );
    }

    // ── RetryModel ────────────────────────────────────────────────────

    #[test]
    fn retry_model_clears_stream_task() {
        let mut app = test_app("");
        app.clear_stream_task();

        handle_async_event(&mut app, AsyncEvent::RetryModel);

        assert!(!app.has_stream_task());
    }

    // ── SyncCompleted ─────────────────────────────────────────────────

    #[test]
    fn sync_completed_updates_config_and_clears_loading() {
        let mut app = test_app("");
        app.is_loading = true;
        app.loading_start = Some(Instant::now());
        app.scroll_offset = 5;

        handle_async_event(
            &mut app,
            AsyncEvent::SyncCompleted {
                loaded_config: navi_sdk::LoadedConfig {
                    config: navi_sdk::NaviConfig::default(),
                    global_config_path: None,
                    project_config_path: None,
                    data_dir: std::path::PathBuf::from("/tmp/navi-test"),
                },
                message: "Synced 3 providers".to_string(),
            },
        );

        assert!(!app.is_loading);
        assert!(app.loading_start.is_none());
        assert_eq!(app.scroll_offset, 0);
        let last = app.messages.last().unwrap();
        assert_eq!(last.status.as_deref(), Some("syncing"));
        assert_eq!(last.content, "Synced 3 providers");
    }

    // ── HarnessTrace and PatchProposed ────────────────────────────────

    #[test]
    fn harness_trace_pushed_to_events() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::HarnessTrace(serde_json::json!({
                "profile": "small",
                "tool_loop": 5
            }))),
        );

        assert_eq!(app.events.len(), 1);
    }

    #[test]
    fn patch_proposed_pushed_to_events() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::PatchProposed(navi_sdk::PatchProposal {
                id: "patch-1".to_string(),
                summary: "fix bug".to_string(),
                files: vec![std::path::PathBuf::from("src/main.rs")],
                unified_diff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new"
                    .to_string(),
            })),
        );

        assert_eq!(app.events.len(), 1);
    }

    // ── No-op variants don't panic ────────────────────────────────────

    #[test]
    fn user_task_submitted_is_noop() {
        let mut app = test_app("");
        let messages_before = app.messages.len();

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UserTaskSubmitted {
                text: "do something".to_string(),
                content_parts: vec![],
            }),
        );

        assert_eq!(app.messages.len(), messages_before);
    }

    #[test]
    fn model_output_is_noop() {
        let mut app = test_app("");
        let messages_before = app.messages.len();

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ModelOutput {
                text: "output".to_string(),
                thinking: Some("thinking".to_string()),
            }),
        );

        assert_eq!(app.messages.len(), messages_before);
    }

    // ── MicroCompactApplied ───────────────────────────────────────────

    #[test]
    fn micro_compact_applied_pushes_to_events() {
        let mut app = test_app("");

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::MicroCompactApplied {
                messages_cleared: 7,
            }),
        );

        assert_eq!(app.events.len(), 1);
    }

    // ── AutoCompactStarted ────────────────────────────────────────────

    #[test]
    fn auto_compact_started_pushes_diagnostic_and_event() {
        let mut app = test_app("");

        handle_async_event(&mut app, AsyncEvent::Agent(AgentEvent::AutoCompactStarted));

        assert_eq!(app.events.len(), 1);
        let diag = app.diagnostics();
        assert!(diag.iter().any(|d| d.contains("Auto-compact")));
    }
}
