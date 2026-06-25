use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::TuiApp;
use crate::chat::{fork_from_user_message, revert_to_user_message};
use crate::commands::filtered_commands;
use crate::keybindings::{close_active_modal, handle_key, replace_modal};
use crate::notifications::{push_diagnostic, show_notification};
use crate::plugins::{install_or_update_from_marketplace, plugin_picker_rows};
use crate::providers::{
    ListRow, apply_model_selection, build_model_rows, first_model_index, selected_model_in_rows,
    sync_scroll_to_selection,
};
use crate::render::text::display_width;
use crate::runtime::provider_supports_oauth;
use crate::state::{Mode, SelectionState};
use crate::ui::interaction::{HitAction, HitRegion, ScrollTarget};
use crate::ui::list::SelectListState;

fn map_mouse_to_text(app: &TuiApp, col: u16, row: u16) -> Option<(usize, usize)> {
    let cache = app.chat_render_cache.borrow();
    let inner = cache.chat_rect?;
    if col < inner.x
        || col >= inner.x + inner.width
        || row < inner.y
        || row >= inner.y + inner.height
    {
        return None;
    }
    let visible_y = (row - inner.y) as usize;

    let total_lines = cache.lines.len();
    let visible_height = inner.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = app.scroll_offset.min(max_scroll);
    let start = total_lines
        .saturating_sub(visible_height)
        .saturating_sub(effective_scroll);

    let line_index = start + visible_y;
    if line_index >= total_lines {
        return None;
    }

    let char_index = (col - inner.x) as usize;
    Some((line_index, char_index))
}

pub(crate) fn selected_text(app: &TuiApp) -> Option<String> {
    let selection = if let Some(sel) = &app.selection {
        sel
    } else {
        return None;
    };

    let start = selection.start.min(selection.end);
    let end = selection.start.max(selection.end);

    let cache = app.chat_render_cache.borrow();
    let mut selected_text = String::new();

    for line_idx in start.0..=end.0 {
        if let Some(line) = cache.lines.get(line_idx) {
            let mut line_text = String::new();
            for span in &line.spans {
                line_text.push_str(&span.content);
            }

            let start_char = if line_idx == start.0 { start.1 } else { 0 };
            let end_char = if line_idx == end.0 {
                end.1
            } else {
                display_width(&line_text)
            };

            let substr: String = line_text
                .chars()
                .skip(start_char)
                .take(end_char.saturating_sub(start_char))
                .collect();
            selected_text.push_str(&substr);

            if line_idx != end.0 {
                selected_text.push('\n');
            }
        }
    }

    (!selected_text.is_empty()).then_some(selected_text)
}

pub(crate) fn copy_text_to_clipboard(app: &mut TuiApp, text: &str) {
    if text.is_empty() {
        return;
    }
    // ALWAYS send OSC 52 as a robust fallback for terminals.
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(text);
    print!("\x1B]52;c;{}\x07", b64);
    let _ = std::io::Write::flush(&mut std::io::stdout());

    show_notification(app, "Clipboard", "Text copied (OSC 52).".to_string());
}

pub(crate) fn finish_selection(app: &mut TuiApp, end: Option<(usize, usize)>) -> bool {
    let Some(selection) = &mut app.selection else {
        return false;
    };
    if !selection.active {
        return false;
    }
    if let Some(end) = end {
        selection.end = end;
    }
    selection.active = false;
    if selection.start == selection.end {
        return false;
    }
    selected_text(app).is_some()
}

