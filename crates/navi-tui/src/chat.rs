use navi_core::{AttachmentKind, model_supports_attachment};
use navi_sdk::{
    AgentEvent, CompactState, ContentPart, ModelMessage, ModelRole, SessionStore,
    build_system_prompt, effective_context_window,
};

use crate::TuiApp;
use crate::notifications::show_notification;
use crate::providers::selected_provider_label;
use crate::state::{ChatImage, ChatMessage, ChatRole, QueuedUserMessage};
use crate::stream::start_streaming_request;
use crate::tools::cancel_stream;

#[derive(Debug, Clone)]
struct ChatPrefix {
    messages: Vec<ChatMessage>,
    conversation_history: Vec<ModelMessage>,
    events: Vec<AgentEvent>,
}

pub(crate) fn submit_message(app: &mut TuiApp) {
    // Expand paste chips into full text before send / queue.
    crate::paste_compact::expand_composer_pastes(app);

    let has_images = !app.pending_images.is_empty();
    if app.input.trim().is_empty() && !has_images {
        return;
    }

    if app.is_loading {
        queue_current_message(app);
        return;
    }

    submit_current_message_now(app);
}

/// Set a thread goal from the Set Goal modal: store on the runtime, show a
/// Goal-labeled user bubble in chat, and start a turn so the model sees it.
pub(crate) fn submit_goal_objective(app: &mut TuiApp, objective: String) {
    let objective = objective.trim().to_string();
    if objective.is_empty() {
        return;
    }

    app.goal_state = Some(crate::state::GoalUiState);

    let session_id = app.session_id.as_str().to_string();
    let engine = app.engine();
    let obj = objective.clone();
    crate::runtime::spawn_runtime_task(async move {
        let _ = engine
            .start_session(navi_sdk::NaviSessionRequest {
                session_id: Some(session_id.clone()),
                project_dir: None,
                ..Default::default()
            })
            .await;
        if let Err(err) = engine.set_goal(&session_id, obj, None).await {
            tracing::warn!(error = %err, "set_goal failed");
        }
    });

    // Chat bubble: human-readable objective (Goal badge in render).
    let mut chat_msg = ChatMessage::new(ChatRole::User, objective.clone()).stamp_now();
    chat_msg.is_goal = true;
    app.messages.push(chat_msg);

    // Model-visible framing (shared with SDK/bindings hosts).
    let model_text = navi_core::build_host_set_goal_user_prompt(&objective);
    app.compact_state.add_unsent_bytes(model_text.len());
    app.conversation_history
        .push(ModelMessage::user(model_text.clone()));

    let submitted_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();
    app.events.push(AgentEvent::UserTaskSubmitted {
        text: model_text,
        content_parts: Vec::new(),
        submitted_at,
    });
    crate::persistence::checkpoint_session_now(app);

    app.input.clear();
    app.input_cursor = 0;
    app.input_selection = None;
    app.scroll_offset = 0;
    app.reset_run_state();
    app.model_retry_attempts = 0;

    if app.is_loading {
        // Rare: a turn started between modal open and submit — queue instead.
        // Goal is already set on the runtime; queue a short reminder turn.
        app.queued_user_messages.push_back(QueuedUserMessage {
            text: format!("Continue the active goal: {objective}"),
            images: Vec::new(),
        });
        return;
    }

    start_streaming_request(app);
}

fn queue_current_message(app: &mut TuiApp) {
    // Caller already expanded pastes via submit_message.
    let text = std::mem::take(&mut app.input);
    let images = std::mem::take(&mut app.pending_images);
    app.pending_pastes.clear();
    app.queued_user_messages
        .push_back(QueuedUserMessage { text, images });
    app.input_cursor = 0;
    app.input_selection = None;
    app.scroll_offset = 0;
    tracing::info!(
        queued = app.queued_user_messages.len(),
        "TUI prompt queued behind active turn"
    );
}

