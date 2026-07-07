use navi_sdk::{AgentEvent, ApprovalDecision, ModelMessage, ToolInvocation};

use crate::app::TuiApp;
use crate::chat::{active_assistant_message, tail_model_response, update_active_assistant_status};
use crate::notifications::push_diagnostic;
use crate::runtime::spawn_runtime_task;

pub(crate) fn record_tool_requested(app: &mut TuiApp, invocation: ToolInvocation) {
    app.tool_invocations
        .insert(invocation.id.clone(), invocation.clone());
    app.running_tools
        .insert(invocation.id.clone(), invocation.clone());

    // Preserve assistant text/thinking on the assistant message that contains
    // all tool calls from this model turn. Chat Completions providers reject
    // histories with adjacent assistant tool-call messages before tool results.
    let is_continuation_tool_call = app
        .conversation_history
        .last()
        .is_some_and(|message| !message.tool_calls.is_empty());
    if is_continuation_tool_call {
        if let Some(message) = app.conversation_history.last_mut() {
            message.tool_calls.push(invocation.clone());
        }
    } else {
        let active_msg = tail_model_response(app);
        let active_content = active_msg
            .as_ref()
            .map(|active| active.content.clone())
            .unwrap_or_default();
        let active_thinking = active_msg.as_ref().and_then(|active| {
            if active.thinking_content.is_empty() {
                None
            } else {
                Some(active.thinking_content.clone())
            }
        });
        if !active_content.trim().is_empty() {
            app.compact_state.add_unsent_bytes(active_content.len());
        }
        app.conversation_history
            .push(ModelMessage::assistant_tool_call_with_context(
                invocation.clone(),
                active_content,
                active_thinking,
            ));
    }

    let invocation_json = serde_json::to_string(&invocation).unwrap_or_default();
    app.compact_state.add_unsent_bytes(invocation_json.len());
    app.events.push(AgentEvent::ToolRequested(invocation));
    update_active_assistant_status(app);
}

pub(crate) fn approve_pending_tool(app: &mut TuiApp) {
    if !app.pending_approvals.is_empty() {
        let request = app.pending_approvals.remove(0);
        tracing::info!(invocation_id = %request.id, "tool approval accepted via pending_approvals");
        let engine = app.engine();
        let session_id = app.session_id.as_str().to_string();
        let decision = ApprovalDecision::Approved {
            id: request.id.clone(),
        };
        spawn_runtime_task(async move {
            let _ = engine.resolve_approval(&session_id, decision).await;
        });
        update_active_assistant_status(app);
    }
}

pub(crate) fn deny_pending_tool(app: &mut TuiApp) {
    if !app.pending_approvals.is_empty() {
        let request = app.pending_approvals.remove(0);
        tracing::warn!(invocation_id = %request.id, "tool approval denied via pending_approvals");
        push_diagnostic(app, format!("Denied tool ID: {}", request.id));
        let engine = app.engine();
        let session_id = app.session_id.as_str().to_string();
        let decision = ApprovalDecision::Denied {
            id: request.id.clone(),
        };
        spawn_runtime_task(async move {
            let _ = engine.resolve_approval(&session_id, decision).await;
        });
        update_active_assistant_status(app);
    }
}

pub(crate) fn cancel_stream(app: &mut TuiApp) {
    let (had_stream, had_tool) = app.abort_async_tasks();
    tracing::warn!(had_stream, had_tool, "active operation cancelled");
    push_diagnostic(app, "Cancelled active operation.");
    let engine = app.engine();
    let session_id = app.session_id.as_str().to_string();
    spawn_runtime_task(async move {
        let _ = engine.cancel_turn(&session_id).await;
    });
    app.is_loading = false;
    app.loading_start = None;
    app.pending_approvals.clear();
    app.running_tools.clear();
    app.subagent_activity.clear();
    app.queued_user_messages.clear();
    app.queued_message_selected = 0;
    app.queued_message_scroll = 0;
    app.queued_edit_index = None;
    app.queued_edit_text.clear();
    app.queued_edit_cursor = 0;
    app.close_subagent_view();
    app.skip_next_model_done = false;
    if let Some(active) = active_assistant_message(app) {
        active.status = Some("cancelled".to_string());
        if active.content.is_empty() {
            active.content = "Cancelled.".to_string();
        }
    }
}