pub(crate) fn handle_mouse(app: &mut TuiApp, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if scroll_active_list(app, 3) {
                return;
            }
            app.scroll_offset = app.scroll_offset.saturating_sub(3);
        }
        MouseEventKind::ScrollUp => {
            if scroll_active_list(app, -3) {
                return;
            }
            app.scroll_offset = app.scroll_offset.saturating_add(3);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(hit) = app.hit_test(mouse.column, mouse.row) {
                if matches!(hit.action, HitAction::ScrollTo { .. }) {
                    dispatch_hit(app, hit);
                    return;
                }
                app.hover_index = None;
                app.hovered_chat_source = chat_source_for_action(&hit.action);
                dispatch_hit(app, hit);
                return;
            }
            app.hover_index = None;
            app.hovered_chat_source = None;
            if let Some(pos) = map_mouse_to_text(app, mouse.column, mouse.row) {
                app.selection = Some(SelectionState {
                    start: pos,
                    end: pos,
                    active: true,
                });
            } else {
                app.selection = None;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(hit) = app.hit_test(mouse.column, mouse.row)
                && matches!(hit.action, HitAction::ScrollTo { .. })
            {
                dispatch_hit(app, hit);
                app.selection = None;
                return;
            }
            if let Some(pos) = map_mouse_to_text(app, mouse.column, mouse.row)
                && let Some(selection) = &mut app.selection
                && selection.active
            {
                selection.end = pos;
            }
            if app.selection.as_ref().map(|s| s.active).unwrap_or(false)
                && let Some(inner) = app.chat_render_cache.borrow().chat_rect
            {
                if mouse.row <= inner.y + 1 {
                    app.scroll_offset = app.scroll_offset.saturating_add(1);
                } else if mouse.row >= inner.y + inner.height.saturating_sub(2) {
                    app.scroll_offset = app.scroll_offset.saturating_sub(1);
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.hit_test(mouse.column, mouse.row).is_some() {
                app.selection = None;
                return;
            }
            let pos = map_mouse_to_text(app, mouse.column, mouse.row);
            if finish_selection(app, pos) {
                if let Some(text) = selected_text(app) {
                    copy_text_to_clipboard(app, &text);
                }
            }
        }
        MouseEventKind::Moved => {
            if let Some(hit) = app.hit_test(mouse.column, mouse.row) {
                apply_hover(app, &hit);
            } else {
                app.hover_index = None;
                app.hovered_chat_source = None;
            }
        }
        _ => {}
    }
}

fn apply_hover(app: &mut TuiApp, hit: &HitRegion) {
    app.hovered_chat_source = chat_source_for_action(&hit.action);
    match &hit.action {
        HitAction::QuestionOption(index) => {
            app.hover_index = Some(*index);
        }
        HitAction::QuestionText => {
            app.hover_index = None;
        }
        HitAction::QuestionDeny => {
            app.hover_index = None;
        }
        HitAction::Command(index) => app.hover_index = Some(*index),
        HitAction::Model(index) => app.hover_index = Some(*index),
        HitAction::ProviderApiKey(index) => {
            app.hover_index = Some(*index);
        }
        HitAction::Session(index) => app.hover_index = Some(*index),
        HitAction::Skill(index) => app.hover_index = Some(*index),
        HitAction::Setting(index) => app.hover_index = Some(*index),
        HitAction::MessageAction(index) => app.hover_index = Some(*index),
        HitAction::PluginInstallOrUpdate(index) => {
            app.hover_index = Some(*index);
        }
        HitAction::ThemeSelect(index) => app.hover_index = Some(*index),
        _ => {
            app.hover_index = None;
        }
    }
}

fn chat_source_for_action(action: &HitAction) -> Option<crate::state::ChatLineSource> {
    match action {
        HitAction::ChatMessage(index) => Some(crate::state::ChatLineSource::Message(*index)),
        HitAction::ToolResult(id) => Some(crate::state::ChatLineSource::ToolResult(id.clone())),
        HitAction::ToolGroup(ids) => Some(crate::state::ChatLineSource::ToolGroup(ids.clone())),
        HitAction::Subagent(id) => Some(crate::state::ChatLineSource::Subagent(id.clone())),
        _ => None,
    }
}

fn dispatch_hit(app: &mut TuiApp, hit: HitRegion) {
    match hit.action {
        HitAction::Key { code, modifiers } => {
            let _ = handle_key(app, code, modifiers);
        }
        HitAction::CloseModal => close_active_modal(app),
        HitAction::ReopenQuestion => replace_modal(app, crate::state::ModalKind::Question),
        HitAction::QuestionOption(index) => {
            if let Some(question) = app.pending_questions.first_mut() {
                question.selected_row = index;
                if question.request.multiple
                    && let Some(selected) = question.selected_options.get_mut(index)
                {
                    *selected = !*selected;
                }
            }
        }
        HitAction::QuestionText => {
            if let Some(question) = app.pending_questions.first_mut() {
                question.focus_custom();
            }
        }
        HitAction::QuestionDeny => {
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Char('n'),
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::QuestionSend => {
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::Command(index) => {
            app.selected_command = index;
            let _ = crate::keybindings::run_selected_command(app);
        }
        HitAction::Model(index) => {
            app.selected_model = index;
            if let Some(model) = app.models.get(index).cloned() {
                if crate::providers::model_is_available_for_selection(app, &model) {
                    apply_model_selection(app, index);
                    crate::keybindings::close_all_modals(app);
                } else {
                    app.pending_model_selection = Some(index);
                    replace_modal(app, crate::state::ModalKind::ApiKeyEntry);
                    app.api_key_input.clear();
                    app.api_key_cursor = 0;
                }
            }
        }
        HitAction::ModelProviderRefresh(provider_id) => {
            crate::keybindings::sync_models_tui(app);
            if let Some(index) = app
                .models
                .iter()
                .position(|model| model.provider_id == provider_id)
            {
                app.selected_model = index;
            }
        }
        HitAction::ProviderApiKey(index) => {
            let providers = navi_sdk::provider_catalog(&app.loaded_config.config);
            if let Some(provider) = providers.get(index) {
                app.selected_provider_setting = index;
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                replace_modal(app, crate::state::ModalKind::ApiKeyEntry);
            }
        }
        HitAction::ProviderOAuth(index) => {
            let providers = navi_sdk::provider_catalog(&app.loaded_config.config);
            if let Some(provider) = providers.get(index) {
                app.selected_provider_setting = index;
                if provider_supports_oauth(&provider.id) {
                    crate::providers::start_provider_oauth(app, provider);
                }
            }
        }
        HitAction::OAuthOpen => {
            if let Some(uri) = app
                .oauth_state
                .as_ref()
                .map(|state| state.verification_uri.clone())
            {
                crate::browser::open_url(app, uri);
            }
        }
        HitAction::Session(index) => {
            if let Some(snapshot) = app.saved_sessions.get(index).cloned() {
                app.selected_session = index;
                crate::persistence::save_current_session(app);
                crate::persistence::load_session(app, &snapshot);
                crate::keybindings::close_all_modals(app);
            }
        }
        HitAction::Skill(index) => {
            let skills = app.filtered_skills();
            if let Some(skill) = skills.get(index) {
                let skill_id = skill.id.clone();
                app.selected_skill = index;
                app.toggle_skill(&skill_id);
            }
        }
        HitAction::Setting(index) => {
            app.selected_setting = index;
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::PluginInstallOrUpdate(index) => {
            let rows = plugin_picker_rows(app);
            if let Some(row) = rows.get(index) {
                app.selected_plugin_row = index;
                match row {
                    crate::plugins::PluginPickerRow::Catalog(entry) => {
                        let installed = crate::plugins::list_installed_plugin_ids(app)
                            .iter()
                            .any(|id| id == &entry.id);
                        install_or_update_from_marketplace(app, &entry.id, installed);
                    }
                    crate::plugins::PluginPickerRow::Installed { id, .. } => {
                        install_or_update_from_marketplace(app, id, true);
                    }
                }
            }
        }
        HitAction::PluginRefresh => crate::plugins::refresh_plugin_catalog(app),
        HitAction::ToolApprove => crate::tools::approve_pending_tool(app),
        HitAction::ToolDeny => crate::tools::deny_pending_tool(app),
        HitAction::PluginApprove => {
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Char('y'),
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::PluginDeny => {
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Char('n'),
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::ThemePicker => {
            app.theme_filter.clear();
            replace_modal(app, crate::state::ModalKind::ThemePicker);
        }
        HitAction::ThemeSelect(index) => {
            if let Some(theme) = crate::theme::ThemeId::ALL.get(index) {
                app.selected_theme = index;
                app.set_theme(*theme);
            }
        }
        HitAction::ChatMessage(index) => {
            if app
                .messages
                .get(index)
                .is_some_and(|message| message.role == crate::state::ChatRole::User)
            {
                app.message_action_target = Some(index);
                app.selected_message_action = 0;
                replace_modal(app, crate::state::ModalKind::MessageActions);
            }
        }
        HitAction::ToolResult(id) => {
            toggle_tool_result(app, &id);
        }
        HitAction::ToolGroup(ids) => {
            let expand = ids.iter().any(|id| !app.expanded_tool_results.contains(id));
            for id in ids {
                if expand {
                    app.expanded_tool_results.insert(id);
                } else {
                    app.expanded_tool_results.remove(&id);
                }
            }
            app.chat_render_cache.borrow_mut().signature_hash = 0;
        }
        HitAction::Subagent(id) => {
            app.open_subagent_view(id);
        }
        HitAction::MessageAction(index) => {
            run_message_action(app, index);
        }
        HitAction::ScrollTo { target, offset } => scroll_to(app, target, offset),
        HitAction::McpServer(index) => {
            app.mcp_ui_state.selected_server = index;
            app.mcp_ui_state.is_focused_on_tools = false;
        }
        HitAction::McpTool(index) => {
            app.mcp_ui_state.selected_tool = index;
            app.mcp_ui_state.is_focused_on_tools = true;
        }
        HitAction::RemoveImage(index) => {
            if index < app.pending_images.len() {
                app.pending_images.remove(index);
            }
        }
    }
}

fn toggle_tool_result(app: &mut TuiApp, id: &str) {
    if !app.expanded_tool_results.remove(id) {
        app.expanded_tool_results.insert(id.to_string());
    }
    app.chat_render_cache.borrow_mut().signature_hash = 0;
}

pub(crate) fn run_message_action(app: &mut TuiApp, index: usize) {
    let Some(action) = crate::state::MessageAction::ALL.get(index).copied() else {
        return;
    };
    let Some(message_index) = app.message_action_target else {
        close_active_modal(app);
        return;
    };

    match action {
        crate::state::MessageAction::Copy => {
            if let Some(text) = app
                .messages
                .get(message_index)
                .map(|message| message.content.clone())
            {
                copy_text_to_clipboard(app, &text);
            }
            close_active_modal(app);
        }
        crate::state::MessageAction::Revert => {
            match revert_to_user_message(app, message_index) {
                Ok(()) => show_notification(app, "Message", "Reverted to selected message."),
                Err(err) => push_diagnostic(app, err),
            }
            close_active_modal(app);
        }
        crate::state::MessageAction::Fork => {
            match fork_from_user_message(app, message_index) {
                Ok(()) => show_notification(app, "Message", "Forked into a new session."),
                Err(err) => push_diagnostic(app, err),
            }
            close_active_modal(app);
        }
    }
}

fn scroll_active_list(app: &mut TuiApp, delta: isize) -> bool {
    let Some(target) = active_scroll_target(app) else {
        return false;
    };
    scroll_by(app, target, delta);
    true
}

fn active_scroll_target(app: &TuiApp) -> Option<ScrollTarget> {
    match app.mode {
        Mode::Commands => Some(ScrollTarget::Commands),
        Mode::Models => Some(ScrollTarget::Models),
        Mode::Providers => Some(ScrollTarget::Providers),
        Mode::Sessions => Some(ScrollTarget::Sessions),
        Mode::Skills => Some(ScrollTarget::Skills),
        Mode::Plugins => Some(ScrollTarget::Plugins),
        Mode::PluginApproval => Some(ScrollTarget::PluginApproval),
        Mode::Question => Some(ScrollTarget::QuestionOptions),
        Mode::Settings
        | Mode::ThemePicker
        | Mode::Thinking
        | Mode::Help
        | Mode::Debug
        | Mode::MessageActions
        | Mode::OAuth => None,
        Mode::Normal
        | Mode::ApiKeyEntry
        | Mode::Mcp
        | Mode::BackgroundCommands
        | Mode::BackgroundModels
        | Mode::BgModelPicker => None,
        Mode::Setup => None,
    }
}

fn scroll_by(app: &mut TuiApp, target: ScrollTarget, delta: isize) {
    match target {
        ScrollTarget::Commands => {
            let len = filtered_commands(app).len();
            app.command_scroll = shifted_scroll(app.command_scroll, len, 10, delta);
            app.selected_command = app
                .selected_command
                .clamp(app.command_scroll, app.command_scroll + 9);
        }
        ScrollTarget::Models => scroll_models_by(app, delta),
        ScrollTarget::Providers => {
            let len = navi_sdk::provider_catalog(&app.loaded_config.config).len();
            let (selected, scroll) = shifted_select_state(
                app.selected_provider_setting,
                app.provider_settings_scroll,
                len,
                delta,
                12,
            );
            app.selected_provider_setting = selected;
            app.provider_settings_scroll = scroll;
        }
        ScrollTarget::Sessions => {
            let (selected, scroll) = shifted_select_state(
                app.selected_session,
                app.session_scroll,
                app.saved_sessions.len(),
                delta,
                10,
            );
            app.selected_session = selected;
            app.session_scroll = scroll;
        }
        ScrollTarget::Skills => {
            let len = app.filtered_skills().len();
            let (selected, scroll) =
                shifted_select_state(app.selected_skill, app.skill_scroll, len, delta, 14);
            app.selected_skill = selected;
            app.skill_scroll = scroll;
        }
        ScrollTarget::Plugins => {
            let len = plugin_picker_rows(app).len();
            let (selected, scroll) = shifted_select_state(
                app.selected_plugin_row,
                app.plugin_row_scroll,
                len,
                delta,
                14,
            );
            app.selected_plugin_row = selected;
            app.plugin_row_scroll = scroll;
        }
        ScrollTarget::PluginApproval => {
            if delta.is_positive() {
                app.plugin_approval_scroll =
                    app.plugin_approval_scroll.saturating_add(delta as usize);
            } else {
                app.plugin_approval_scroll = app
                    .plugin_approval_scroll
                    .saturating_sub(delta.unsigned_abs());
            }
        }
        ScrollTarget::QuestionOptions => {
            if let Some(question) = app.pending_questions.first_mut() {
                let len = question.request.options.len();
                question.option_scroll = shifted_scroll(question.option_scroll, len, 8, delta);
                question.selected_row = question
                    .selected_row
                    .clamp(question.option_scroll, question.option_scroll + 7);
            }
        }
    }
}

fn scroll_to(app: &mut TuiApp, target: ScrollTarget, offset: usize) {
    match target {
        ScrollTarget::Commands => {
            let len = filtered_commands(app).len();
            app.command_scroll = offset.min(len.saturating_sub(10));
            app.selected_command = app
                .selected_command
                .clamp(app.command_scroll, app.command_scroll + 9);
        }
        ScrollTarget::Models => scroll_models_to(app, offset),
        ScrollTarget::Providers => {
            let len = navi_sdk::provider_catalog(&app.loaded_config.config).len();
            app.selected_provider_setting = offset.min(len.saturating_sub(1));
            app.provider_settings_scroll = app.selected_provider_setting;
        }
        ScrollTarget::Sessions => {
            app.selected_session = offset.min(app.saved_sessions.len().saturating_sub(1));
            app.session_scroll = app.selected_session;
        }
        ScrollTarget::Skills => {
            let len = app.filtered_skills().len();
            app.selected_skill = offset.min(len.saturating_sub(1));
            app.skill_scroll = app.selected_skill;
        }
        ScrollTarget::Plugins => {
            let len = plugin_picker_rows(app).len();
            app.selected_plugin_row = offset.min(len.saturating_sub(1));
            app.plugin_row_scroll = app.selected_plugin_row;
        }
        ScrollTarget::PluginApproval => app.plugin_approval_scroll = offset,
        ScrollTarget::QuestionOptions => {
            if let Some(question) = app.pending_questions.first_mut() {
                let len = question.request.options.len();
                question.option_scroll = offset.min(len.saturating_sub(8));
                question.selected_row = question
                    .selected_row
                    .clamp(question.option_scroll, question.option_scroll + 7);
            }
        }
    }
}

fn shifted_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta.is_positive() {
        current.saturating_add(delta as usize).min(len - 1)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

fn shifted_scroll(current: usize, len: usize, visible_rows: usize, delta: isize) -> usize {
    let max_scroll = len.saturating_sub(visible_rows);
    if delta.is_positive() {
        current.saturating_add(delta as usize).min(max_scroll)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

fn shifted_select_state(
    selected: usize,
    scroll: usize,
    len: usize,
    delta: isize,
    visible_rows: usize,
) -> (usize, usize) {
    let mut state = SelectListState::new(selected, scroll);
    if delta.is_positive() {
        state.page_next(len, delta as usize);
    } else {
        state.page_previous(delta.unsigned_abs());
    }
    state.sync_scroll(visible_rows);
    state.clamp_scroll(len, visible_rows);
    (state.selected(), state.scroll())
}

fn scroll_models_by(app: &mut TuiApp, delta: isize) {
    let rows = build_model_rows(app);
    if rows.is_empty() {
        return;
    }
    let current = selected_model_in_rows(&rows, app.selected_model).unwrap_or(0);
    let target = shifted_index(current, rows.len(), delta);
    select_model_near_row(app, &rows, target, delta.is_positive());
    sync_scroll_to_selection(app, &rows, 14);
}

fn scroll_models_to(app: &mut TuiApp, offset: usize) {
    let rows = build_model_rows(app);
    if rows.is_empty() {
        return;
    }
    let target = offset.min(rows.len().saturating_sub(1));
    app.model_scroll = target;
    select_model_near_row(app, &rows, target, true);
}

fn select_model_near_row(app: &mut TuiApp, rows: &[ListRow], row: usize, prefer_after: bool) {
    let selected = if prefer_after {
        rows.iter().skip(row).find_map(model_row_index).or_else(|| {
            rows.iter()
                .take(row.saturating_add(1))
                .rev()
                .find_map(model_row_index)
        })
    } else {
        rows.iter()
            .take(row.saturating_add(1))
            .rev()
            .find_map(model_row_index)
            .or_else(|| rows.iter().skip(row).find_map(model_row_index))
    };
    app.selected_model = selected
        .or_else(|| first_model_index(rows))
        .unwrap_or(app.selected_model);
}

fn model_row_index(row: &ListRow) -> Option<usize> {
    match row {
        ListRow::Model { index } => Some(*index),
        ListRow::Header { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crossterm::event::{KeyModifiers, MouseEvent};
    use navi_sdk::{AgentEvent, QuestionOption, QuestionRequest, QuestionResponse};
    use ratatui::layout::Rect;

    use super::*;
    use crate::dispatch::{AsyncEvent, handle_async_event};
    use crate::state::{Mode, QuestionUiState};
    use crate::testing::{EngineCall, MockEngine};
    use crate::tests::test_app;

    fn question_request(multiple: bool) -> QuestionRequest {
        QuestionRequest {
            id: "question-1".to_string(),
            question: "Which direction should NAVI take?".to_string(),
            options: vec![
                QuestionOption {
                    label: "Fast".to_string(),
                    description: None,
                },
                QuestionOption {
                    label: "Thorough".to_string(),
                    description: None,
                },
            ],
            multiple,
            allow_custom: true,
        }
    }

    fn mouse_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_moved(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_scroll_down(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn open_question(app: &mut TuiApp, request: QuestionRequest) {
        handle_async_event(
            app,
            AsyncEvent::Agent(AgentEvent::QuestionRequested(request)),
        );
    }

    async fn wait_for_question_resolution(engine: &MockEngine) {
        for _ in 0..50 {
            if engine
                .calls()
                .iter()
                .any(|call| matches!(call, EngineCall::ResolveQuestion { .. }))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    #[test]
    fn mouse_down_on_pending_question_hit_reopens_question_modal() {
        let mut app = test_app("");
        app.pending_questions
            .push(QuestionUiState::new(question_request(false)));
        app.mode = Mode::Normal;
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            10,
            "pending question",
            HitAction::ReopenQuestion,
        );

        handle_mouse(&mut app, mouse_down(4, 3));

        assert_eq!(app.mode, Mode::Question);
    }

    #[test]
    fn mouse_down_on_close_modal_hit_closes_active_modal() {
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Help);
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            10,
            "close modal",
            HitAction::CloseModal,
        );

        handle_mouse(&mut app, mouse_down(4, 3));

        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn mouse_wheel_scrolls_active_modal_list_instead_of_chat() {
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Commands);
        app.scroll_offset = 12;
        app.selected_command = 0;
        app.command_scroll = 0;

        handle_mouse(&mut app, mouse_scroll_down(4, 3));

        assert!(app.selected_command > 0);
        assert!(app.command_scroll > 0);
        assert_eq!(app.scroll_offset, 12);
    }

    #[test]
    fn mouse_move_after_wheel_does_not_restore_list_scroll_to_hovered_item() {
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Commands);
        app.selected_command = 0;
        app.command_scroll = 0;
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "command first row",
            HitAction::Command(0),
        );

        handle_mouse(&mut app, mouse_scroll_down(4, 3));
        let scrolled = app.command_scroll;
        let selected_after_scroll = app.selected_command;
        handle_mouse(&mut app, mouse_moved(4, 3));

        assert!(scrolled > 0);
        assert_eq!(app.command_scroll, scrolled);
        assert_eq!(app.selected_command, selected_after_scroll);
    }

    #[test]
    fn mouse_down_on_scrollbar_moves_target_list() {
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Providers);
        let target_offset = 2.min(
            navi_sdk::provider_catalog(&app.loaded_config.config)
                .len()
                .saturating_sub(1),
        );
        app.register_hit(
            Rect::new(2, 3, 1, 1),
            80,
            "provider scrollbar",
            HitAction::ScrollTo {
                target: ScrollTarget::Providers,
                offset: target_offset,
            },
        );

        handle_mouse(&mut app, mouse_down(2, 3));

        assert_eq!(app.selected_provider_setting, target_offset);
        assert_eq!(app.provider_settings_scroll, target_offset);
    }

    #[test]
    fn mouse_move_on_question_option_does_not_update_selection() {
        let mut app = test_app("");
        open_question(&mut app, question_request(false));
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "question option",
            HitAction::QuestionOption(1),
        );

        handle_mouse(&mut app, mouse_moved(4, 3));

        assert_eq!(app.pending_questions[0].selected_row, 0);
    }

    #[test]
    fn mouse_down_on_question_option_updates_selection() {
        let mut app = test_app("");
        open_question(&mut app, question_request(false));
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "question option",
            HitAction::QuestionOption(1),
        );

        handle_mouse(&mut app, mouse_down(4, 3));

        assert_eq!(app.pending_questions[0].selected_row, 1);
        assert!(app.selection.is_none());
    }

    #[test]
    fn mouse_down_on_multi_question_option_toggles_option() {
        let mut app = test_app("");
        open_question(&mut app, question_request(true));
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "question option",
            HitAction::QuestionOption(1),
        );

        handle_mouse(&mut app, mouse_down(4, 3));

        assert_eq!(app.pending_questions[0].selected_row, 1);
        assert!(app.pending_questions[0].selected_options[1]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mouse_down_on_question_send_resolves_selected_answer() {
        let mut app = test_app("");
        let engine = Arc::new(MockEngine::new());
        app.set_engine(engine.clone());
        open_question(&mut app, question_request(false));
        let session_id = app.session_id.as_str().to_string();
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "question option",
            HitAction::QuestionOption(1),
        );
        app.register_hit(
            Rect::new(2, 5, 12, 1),
            20,
            "question send",
            HitAction::QuestionSend,
        );

        handle_mouse(&mut app, mouse_down(4, 3));
        handle_mouse(&mut app, mouse_down(4, 5));
        wait_for_question_resolution(&engine).await;

        let calls = engine.calls();
        assert!(
            calls.iter().any(|call| matches!(
                call,
                EngineCall::ResolveQuestion {
                    session_id: resolved_session_id,
                    response: QuestionResponse::Answered { id, answers },
                } if resolved_session_id == &session_id
                    && id == "question-1"
                    && answers == &vec!["Thorough".to_string()]
            )),
            "calls: {calls:?}"
        );
    }

    #[test]
    fn mouse_up_on_hit_region_does_not_dispatch_action() {
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Help);
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            10,
            "close modal",
            HitAction::CloseModal,
        );

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 4,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
        );

        assert_eq!(app.mode, Mode::Help);
    }
}