pub(crate) fn drain_next_queued_message(app: &mut TuiApp) {
    if app.is_loading {
        return;
    }
    let Some(next) = app.queued_user_messages.pop_front() else {
        return;
    };

    // Preserve any draft the user typed into the input while the previous turn
    // was still running. Submitting the queued message must not wipe that draft.
    let draft_text = std::mem::take(&mut app.input);
    let draft_cursor = app.input_cursor;
    let draft_selection = app.input_selection.take();
    let draft_images = std::mem::take(&mut app.pending_images);

    app.input = next.text;
    app.input_cursor = app.input.len();
    app.input_selection = None;
    app.pending_images = next.images;
    submit_current_message_now(app);

    // `submit_current_message_now` clears input/images for the sent message.
    // Restore the in-progress draft afterward.
    app.input = draft_text;
    app.input_cursor = draft_cursor.min(app.input.len());
    app.input_selection = draft_selection;
    // Only restore draft images if submit didn't leave residual pending images
    // (it clears them when submitting). Safe to assign back.
    app.pending_images = draft_images;
}

fn submit_current_message_now(app: &mut TuiApp) {
    let has_images = !app.pending_images.is_empty();
    let text = app.input.clone();
    // Hydrate @file / @folder mentions for the model (chat bubble keeps the short form).
    let model_text = crate::path_mentions::hydrate_path_mentions(&app.project_dir, &text);
    tracing::info!(
        model = %app.loaded_config.config.model.name,
        provider = %app.loaded_config.config.model.provider,
        chars = text.len(),
        model_chars = model_text.len(),
        images = has_images,
        "TUI prompt submitted"
    );

    if has_images {
        warn_if_model_cannot_view_images(app);
    }

    let mut chat_msg = ChatMessage::new(ChatRole::User, text.clone()).stamp_now();

    // Collect pending images into ContentParts and labels.
    let mut content_parts: Vec<ContentPart> = Vec::new();
    if has_images {
        // Parse `text` to find `[Image N]` tags and map them to the corresponding pending images.
        let mut referenced_indices = std::collections::HashSet::new();
        let mut last_idx = 0;
        let bytes = text.as_bytes();

        while last_idx < text.len() {
            let rest = &text[last_idx..];
            if rest.starts_with("[Image ") {
                let mut check_idx = 7;
                let mut has_digits = false;
                while check_idx < rest.len() && bytes[last_idx + check_idx].is_ascii_digit() {
                    has_digits = true;
                    check_idx += 1;
                }
                if has_digits && check_idx < rest.len() && bytes[last_idx + check_idx] == b']' {
                    let num_str = &rest[7..check_idx];
                    if let Ok(num) = num_str.parse::<usize>() {
                        let img_idx = num.saturating_sub(1);
                        if img_idx < app.pending_images.len() {
                            referenced_indices.insert(img_idx);
                            let img = &app.pending_images[img_idx];

                            content_parts.push(ContentPart::Image {
                                media_type: img.media_type.clone(),
                                data: img.data.clone(),
                            });

                            let chat_image = ChatImage::from_pending(num, img);
                            chat_msg.image_labels.push(format!("[Image {}]", num));
                            chat_msg.images.push(chat_image);

                            last_idx += check_idx + 1;
                            continue;
                        }
                    }
                }
            }

            let search_start = rest
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(rest.len());
            let next_image_pos = rest[search_start..]
                .find("[Image ")
                .map(|pos| pos + search_start)
                .unwrap_or(rest.len());
            let text_chunk = &rest[..next_image_pos];
            content_parts.push(ContentPart::Text {
                text: text_chunk.to_string(),
            });
            last_idx += text_chunk.len();
        }

        // Consume all pending images since they are processed/cleared now.
        app.pending_images.clear();

        // Merge adjacent Text parts
        let mut merged_parts = Vec::new();
        for part in content_parts {
            match part {
                ContentPart::Text { text } => {
                    if let Some(ContentPart::Text { text: last_text }) = merged_parts.last_mut() {
                        last_text.push_str(&text);
                    } else {
                        merged_parts.push(ContentPart::Text { text });
                    }
                }
                other => merged_parts.push(other),
            }
        }
        content_parts = merged_parts;
    }

    app.messages.push(chat_msg);

    app.compact_state.add_unsent_bytes(model_text.len());
    // Prefer hydrated text for the model; keep image parts when present.
    let mut model_parts = content_parts.clone();
    if model_text != text {
        if model_parts.is_empty() {
            // plain text path uses model_text below
        } else {
            // Replace leading/only text part with hydrated content when possible.
            match model_parts.first_mut() {
                Some(ContentPart::Text { text: t }) => *t = model_text.clone(),
                _ => model_parts.insert(
                    0,
                    ContentPart::Text {
                        text: model_text.clone(),
                    },
                ),
            }
        }
    }
    if model_parts.is_empty() {
        app.conversation_history
            .push(ModelMessage::user(model_text.clone()));
    } else {
        app.conversation_history.push(ModelMessage::user_multimodal(
            model_text.clone(),
            model_parts.clone(),
        ));
    }

    let submitted_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();
    app.events.push(AgentEvent::UserTaskSubmitted {
        text: model_text.clone(),
        content_parts: if model_parts.is_empty() {
            content_parts.clone()
        } else {
            model_parts.clone()
        },
        submitted_at,
    });
    // Durability: persist the accepted user prompt before the agent turn runs.
    // A kill mid-tool-loop still leaves a resumable session with this ask.
    crate::persistence::checkpoint_session_now(app);

    app.input.clear();
    app.input_cursor = 0;
    app.input_selection = None;
    app.pending_pastes.clear();
    app.scroll_offset = 0;
    app.reset_run_state();
    app.model_retry_attempts = 0;

    start_streaming_request(app);
}

