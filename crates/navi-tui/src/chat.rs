use navi_sdk::{AgentEvent, ModelMessage, ModelRole, build_system_prompt};
use tracing;

use crate::TuiApp;
use crate::providers::selected_provider_label;
use crate::state::{ChatMessage, ChatRole};
use crate::stream::start_streaming_request;
use crate::tools::cancel_stream;

pub(crate) fn submit_message(app: &mut TuiApp) {
    let text = agent_prompt_text(app);
    if text.is_empty() {
        return;
    }
    tracing::info!(
        model = %app.loaded_config.config.model.name,
        provider = %app.loaded_config.config.model.provider,
        chars = text.len(),
        "TUI prompt submitted"
    );

    app.messages
        .push(ChatMessage::new(ChatRole::User, text.clone()));

    app.compact_state.add_unsent_bytes(text.len());
    app.conversation_history
        .push(ModelMessage::user(text.clone()));

    app.events
        .push(AgentEvent::UserTaskSubmitted { text: text.clone() });

    app.input.clear();
    app.input_cursor = 0;
    app.scroll_offset = 0;
    app.reset_run_state();
    app.model_retry_attempts = 0;

    start_streaming_request(app);
}

fn agent_prompt_text(app: &TuiApp) -> String {
    let text = app.input.trim();
    if text.is_empty() {
        return String::new();
    }
    app.selected_agent
        .map(|agent| agent.apply_to_prompt(text))
        .unwrap_or_else(|| text.to_string())
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
    let (text, thinking) = {
        let active = if fallback_text.trim().is_empty() {
            match tail_model_response(app) {
                Some(active) => active,
                None => {
                    let active = ensure_tail_model_response(app);
                    active.content = "No response.".to_string();
                    active
                }
            }
        } else {
            ensure_tail_model_response(app)
        };
        if active.content.trim().is_empty() && !fallback_text.trim().is_empty() {
            active.content = fallback_text.to_string();
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
        if let Some(active) = active_assistant_message(app) {
            active.content = "No response.".to_string();
        }
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
                status.starts_with("tool:") || status.starts_with("approval:")
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
