use crate::state::SetupPhase;
use navi_sdk::{
    AgentEvent, ApprovalDecision, ApprovalRisk, BackgroundCommandSnapshot, LoadedConfig,
    ModelMessage, NaviUsageReport, available_model_options, canonical_provider_id,
    compact_tool_observation,
};

use crate::app::TuiApp;
use crate::chat::{
    drain_next_queued_message, ensure_tail_model_response, finalize_active_assistant,
    remove_active_tool_placeholder, update_active_assistant_status,
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
        paste_slot: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
    },
    OAuthCompleted {
        provider_id: String,
        result: std::result::Result<(), String>,
    },
    UsageLoaded {
        result: std::result::Result<NaviUsageReport, String>,
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
        AsyncEvent::UsageLoaded { result } => {
            app.usage_state.loading = false;
            match result {
                Ok(report) => {
                    app.usage_state.error = None;
                    app.usage_state.report = Some(report);
                }
                Err(error) => {
                    app.usage_state.report = None;
                    app.usage_state.error = Some(error.clone());
                    show_notification(app, "Usage", error);
                }
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
            paste_slot,
        } => {
            app.oauth_state = Some(OAuthUiState {
                provider_id,
                verification_uri: verification_uri.clone(),
                user_code,
                paste_slot,
                paste_status: None,
            });
            crate::keybindings::replace_modal(app, ModalKind::OAuth);
            // Open the authorize / device URL automatically (user can still
            // copy it from the OAuth modal with `c` if the browser fails).
            if !verification_uri.is_empty() {
                crate::browser::open_url(app, verification_uri);
            }
            show_notification(
                app,
                "OAuth",
                "Browser opened — finish login. If a code is shown, press p/Ctrl+V to paste it.",
            );
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
            crate::background::replace_background_commands(app, commands);
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
                if invocation.tool_name == "bash"
                    && result.output.get("background").and_then(|v| v.as_bool()) == Some(true)
                    && let Some(snapshot) = BackgroundCommandSnapshot::from_json(&result.output)
                {
                    crate::background::upsert_background_command(app, snapshot);
                }

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
                // Plan review modal is opened by PlanReviewRequested (blocks the
                // turn until the user resolves it) — not on ToolCompleted.
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
            // Refresh context meter every turn .
            app.compact_state
                .update_usage_full(input_tokens, output_tokens);
            app.usage_state.session_input_tokens =
                app.usage_state.session_input_tokens.saturating_add(input_tokens);
            app.usage_state.session_output_tokens = app
                .usage_state
                .session_output_tokens
                .saturating_add(output_tokens);
            app.usage_state.last_input_tokens = Some(input_tokens);
            app.usage_state.last_output_tokens = Some(output_tokens);
            app.usage_state.last_turn_label = app.compact_state.turn_usage_label();

            // Session spend: list rates → USD (cache-aware); credit providers also track credits.
            if let Some(cost) = estimate_turn_cost_usd(
                app,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            ) {
                app.usage_state.session_cost_usd += cost;
                app.usage_state.session_cost_known = true;
                apply_session_credit_fields(app);
            }

            // Annotate the active assistant bubble so usage is visible per turn.
            if let Some(msg) = app.messages.last_mut()
                && msg.role == ChatRole::Assistant
            {
                msg.usage_label = Some(format!(
                    "{} in · {} out",
                    format_tokens_k(input_tokens),
                    format_tokens_k(output_tokens),
                ));
            }
            // Force footer/chat redraw so the meter updates immediately.
            app.chat_render_cache.borrow_mut().signature_hash = 0;
            app.events.push(AgentEvent::UsageReported {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
            });
        }
        AgentEvent::SessionRecap {
            summary,
            suppressed,
        } => {
            if !suppressed && !summary.trim().is_empty() {
                // Upgrade in place when a later LLM recap arrives (same turn).
                if let Some(existing) = app.messages.iter_mut().rev().find(|m| m.is_recap) {
                    existing.content = summary.clone();
                    existing.status = Some("recap".to_string());
                } else {
                    app.messages.push(ChatMessage {
                        status: Some("recap".to_string()),
                        is_recap: true,
                        ..ChatMessage::new(ChatRole::Assistant, summary.clone())
                    });
                }
                app.chat_render_cache.borrow_mut().signature_hash = 0;
                app.scroll_offset = 0;
            }
            app.events.push(AgentEvent::SessionRecap {
                summary,
                suppressed,
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
                "Auto-compact: summarizing session context...".to_string(),
            );
            app.events.push(AgentEvent::AutoCompactStarted);
        }
        AgentEvent::AutoCompactCompleted { tokens_saved } => {
            show_notification(
                app,
                "Compact",
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
            submitted_at: _,
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
        AgentEvent::GoalUpdated {
            session_id: _,
            goal_id: _,
            objective,
            short_description,
            status,
            tokens_used,
            token_budget,
        } => {
            use navi_sdk::GoalStatus;
            app.goal_state = Some(crate::state::GoalUiState {
                objective: objective.clone(),
                short_description,
                tokens_used,
                token_budget,
            });
            if status == GoalStatus::Complete {
                show_notification(app, "Goal Completed", &objective);
            } else if status == GoalStatus::Blocked {
                show_notification(app, "Goal Blocked", &objective);
            }
        }
        AgentEvent::SetGoalRequested { .. } => {}
        AgentEvent::AutoDreamStarted {
            hours_since_last,
            sessions_reviewed,
        } => {
            app.dreaming = true;
            tracing::info!(
                "auto-dream started: {}h since last, {} sessions",
                hours_since_last,
                sessions_reviewed
            );
        }
        AgentEvent::AutoDreamCompleted {
            marked_stale,
            duplicates_merged,
            active_count,
        } => {
            app.dreaming = false;
            show_notification(
                app,
                "Dream Completed",
                &format!(
                    "{} stale, {} duplicates merged, {} active",
                    marked_stale, duplicates_merged, active_count
                ),
            );
        }
        AgentEvent::AutoDreamFailed { reason } => {
            app.dreaming = false;
            tracing::warn!("auto-dream failed: {}", reason);
        }
        AgentEvent::PlanProposed { title, steps } => {
            crate::plan_review::open_plan_review_from_proposed(
                app,
                title.clone(),
                steps.clone(),
            );
        }
        AgentEvent::PlanReviewRequested(request) => {
            crate::plan_review::open_plan_review_from_request(app, request.clone());
            app.events
                .push(AgentEvent::PlanReviewRequested(request));
        }
        AgentEvent::PlanReviewResolved(response) => {
            app.events
                .push(AgentEvent::PlanReviewResolved(response));
        }
        AgentEvent::SudoPasswordRequested(request) => {
            app.sudo_password_prompt = Some(crate::state::SudoPasswordUiState {
                request_id: request.id.clone(),
                command_summary: request.command_summary.clone(),
                password: String::new(),
                cursor: 0,
            });
            crate::keybindings::replace_modal(app, ModalKind::SudoPassword);
            app.events
                .push(AgentEvent::SudoPasswordRequested(request));
        }
        AgentEvent::AgentModeChanged { mode } => {
            app.agent_mode = mode;
            if mode.restricts_tools() {
                show_notification(
                    app,
                    "Plan Mode",
                    "Read-only tools only. Model will propose a plan.",
                );
            } else {
                show_notification(app, "Default Mode", "Full tool access restored.");
                app.proposed_plan = None;
                app.plan_review = None;
                if app.mode == Mode::ConfirmPlan {
                    crate::keybindings::close_active_modal(app);
                }
            }
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
    let completed_ok = res.is_ok();
    let mut recap_text: Option<String> = None;
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
            recap_text = Some(text);
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
    if completed_ok {
        if let Some(assistant) = recap_text {
            maybe_emit_session_recap(app, &assistant);
        }
        drain_next_queued_message(app);
    }
}

/// recap after a successful turn.
///
/// 1. Instant short local line (never dumps assistant prose).
/// 2. Background model upgrades it to a true one-sentence summary when possible.
fn maybe_emit_session_recap(app: &mut TuiApp, assistant_text: &str) {
    let user_prompt = app
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ChatRole::User)
        .map(|m| m.content.clone())
        .unwrap_or_default();

    let mut tool_names: Vec<String> = app
        .events
        .iter()
        .rev()
        .take(80)
        .filter_map(|e| match e {
            AgentEvent::ToolRequested(inv) => Some(inv.tool_name.clone()),
            _ => None,
        })
        .collect();
    tool_names.reverse();
    let tool_calls = tool_names.len();
    let suppressed =
        navi_core::should_suppress_recap(assistant_text.chars().count(), tool_calls);

    let local_summary =
        navi_core::local_recap_with_tools(&user_prompt, assistant_text, &tool_names);

    // Always emit local immediately (UI + tests); long-tail stays suppressed.
    handle_async_event(
        app,
        AsyncEvent::Agent(AgentEvent::SessionRecap {
            summary: local_summary,
            suppressed,
        }),
    );

    if suppressed {
        return;
    }

    let model_name = app
        .models
        .get(app.selected_model)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| app.loaded_config.config.model.name.clone());
    let loaded_config = app.loaded_config.clone();
    let project_dir = app.project_dir.clone();
    let assistant = assistant_text.to_string();
    let tx = app.async_sender();

    // Upgrade with a real model one-liner (plain voice). On failure, keep local.
    spawn_runtime_task(async move {
        let Ok(provider) =
            navi_sdk::build_provider_for_project_config(&loaded_config, &project_dir)
        else {
            return;
        };
        let Ok(text) = navi_core::llm_recap(
            provider.as_ref(),
            &model_name,
            &user_prompt,
            &assistant,
            &tool_names,
        )
        .await
        else {
            return;
        };
        if text.trim().is_empty() {
            return;
        }
        let _ = tx.send(AsyncEvent::Agent(AgentEvent::SessionRecap {
            summary: text,
            suppressed: false,
        }));
    });
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

fn format_tokens_k(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Estimate USD cost for a turn from registry list pricing (per 1M tokens).
///
/// Cache hits are billed at provider cache rates (Charm Hyper: cached input free)
/// so a 22k-token prefix with 99% cache does not look like a full-price charge.
fn estimate_turn_cost_usd(
    app: &TuiApp,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_create_tokens: u64,
) -> Option<f64> {
    let provider_id = app.loaded_config.config.model.provider.as_str();
    let model_name = app.loaded_config.config.model.name.as_str();
    let (in_rate, out_rate) =
        navi_sdk::model_list_pricing(&app.loaded_config.config, provider_id, model_name)?;
    let (cache_in, _cache_out) = navi_sdk::model_cache_list_pricing(provider_id).unwrap_or((0.0, 0.0));
    Some(navi_sdk::estimate_token_cost_usd_with_cache(
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_create_tokens,
        in_rate,
        out_rate,
        Some(cache_in),
        Some(in_rate), // cache write ≈ full input rate when unknown
    ))
}

fn apply_session_credit_fields(app: &mut TuiApp) {
    let provider_id = app.loaded_config.config.model.provider.as_str();
    if !navi_sdk::provider_uses_credits(provider_id) || !app.usage_state.session_cost_known {
        app.usage_state.session_credits_spent = None;
        app.usage_state.session_credit_unit = None;
        return;
    }
    let unit = navi_sdk::provider_credit_unit(provider_id).unwrap_or("credits");
    if let Some(credits) =
        navi_sdk::usd_to_provider_credits(provider_id, app.usage_state.session_cost_usd)
    {
        app.usage_state.session_credits_spent = Some(credits);
        app.usage_state.session_credit_unit = Some(unit.to_string());
    } else {
        app.usage_state.session_credits_spent = None;
        app.usage_state.session_credit_unit = Some(unit.to_string());
    }
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
        // 3000→1500: turn label keeps one decimal under 10k (1.5k).
        assert_eq!(
            app.usage_state.last_turn_label.as_deref(),
            Some("3k→1.5k")
        );
    }

    #[test]
    fn usage_reported_accumulates_session_cost_from_pricing() {
        let mut app = test_app("");
        // Inject list pricing on the selected model (API-key / non-OAuth path).
        let provider_id = app.loaded_config.config.model.provider.clone();
        let model_name = app.loaded_config.config.model.name.clone();
        if let Some(provider) = app
            .loaded_config
            .config
            .providers
            .iter_mut()
            .find(|p| p.id == provider_id)
        {
            if let Some(model) = provider.models.iter_mut().find(|m| m.name == model_name) {
                model.pricing_input_per_1m = Some(1.0); // $1 / 1M input
                model.pricing_output_per_1m = Some(2.0); // $2 / 1M output
            } else {
                provider.models.push(navi_sdk::ProviderModelConfig {
                    name: model_name.clone(),
                    task_size: None,
                    context_window_tokens: None,
                    max_output_tokens: None,
                    recommended_temperature: None,
                    supports_thinking: None,
                    supports_images: None,
                    supports_audio: None,
                    supports_video: None,
                    supports_documents: None,
                    tool_prompt_manifest: None,
                    pricing_input_per_1m: Some(1.0),
                    pricing_output_per_1m: Some(2.0),
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                });
            }
        } else {
            app.loaded_config.config.providers.push(navi_sdk::ProviderConfig {
                id: provider_id,
                models: vec![navi_sdk::ProviderModelConfig {
                    name: model_name,
                    task_size: None,
                    context_window_tokens: None,
                    max_output_tokens: None,
                    recommended_temperature: None,
                    supports_thinking: None,
                    supports_images: None,
                    supports_audio: None,
                    supports_video: None,
                    supports_documents: None,
                    tool_prompt_manifest: None,
                    pricing_input_per_1m: Some(1.0),
                    pricing_output_per_1m: Some(2.0),
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                }],
                ..Default::default()
            });
        }

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UsageReported {
                input_tokens: 1_000_000,
                output_tokens: 500_000,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
        );

        // $1.00 input + $1.00 output = $2.00
        assert!(app.usage_state.session_cost_known);
        assert!((app.usage_state.session_cost_usd - 2.0).abs() < 1e-9);
        assert_eq!(app.usage_state.session_input_tokens, 1_000_000);
        assert_eq!(app.usage_state.session_output_tokens, 500_000);
    }

    #[test]
    fn usage_reported_estimates_hypercredits_for_charm_hyper() {
        let mut app = test_app("");
        app.loaded_config.config.model.provider = "charm-hyper".into();
        app.loaded_config.config.model.name = "glm-5.2".into();
        // Ensure rates resolve (embedded snapshot fallback).
        let rates = navi_sdk::model_list_pricing(&app.loaded_config.config, "charm-hyper", "glm-5.2");
        assert!(
            rates.is_some(),
            "glm-5.2 should have list pricing in the registry snapshot"
        );

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UsageReported {
                input_tokens: 1_000_000,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            }),
        );

        assert!(app.usage_state.session_cost_known);
        // glm-5.2: $1.40 / 1M input → $1.40; 1 hypercredit = $0.05 → 28 credits
        assert!((app.usage_state.session_cost_usd - 1.4).abs() < 1e-9);
        let credits = app.usage_state.session_credits_spent.expect("hypercredits");
        assert!((credits - 28.0).abs() < 1e-6);
        assert_eq!(
            app.usage_state.session_credit_unit.as_deref(),
            Some("hypercredits")
        );
    }

    #[test]
    fn usage_reported_cache_hit_does_not_bill_full_hyper_input() {
        let mut app = test_app("");
        app.loaded_config.config.model.provider = "charm-hyper".into();
        app.loaded_config.config.model.name = "glm-5.2".into();

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::UsageReported {
                input_tokens: 22_000,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 21_780,
            }),
        );

        assert!(app.usage_state.session_cost_known);
        // Only 220 non-cached tokens at $1.40/1M ≈ $0.000308
        let expected = (220.0 / 1_000_000.0) * 1.4;
        assert!(
            (app.usage_state.session_cost_usd - expected).abs() < 1e-12,
            "got {} expected {}",
            app.usage_state.session_cost_usd,
            expected
        );
        // Full-price 22k would be ~$0.0308 — must not overbill.
        assert!(app.usage_state.session_cost_usd < 0.001);
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
        // Successful turns may append a session recap and jump scroll to the tail.
        assert_eq!(app.scroll_offset, 0);
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

    #[test]
    fn turn_completed_ok_drains_one_queued_message_after_cleanup() {
        let mut app = test_app("");
        app.provider_configured = false;
        app.is_loading = true;
        app.input = "queued follow-up".to_string();
        app.input_cursor = app.input.len();
        crate::chat::submit_message(&mut app);

        handle_async_event(
            &mut app,
            AsyncEvent::TurnCompleted(Ok("first done".to_string())),
        );

        assert!(app.queued_user_messages.is_empty());
        assert!(!app.is_loading);
        assert!(
            app.messages
                .iter()
                .any(|message| message.role == ChatRole::User
                    && message.content == "queued follow-up")
        );
        assert!(app.conversation_history.iter().any(|message| matches!(
            message.role,
            ModelRole::User
        ) && message.content
            == "queued follow-up"));
    }

    #[test]
    fn turn_completed_err_keeps_queued_message_pending() {
        let mut app = test_app("");
        app.is_loading = true;
        app.input = "do this next".to_string();
        app.input_cursor = app.input.len();
        crate::chat::submit_message(&mut app);

        handle_async_event(
            &mut app,
            AsyncEvent::TurnCompleted(Err("provider failed".to_string())),
        );

        assert_eq!(app.queued_user_messages.len(), 1);
        assert_eq!(app.queued_user_messages[0].text, "do this next");
        assert!(app.conversation_history.iter().all(|message| !(matches!(
            message.role,
            ModelRole::User
        ) && message.content
            == "do this next")));
    }

    #[test]
    fn tool_completed_does_not_drain_queued_message() {
        let mut app = test_app("");
        let invocation = sample_invocation("tool-1");
        app.is_loading = true;
        app.tool_invocations
            .insert(invocation.id.clone(), invocation.clone());
        app.running_tools.insert(invocation.id.clone(), invocation);
        app.input = "wait until full turn is done".to_string();
        app.input_cursor = app.input.len();
        crate::chat::submit_message(&mut app);

        handle_async_event(
            &mut app,
            AsyncEvent::Agent(AgentEvent::ToolCompleted(sample_result("tool-1", true))),
        );

        assert_eq!(app.queued_user_messages.len(), 1);
        assert_eq!(
            app.queued_user_messages[0].text,
            "wait until full turn is done"
        );
        assert!(app.conversation_history.iter().all(|message| !(matches!(
            message.role,
            ModelRole::User
        ) && message.content
            == "wait until full turn is done")));
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
        // Shared cleanup: tools/approvals cleared on both paths.
        // Scroll may differ: ok path can jump to tail after a session recap.
        assert_eq!(app_ok.running_tools.len(), app_err.running_tools.len());
        assert_eq!(
            app_ok.pending_approvals.len(),
            app_err.pending_approvals.len()
        );
        assert!(!app_ok.is_loading);
        assert!(app_ok.running_tools.is_empty());
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
            submitted_at: None,
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