fn is_model_response_message(message: &ChatMessage) -> bool {
    message.role == ChatRole::Assistant
        && message.tool_invocation.is_none()
        && message.tool_result.is_none()
        && !message.is_compact_summary
}

pub(crate) fn tail_model_response(app: &mut TuiApp) -> Option<&mut ChatMessage> {
    if app.messages.last().is_some_and(is_model_response_message) {
        app.messages.last_mut()
    } else {
        None
    }
}

pub(crate) fn active_assistant_message(app: &mut TuiApp) -> Option<&mut ChatMessage> {
    app.messages
        .iter_mut()
        .rev()
        .find(|message| is_model_response_message(message))
}

fn model_response_placeholder(app: &TuiApp) -> ChatMessage {
    let model_label = app.loaded_config.config.model.name.clone();
    let provider_label = selected_provider_label(app).to_string();
    ChatMessage {
        model_label: Some(model_label),
        provider_label: Some(provider_label),
        status: Some("thinking".to_string()),
        ..ChatMessage::new(ChatRole::Assistant, String::new())
    }
}

fn warn_if_model_cannot_view_images(app: &mut TuiApp) {
    let config = &app.loaded_config.config;
    let provider = config.model.provider.as_str();
    let model = config.model.name.as_str();
    if model_supports_attachment(config, provider, model, AttachmentKind::Image) {
        return;
    }
    let has_attachment_model = config.attachment_models.image.is_some();
    let detail = if has_attachment_model {
        format!(
            "{model} via {provider} cannot view images directly. NAVI will not inline image bytes into the prompt; configure analyze_attachment routing or switch to a vision model."
        )
    } else {
        format!(
            "{model} via {provider} cannot view images (supports_images=false/unknown). Switch to a vision model (ctrl+m) or set attachment_models.image for analyze_attachment."
        )
    };
    tracing::warn!(%provider, %model, "image attached to non-vision chat model");
    show_notification(app, "Image", detail);
}

pub(crate) fn ensure_tail_model_response(app: &mut TuiApp) -> &mut ChatMessage {
    let needs_message = app
        .messages
        .last()
        .is_none_or(|message| !is_model_response_message(message));
    if needs_message {
        let message = model_response_placeholder(app);
        app.messages.push(message);
    }
    // If another concurrent path cleared `messages` between the push and this
    // access, fall back to a transient in-memory placeholder so the caller can
    // still write to it without panicking the TUI.
    if app.messages.is_empty() {
        tracing::error!(
            "messages became empty immediately after pushing a model response placeholder"
        );
        let mut message = model_response_placeholder(app);
        message.thinking_content.clear();
        app.messages.push(message);
    }
    // Invariant: the empty-branch above always pushes before this access.
    app.messages
        .last_mut()
        .expect("placeholder was just pushed")
}

pub(crate) fn update_active_assistant_status(app: &mut TuiApp) {
    let status = if !app.pending_approvals.is_empty() {
        if app.pending_approvals.len() == 1 {
            let req = &app.pending_approvals[0];
            let name = app
                .tool_invocations
                .get(&req.id)
                .map(|inv| inv.tool_name.as_str())
                .unwrap_or("tool");
            Some(format!("approval: {}", name))
        } else {
            Some(format!("approval: {} tools", app.pending_approvals.len()))
        }
    } else if !app.pending_questions.is_empty() {
        if app.pending_questions.len() == 1 {
            Some("question".to_string())
        } else {
            Some(format!("questions: {}", app.pending_questions.len()))
        }
    } else if !app.running_tools.is_empty() {
        if app.running_tools.len() == 1 {
            let name = app
                .running_tools
                .values()
                .next()
                .map(|inv| inv.tool_name.as_str())
                .unwrap_or("tool");
            Some(format!("tool: {}", name))
        } else {
            let names: Vec<&str> = app
                .running_tools
                .values()
                .map(|inv| inv.tool_name.as_str())
                .collect();
            Some(format!("tool: {}", names.join(", ")))
        }
    } else if !app.streaming_tool_calls.is_empty() {
        if app.streaming_tool_calls.len() == 1 {
            let name = app
                .streaming_tool_calls
                .first()
                .map(|c| c.tool_name.as_str())
                .unwrap_or("tool");
            Some(format!("streaming_tool:{name}"))
        } else {
            Some(format!(
                "streaming_tool:{} tools",
                app.streaming_tool_calls.len()
            ))
        }
    } else if app.is_loading {
        Some("thinking".to_string())
    } else {
        None
    };

    if let Some(status) = status {
        let msg = ensure_tail_model_response(app);
        msg.status = Some(status);
    } else if let Some(msg) = tail_model_response(app) {
        msg.status = None;
    }
}

pub(crate) fn finalize_active_assistant(app: &mut TuiApp, elapsed_ms: u64, fallback_text: &str) {
    app.model_retry_attempts = 0;
    let model_name = app.loaded_config.config.model.name.clone();
    let provider_id = app.loaded_config.config.model.provider.clone();
    let (text, thinking) = {
        let active = if fallback_text.trim().is_empty() {
            // The turn returned no final text. Try the tail model response
            // first (the common case when the model emitted deltas but no
            // tool calls). If the tail is a tool-result message, fall back to
            // the last assistant model-response message — it may contain text
            // the model streamed before making tool calls.
            match tail_model_response(app) {
                Some(active) => active,
                None => match active_assistant_message(app) {
                    Some(active)
                        if !active.content.trim().is_empty()
                            || !active.thinking_content.trim().is_empty() =>
                    {
                        active
                    }
                    _ => ensure_tail_model_response(app),
                },
            }
        } else {
            ensure_tail_model_response(app)
        };
        if active.content.trim().is_empty() && !fallback_text.trim().is_empty() {
            active.content = fallback_text.to_string();
        }
        // Some models (esp. after a mid-session model switch with thinking on)
        // stream only into the reasoning channel and leave content empty. Prefer
        // promoting that thinking over painting a useless "No response."
        if active.content.trim().is_empty() && !active.thinking_content.trim().is_empty() {
            active.content = std::mem::take(&mut active.thinking_content);
            tracing::info!(
                elapsed_ms,
                "promoted thinking-only stream to assistant content (empty final text)"
            );
        }
        if active.content.trim().is_empty() {
            active.content = format!(
                "No response from `{model_name}` ({provider_id}). The model returned empty content — try again, turn thinking off, or switch models."
            );
            tracing::warn!(
                model = %model_name,
                provider = %provider_id,
                elapsed_ms,
                "turn finalized with empty assistant content"
            );
        }
        active.elapsed_ms = Some(elapsed_ms);
        active.status = None;
        (
            active.content.clone(),
            if active.thinking_content.is_empty() {
                None
            } else {
                Some(active.thinking_content.clone())
            },
        )
    };
    if text.trim().is_empty() {
        // Should be unreachable after the empty-content fallback above.
        return;
    }

    app.compact_state.add_unsent_bytes(text.len());
    app.conversation_history
        .push(ModelMessage::assistant_with_thinking(
            text.clone(),
            thinking.clone(),
        ));
    app.events.push(AgentEvent::ModelOutput { text, thinking });
    tracing::info!(elapsed_ms, "TUI model stream finalized");
    crate::persistence::schedule_session_checkpoint(app);
}

pub(crate) fn remove_active_empty_generation_placeholder(app: &mut TuiApp) {
    let Some(index) = app.messages.iter().rposition(|message| {
        message.role == ChatRole::Assistant
            && message.content.trim().is_empty()
            && message.thinking_content.trim().is_empty()
            && message.status.as_deref().is_some_and(|status| {
                status == "thinking" || status == "receiving" || status.starts_with("tool:")
            })
    }) else {
        return;
    };
    app.messages.remove(index);
}

pub(crate) fn remove_active_tool_placeholder(app: &mut TuiApp) {
    let Some(index) = app.messages.iter().rposition(|message| {
        message.role == ChatRole::Assistant
            && message.content.trim().is_empty()
            && message.thinking_content.trim().is_empty()
            && message.status.as_deref().is_some_and(|status| {
                status.starts_with("tool:")
                    || status.starts_with("approval:")
                    || status == "thinking"
                    || status == "receiving"
            })
    }) else {
        return;
    };
    app.messages.remove(index);
}

pub(crate) fn retry_last_response(app: &mut TuiApp) {
    if app.is_loading {
        cancel_stream(app);
    }

    if app
        .messages
        .last()
        .is_some_and(|message| message.role == ChatRole::Assistant)
    {
        app.messages.pop();
    }
    if app
        .conversation_history
        .last()
        .is_some_and(|message| matches!(message.role, ModelRole::Assistant))
    {
        app.conversation_history.pop();
    }
    if app
        .events
        .last()
        .is_some_and(|event| matches!(event, AgentEvent::ModelOutput { .. }))
    {
        app.events.pop();
    }

    if app
        .conversation_history
        .last()
        .is_some_and(|message| matches!(message.role, ModelRole::User))
    {
        start_streaming_request(app);
    }
}

pub(crate) fn revert_to_user_message(app: &mut TuiApp, message_index: usize) -> Result<(), String> {
    let prompt = user_message_text(app, message_index)?;
    ensure_safe_history_action(app)?;
    let keep_user_turns = app
        .messages
        .iter()
        .take(message_index)
        .filter(|message| message.role == ChatRole::User)
        .count();
    let prefix = prefix_before_user_message(app, message_index)?;
    apply_prefix(app, prefix);
    app.input = prompt;
    app.input_cursor = app.input.len();
    app.scroll_offset = 0;

    // Engine: truncate live session history + restore project files.
    let session_id = app.session_id.as_str().to_string();
    let engine = app.engine();
    crate::runtime::spawn_runtime_task(async move {
        match engine.rewind_session(&session_id, keep_user_turns).await {
            Ok(_) => tracing::info!(
                keep_user_turns,
                "TUI revert: engine rewind + filesystem restore complete"
            ),
            Err(err) => tracing::warn!(
                error = %err,
                keep_user_turns,
                "TUI revert: engine rewind failed (UI history already truncated)"
            ),
        }
    });
    // Persist truncated UI session so reload matches (keep same session id).
    crate::persistence::snapshot_current_session(app);
    Ok(())
}

/// User-message checkpoints for the Rewind palette modal: `(message_index, preview)`.
pub(crate) fn rewind_checkpoints(app: &TuiApp) -> Vec<(usize, String)> {
    app.messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == ChatRole::User)
        .map(|(i, m)| {
            let preview = truncate_checkpoint_preview(&m.content, 80);
            (i, preview)
        })
        .collect()
}

fn truncate_checkpoint_preview(text: &str, max_chars: usize) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() <= max_chars {
        return t;
    }
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if i >= max_chars.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

pub(crate) fn fork_from_user_message(app: &mut TuiApp, message_index: usize) -> Result<(), String> {
    let prompt = user_message_text(app, message_index)?;
    ensure_safe_history_action(app)?;
    let prefix = prefix_before_user_message(app, message_index)?;
    crate::persistence::save_current_session(app);
    apply_prefix(app, prefix);
    app.input = prompt;
    app.input_cursor = app.input.len();
    app.scroll_offset = 0;
    Ok(())
}

pub(crate) fn start_new_session(app: &mut TuiApp) {
    crate::persistence::flush_session_checkpoint(app);

    let old_session_id = app.session_id.as_str().to_string();
    let engine = app.engine();
    crate::runtime::spawn_runtime_task(async move {
        let _ = engine.close_session(&old_session_id).await;
    });

    app.abort_async_tasks();
    if let Some(task) = app.bg_poll_task.take() {
        task.abort();
    }

    app.session_id = SessionStore::create_id();
    app.messages.clear();
    app.events.clear();
    reset_system_context(app);
    app.compact_state = CompactState::new(effective_context_window(&app.loaded_config.config));

    app.input.clear();
    app.input_cursor = 0;
    app.input_selection = None;
    app.pending_images.clear();
    app.queued_user_messages.clear();
    app.queued_message_selected = 0;
    app.queued_message_scroll = 0;
    app.queued_edit_index = None;
    app.queued_edit_text.clear();
    app.queued_edit_cursor = 0;
    app.scroll_offset = 0;
    app.is_loading = false;
    app.loading_start = None;
    app.skip_next_model_done = false;
    app.model_retry_attempts = 0;
    app.cancel_esc_pressed = false;

    app.pending_approvals.clear();
    app.pending_questions.clear();
    app.running_tools.clear();
    app.streaming_tool_calls.clear();
    app.subagent_activity.clear();
    app.subagent_transcripts.clear();
    app.subagent_order.clear();
    app.chat_view = crate::state::ChatView::Parent;
    app.tool_invocations.clear();
    app.background_commands.clear();
    app.pending_pastes.clear();
    app.message_action_target = None;
    // Keep selected_message_action — last Message Actions choice is a preference.
    app.expanded_tool_results.clear();
    app.collapsed_tool_results.clear();
    app.hovered_chat_source = None;
    app.selected_chat_source = None;
    app.selection = None;
    app.hover_index = None;
    app.session_title = None;
    app.session_goal = None;
    app.session_checkpoint_due = None;
    // Fresh session: reset token/cost accumulators (persisted separately per snapshot).
    app.usage_state.session_input_tokens = 0;
    app.usage_state.session_output_tokens = 0;
    app.usage_state.session_cost_usd = 0.0;
    app.usage_state.session_cost_known = false;
    app.usage_state.session_credits_spent = None;
    app.usage_state.session_credit_unit = None;
    app.usage_state.remaining_credits = None;
    app.usage_state.remaining_credit_unit = None;
    app.usage_state.report = None;
    app.usage_state.error = None;
    app.usage_state.last_input_tokens = None;
    app.usage_state.last_output_tokens = None;
    app.usage_state.last_turn_label = None;
    app.usage_state.reset_request_usage();
    app.usage_state.last_account_refresh_at = None;
    app.chat_render_cache.borrow_mut().signature_hash = 0;
}

fn user_message_text(app: &TuiApp, message_index: usize) -> Result<String, String> {
    let Some(message) = app.messages.get(message_index) else {
        return Err("Message no longer exists.".to_string());
    };
    if message.role != ChatRole::User {
        return Err("Only user messages can be reverted or forked.".to_string());
    }
    Ok(message.content.clone())
}

fn ensure_safe_history_action(app: &TuiApp) -> Result<(), String> {
    if app.is_loading || app.has_async_task() || !app.running_tools.is_empty() {
        return Err("Wait for the active turn to finish before changing history.".to_string());
    }
    if !app.pending_approvals.is_empty() || !app.pending_questions.is_empty() {
        return Err("Resolve pending approvals/questions before changing history.".to_string());
    }
    Ok(())
}

fn prefix_before_user_message(app: &TuiApp, message_index: usize) -> Result<ChatPrefix, String> {
    user_message_text(app, message_index)?;
    let target_user_ordinal = app
        .messages
        .iter()
        .take(message_index)
        .filter(|message| message.role == ChatRole::User)
        .count();

    let messages = app.messages[..message_index].to_vec();
    let conversation_history =
        truncate_model_history_before_user(&app.conversation_history, target_user_ordinal);
    let events = truncate_events_before_user(&app.events, target_user_ordinal);

    Ok(ChatPrefix {
        messages,
        conversation_history,
        events,
    })
}

fn truncate_model_history_before_user(
    history: &[ModelMessage],
    target_user_ordinal: usize,
) -> Vec<ModelMessage> {
    let mut user_seen = 0usize;
    let mut prefix = Vec::new();
    for message in history {
        if matches!(message.role, ModelRole::User) {
            if user_seen == target_user_ordinal {
                break;
            }
            user_seen += 1;
        }
        prefix.push(message.clone());
    }
    prefix
}

fn truncate_events_before_user(
    events: &[AgentEvent],
    target_user_ordinal: usize,
) -> Vec<AgentEvent> {
    let mut user_seen = 0usize;
    let mut prefix = Vec::new();
    for event in events {
        if matches!(event, AgentEvent::UserTaskSubmitted { .. }) {
            if user_seen == target_user_ordinal {
                break;
            }
            user_seen += 1;
        }
        prefix.push(event.clone());
    }
    prefix
}

fn apply_prefix(app: &mut TuiApp, prefix: ChatPrefix) {
    app.messages = prefix.messages;
    app.conversation_history = prefix.conversation_history;
    app.events = prefix.events;
    app.pending_approvals.clear();
    app.pending_questions.clear();
    app.running_tools.clear();
    app.streaming_tool_calls.clear();
    app.subagent_activity.clear();
    app.subagent_transcripts.clear();
    app.subagent_order.clear();
    app.chat_view = crate::state::ChatView::Parent;
    app.expanded_tool_results.clear();
    app.collapsed_tool_results.clear();
    app.hovered_chat_source = None;
    app.selected_chat_source = None;
    app.tool_invocations.clear();
    for event in &app.events {
        if let AgentEvent::ToolRequested(invocation) = event {
            app.tool_invocations
                .insert(invocation.id.clone(), invocation.clone());
        }
    }
    app.reset_run_state();
    app.model_retry_attempts = 0;
}

pub(crate) fn reset_system_context(app: &mut TuiApp) {
    app.conversation_history = vec![ModelMessage::system(build_system_prompt(
        &app.loaded_config.config,
        &app.project_dir,
    ))];
    app.reset_run_state();
}

pub(crate) fn refresh_system_context(app: &mut TuiApp) {
    let system = ModelMessage::system(build_system_prompt(
        &app.loaded_config.config,
        &app.project_dir,
    ));
    if let Some(first) = app.conversation_history.first_mut() {
        *first = system;
    } else {
        app.conversation_history.push(system);
    }
}
