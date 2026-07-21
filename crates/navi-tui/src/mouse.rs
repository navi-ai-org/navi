use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::TuiApp;
use crate::chat::{fork_from_user_message, revert_to_user_message};

use crate::keybindings::{close_active_modal, handle_key, replace_modal};
use crate::notifications::{push_diagnostic, show_notification};
use crate::plugins::{install_or_update_from_marketplace, plugin_picker_rows};
use crate::providers::{
    ListRow, apply_model_selection, build_model_rows, first_model_index, selected_model_in_rows,
};
use crate::render::text::display_width;
use crate::runtime::provider_supports_oauth;
use crate::state::{Mode, SelectionState};
use crate::ui::SelectListState;
use crate::ui::interaction::{HitAction, HitRegion, ScrollTarget};

/// A wheel notch moves a small number of rendered chat lines. Keeping this
/// line-based (rather than reusing keyboard block navigation) preserves smooth
/// scrollback even when the composer is empty.
const CHAT_WHEEL_LINES: usize = 2;

fn map_mouse_to_text(app: &TuiApp, col: u16, row: u16) -> Option<(usize, usize)> {
    map_mouse_to_text_with_clamp(app, col, row, false)
}

fn map_mouse_to_text_clamped(app: &TuiApp, col: u16, row: u16) -> Option<(usize, usize)> {
    map_mouse_to_text_with_clamp(app, col, row, true)
}

fn map_mouse_to_text_with_clamp(
    app: &TuiApp,
    col: u16,
    row: u16,
    clamp_to_chat: bool,
) -> Option<(usize, usize)> {
    let cache = app.chat_render_cache.borrow();
    let inner = cache.chat_rect?;
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    if !clamp_to_chat
        && (col < inner.x
            || col >= inner.x + inner.width
            || row < inner.y
            || row >= inner.y + inner.height)
    {
        return None;
    }
    let clamped_col = col.clamp(inner.x, inner.x + inner.width.saturating_sub(1));
    let clamped_row = row.clamp(inner.y, inner.y + inner.height.saturating_sub(1));
    let visible_y = (clamped_row - inner.y) as usize;

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

    let char_index = (clamped_col - inner.x) as usize;
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

            let substr = slice_display_columns(&line_text, start_char, end_char);
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

/// Hits that belong to a selectable chat block (message / tool / subagent).
fn is_chat_block_hit(action: &HitAction) -> bool {
    matches!(
        action,
        HitAction::ChatMessage(_)
            | HitAction::ToolResult(_)
            | HitAction::ToolGroup(_)
            | HitAction::Subagent(_)
            // PreviewChatImage is handled as a lightbox open/toggle, not as
            // drag-select chat block chrome (see image click path below).
            | HitAction::MessageAction(_)
    )
}

/// Second-click actions on an already-selected chat block (expand tool, menu, …).
/// Kept separate from [`dispatch_hit`] so first-click can select without firing them.
fn run_secondary_chat_click(app: &mut TuiApp, action: &HitAction) {
    match action {
        HitAction::ChatMessage(index) => {
            if app
                .messages
                .get(*index)
                .is_some_and(|message| message.role == crate::state::ChatRole::User)
            {
                open_message_actions(app, *index);
            }
        }
        HitAction::ToolResult(id) => {
            toggle_tool_result(app, id);
        }
        HitAction::ToolGroup(ids) => {
            let expand = ids.iter().any(|id| !app.expanded_tool_results.contains(id));
            for id in ids {
                if expand {
                    app.expanded_tool_results.insert(id.clone());
                } else {
                    app.expanded_tool_results.remove(id);
                }
            }
            app.chat_render_cache.borrow_mut().signature_hash = 0;
        }
        HitAction::Subagent(id) => {
            app.open_subagent_view(id.clone());
        }
        _ => {}
    }
}

/// Handle a mouse event. Returns `true` when the UI needs a redraw.
pub(crate) fn handle_mouse(app: &mut TuiApp, mouse: MouseEvent) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            if scroll_active_list(app, 3) {
                return true;
            }
            scroll_chat_with_wheel(app, -(CHAT_WHEEL_LINES as isize), mouse.column, mouse.row);
            true
        }
        MouseEventKind::ScrollUp => {
            if scroll_active_list(app, -3) {
                return true;
            }
            scroll_chat_with_wheel(app, CHAT_WHEEL_LINES as isize, mouse.column, mouse.row);
            true
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.pending_chat_click = None;

            if let Some(hit) = app.hit_test(mouse.column, mouse.row) {
                if matches!(hit.action, HitAction::ScrollTo { .. }) {
                    dispatch_hit(app, hit);
                    return true;
                }

                // Composer click must win early: restore input focus before any
                // chat/selection path can re-select a scrollback block.
                if matches!(hit.action, HitAction::FocusComposer) {
                    if app.image_hover.is_some() {
                        crate::view::image_preview::clear_image_hover(app);
                    }
                    app.hover_index = None;
                    app.hovered_chat_source = None;
                    app.selection = None;
                    app.pending_chat_click = None;
                    crate::chat_blocks::clear_selected_block(app);
                    dispatch_hit(app, hit);
                    return true;
                }

                // Chip: open (hover is primary; click is a fallback without motion).
                // Do not toggle-close while still on the chip — that fights hover UX.
                if crate::view::image_preview::is_image_chip_action(&hit.action) {
                    let _ = crate::view::image_preview::set_hover_from_action(app, &hit.action);
                    return true;
                }

                // Cursor on the lightbox body: keep it open (don't dismiss).
                if matches!(hit.action, HitAction::ImageLightboxKeep) {
                    crate::view::image_preview::keep_image_hover(app);
                    return false;
                }

                // Click outside sticky zones dismisses immediately.
                if app.image_hover.is_some() {
                    crate::view::image_preview::clear_image_hover(app);
                }

                // Chat lines always register hit regions. If we dispatch + return
                // here, drag-to-select text can never start. Instead: select the
                // block, start a text selection, and defer click actions (expand
                // tool / message menu) until mouse-up if it was a click not a drag.
                if app.mode == Mode::Normal && is_chat_block_hit(&hit.action) {
                    app.hover_index = None;
                    app.hovered_chat_source = chat_source_for_action(&hit.action);
                    // Remember if this was already the selected block (for 2nd-click actions).
                    let already_selected = app
                        .hovered_chat_source
                        .as_ref()
                        .zip(app.selected_chat_source.as_ref())
                        .is_some_and(|(a, b)| crate::chat_blocks::chat_sources_match(a, b));
                    // Map the click first — before any scroll — so the anchor
                    // stays under the cursor.
                    if let Some(pos) = map_mouse_to_text(app, mouse.column, mouse.row) {
                        let bound = app
                            .hovered_chat_source
                            .clone()
                            .or_else(|| crate::chat_blocks::source_at_line(app, pos.0));
                        if let Some(source) = bound.clone() {
                            // No auto-scroll: scrolling would remapp line indices.
                            crate::chat_blocks::select_chat_block_no_scroll(app, source);
                        }
                        begin_text_selection(app, pos, bound);
                        // Defer 2nd-click actions until mouse-up so a drag can copy text.
                        if already_selected {
                            app.pending_chat_click = Some(hit.action);
                        }
                    } else if already_selected {
                        // No chat_rect (tests / unmapped): can't drag-select — run 2nd-click now.
                        if let Some(source) = app.hovered_chat_source.clone() {
                            crate::chat_blocks::select_chat_block_no_scroll(app, source);
                        }
                        run_secondary_chat_click(app, &hit.action);
                    } else if let Some(source) = app.hovered_chat_source.clone() {
                        crate::chat_blocks::select_chat_block_no_scroll(app, source);
                    }
                    return true;
                }

                app.hover_index = None;
                app.hovered_chat_source = chat_source_for_action(&hit.action);
                // Clicking outside a chat block clears block selection (composer,
                // chrome, empty chrome hits, etc.).
                if !is_chat_block_hit(&hit.action) {
                    crate::chat_blocks::clear_selected_block(app);
                    app.selection = None;
                }
                dispatch_hit(app, hit);
                return true;
            }
            // Empty space click closes lightbox immediately.
            if app.image_hover.is_some() {
                crate::view::image_preview::clear_image_hover(app);
            }
            app.hover_index = None;
            app.hovered_chat_source = None;
            if let Some(pos) = map_mouse_to_text(app, mouse.column, mouse.row) {
                let bound = crate::chat_blocks::source_at_line(app, pos.0);
                // Starting a text drag also selects that block (entry focus).
                if let Some(source) = bound.clone() {
                    crate::chat_blocks::select_chat_block_no_scroll(app, source);
                } else {
                    // Empty gap inside the chat viewport — deselect.
                    crate::chat_blocks::clear_selected_block(app);
                }
                begin_text_selection(app, pos, bound);
            } else {
                // Click completely outside chat text — clear block + drag selection.
                app.selection = None;
                crate::chat_blocks::clear_selected_block(app);
            }
            true
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            // During an active text drag, never let list scrollbars / other hits
            // steal the gesture and wipe the selection.
            if app.selection.as_ref().is_some_and(|s| s.active) {
                return update_active_text_drag(app, mouse.column, mouse.row);
            }
            if let Some(hit) = app.hit_test(mouse.column, mouse.row)
                && matches!(hit.action, HitAction::ScrollTo { .. })
            {
                dispatch_hit(app, hit);
                return true;
            }
            true
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app
                .selection
                .as_ref()
                .is_some_and(|selection| selection.active)
            {
                // Final endpoint from release coords (works even if the terminal
                // never sent Drag events — only Down + Up).
                let pos = map_mouse_to_text_clamped(app, mouse.column, mouse.row);
                let was_drag = app.selection.as_ref().is_some_and(|s| {
                    let end = pos.unwrap_or(s.end);
                    s.start != end
                });
                if finish_selection(app, pos)
                    && was_drag
                    && let Some(text) = selected_text(app)
                {
                    copy_text_to_clipboard(app, &text);
                    app.pending_chat_click = None;
                } else if !was_drag {
                    // Pure click on a chat block: run deferred 2nd-click action
                    // (expand tool / user message menu / open subagent).
                    if let Some(action) = app.pending_chat_click.take() {
                        run_secondary_chat_click(app, &action);
                    }
                    // Clear zero-width selection so the highlight doesn't linger.
                    app.selection = None;
                }
                return true;
            }
            app.pending_chat_click = None;
            if app.hit_test(mouse.column, mouse.row).is_some() {
                app.selection = None;
                return true;
            }
            let pos = map_mouse_to_text(app, mouse.column, mouse.row);
            if finish_selection(app, pos) {
                if let Some(text) = selected_text(app) {
                    copy_text_to_clipboard(app, &text);
                }
            }
            true
        }
        MouseEventKind::Moved => {
            // Some terminals emit Moved (not Drag) while the button is held.
            // Keep the active text selection alive by treating it as a drag.
            if app.selection.as_ref().is_some_and(|s| s.active) {
                return update_active_text_drag(app, mouse.column, mouse.row);
            }
            handle_mouse_moved(app, mouse.column, mouse.row)
        }
        _ => false,
    }
}

fn begin_text_selection(
    app: &mut TuiApp,
    pos: (usize, usize),
    bound: Option<crate::state::ChatLineSource>,
) {
    // Free-form selection across the chat viewport. We still record
    // `bound_source` for block chrome, but drag endpoints are NOT clamped to
    // that block — clamping made multi-line / multi-cell drags look broken
    // (endpoint frozen at the first/last line of the block).
    let _ = bound;
    app.selection = Some(SelectionState {
        start: pos,
        end: pos,
        active: true,
        bound_source: None,
    });
}

/// Extend an in-progress text selection to the line under the cursor, with
/// edge auto-scroll when the pointer is at the chat viewport boundary.
fn update_active_text_drag(app: &mut TuiApp, col: u16, row: u16) -> bool {
    if !app.selection.as_ref().is_some_and(|s| s.active) {
        return false;
    }
    if let Some(inner) = app.chat_render_cache.borrow().chat_rect {
        if row <= inner.y {
            app.scroll_offset = app.scroll_offset.saturating_add(CHAT_WHEEL_LINES);
        } else if row >= inner.y + inner.height.saturating_sub(1) {
            app.scroll_offset = app.scroll_offset.saturating_sub(CHAT_WHEEL_LINES);
        }
    }
    if let Some(pos) = map_mouse_to_text_clamped(app, col, row) {
        if let Some(selection) = &mut app.selection {
            selection.end = pos;
        }
    }
    true
}

fn slice_display_columns(text: &str, start_col: usize, end_col: usize) -> String {
    if start_col >= end_col {
        return String::new();
    }
    let mut out = String::new();
    let mut col = 0usize;
    for ch in text.chars() {
        let width = display_width(&ch.to_string()).max(1);
        let next_col = col.saturating_add(width);
        if next_col > start_col && col < end_col {
            out.push(ch);
        }
        if col >= end_col {
            break;
        }
        col = next_col;
    }
    out
}

/// Free-motion hover: open on `[Image N]`, keep on lightbox body, grace-close on leave.
fn handle_mouse_moved(app: &mut TuiApp, col: u16, row: u16) -> bool {
    let drag_active = app.selection.as_ref().is_some_and(|s| s.active);
    let hit = app.hit_test(col, row);

    let mut needs_redraw = false;

    // Image sticky zone (chip / lightbox). Skip opening while text-dragging.
    if !drag_active && !app.modal_stack.is_active() {
        needs_redraw |= update_image_hover_on_move(app, hit.as_ref());
    } else if app.image_hover.is_some() && drag_active {
        // Dragging text: don't open new images; leave-close is OK if they leave.
        if !hit
            .as_ref()
            .is_some_and(|h| image_hover_sticky_action(&h.action))
        {
            let _ = crate::view::image_preview::schedule_image_hover_close(app);
        }
    }

    // Non-image hover chrome (list rows, etc.).
    if let Some(hit) = hit.as_ref() {
        needs_redraw |= apply_non_image_hover(app, hit);
    } else {
        if app.hover_index.take().is_some()
            || app.hovered_chat_source.take().is_some()
            || app.hover_context_usage
            || app.hover_plan_more
            || app.hover_queued_messages
        {
            app.hover_context_usage = false;
            app.hover_plan_more = false;
            app.hover_queued_messages = false;
            needs_redraw = true;
        }
    }

    needs_redraw
}

fn image_hover_sticky_action(action: &HitAction) -> bool {
    matches!(
        action,
        HitAction::PreviewPendingImage(_)
            | HitAction::PreviewChatImage { .. }
            | HitAction::ImageLightboxKeep
    )
}

/// Returns true when the image lightbox open/identity state changed.
fn update_image_hover_on_move(app: &mut TuiApp, hit: Option<&HitRegion<HitAction>>) -> bool {
    match hit.map(|h| &h.action) {
        Some(action) if crate::view::image_preview::is_image_chip_action(action) => {
            crate::view::image_preview::set_hover_from_action(app, action)
        }
        Some(HitAction::ImageLightboxKeep) => {
            crate::view::image_preview::keep_image_hover(app);
            false
        }
        _ => {
            // Left sticky zone — grace close (not immediate) so chip→modal travel works.
            let _ = crate::view::image_preview::schedule_image_hover_close(app);
            false
        }
    }
}

/// List/modal hover highlighting. Does **not** clear the image lightbox
/// (that is handled only by sticky-zone leave + grace / Esc / click outside).
fn apply_non_image_hover(app: &mut TuiApp, hit: &HitRegion<HitAction>) -> bool {
    if image_hover_sticky_action(&hit.action) {
        // Image chip / lightbox: chat source hover is irrelevant; avoid churn.
        app.hover_index = None;
        app.hover_context_usage = false;
        app.hover_plan_more = false;
        app.hover_queued_messages = false;
        return false;
    }

    let prev_source = app.hovered_chat_source.clone();
    let prev_index = app.hover_index;
    let prev_usage = app.hover_context_usage;
    let prev_plan_more = app.hover_plan_more;
    let prev_queued = app.hover_queued_messages;

    app.hovered_chat_source = chat_source_for_action(&hit.action);
    app.hover_context_usage = matches!(hit.action, HitAction::ContextUsage);
    app.hover_plan_more = matches!(hit.action, HitAction::ExpandPlanMore);
    app.hover_queued_messages = matches!(hit.action, HitAction::OpenMessageQueue);
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
        HitAction::ExtensionsItem(index) => app.hover_index = Some(*index),
        HitAction::MessageAction(index) => app.hover_index = Some(*index),
        HitAction::RewindCheckpoint(index) => app.hover_index = Some(*index),
        HitAction::PluginInstallOrUpdate(index) => {
            app.hover_index = Some(*index);
        }
        HitAction::ThemeSelect(index) => app.hover_index = Some(*index),
        HitAction::ContextUsage | HitAction::ExpandPlanMore => {
            app.hover_index = None;
        }
        _ => {
            app.hover_index = None;
        }
    }

    prev_source != app.hovered_chat_source
        || prev_index != app.hover_index
        || prev_usage != app.hover_context_usage
        || prev_plan_more != app.hover_plan_more
        || prev_queued != app.hover_queued_messages
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

/// Wheel scrolling is viewport navigation, never block navigation. In
/// particular, a previous click/drag must not leave a scrollback block focused
/// while the user scrolls the chat: that made a normal wheel gesture look like
/// selection hopping and kept the composer collapsed.
///
/// Exception: an *active* text drag must survive the wheel so users can select
/// across more than one viewport page (select + scroll).
fn clear_chat_selection_for_wheel(app: &mut TuiApp) {
    app.pending_chat_click = None;
    app.selection = None;
    crate::chat_blocks::clear_selected_block(app);
}

/// Scroll chat by `delta_lines` (+ = older / up, − = newer / down).
///
/// When a text selection drag is active, keep it and re-map the endpoint under
/// the cursor after the viewport moves. Otherwise clear idle selection/block
/// focus so wheel never hops chat cells.
fn scroll_chat_with_wheel(app: &mut TuiApp, delta_lines: isize, col: u16, row: u16) {
    let active_drag = app.selection.as_ref().is_some_and(|s| s.active);
    if active_drag {
        app.pending_chat_click = None;
    } else {
        clear_chat_selection_for_wheel(app);
    }

    if delta_lines >= 0 {
        app.scroll_offset = app.scroll_offset.saturating_add(delta_lines as usize);
    } else {
        app.scroll_offset = app
            .scroll_offset
            .saturating_sub(delta_lines.unsigned_abs() as usize);
    }

    if !active_drag {
        return;
    }

    // Remap selection end to the line now under the cursor (clamped to the
    // bound chat block so selection cannot bleed across messages).
    if let Some(pos) = map_mouse_to_text_clamped(app, col, row) {
        let bound = app.selection.as_ref().and_then(|s| s.bound_source.clone());
        let clamped_line = crate::chat_blocks::clamp_line_to_block(app, pos.0, &bound);
        if let Some(selection) = &mut app.selection {
            selection.end = (clamped_line, pos.1);
        }
    }
}

fn dispatch_hit(app: &mut TuiApp, hit: HitRegion<HitAction>) {
    match hit.action {
        HitAction::Key { code, modifiers } => {
            let _ = handle_key(app, code, modifiers);
        }
        HitAction::CloseModal => close_active_modal(app),
        HitAction::ReopenQuestion => replace_modal(app, crate::state::ModalKind::Question),
        HitAction::OpenMessageQueue => {
            replace_modal(app, crate::state::ModalKind::MessageQueue);
        }
        HitAction::QueuedMessage(index) => {
            app.queued_message_selected =
                index.min(app.queued_user_messages.len().saturating_sub(1));
            let _ = handle_key(
                app,
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            );
        }
        HitAction::RemoveQueuedMessage(index) => {
            crate::keybindings::modals::remove_queued_message_at(app, index);
        }
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
            let rows = crate::commands::command_rows(app);
            if rows.get(index).is_some_and(|r| r.is_selectable()) {
                app.selected_command = index;
                let _ = crate::keybindings::run_selected_command(app);
            }
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
            let session_id = app
                .filtered_sessions()
                .get(index)
                .map(|info| info.id.clone());
            if let Some(session_id) = session_id {
                if let Some(snapshot) =
                    crate::session::load_session_snapshot(&app.session_store, &session_id)
                {
                    app.selected_session = index;
                    crate::persistence::save_current_session(app);
                    crate::persistence::load_session(app, &snapshot);
                    crate::keybindings::close_all_modals(app);
                }
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
        HitAction::ExtensionsItem(index) => {
            app.selected_extensions_item = index;
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
        HitAction::BackgroundCommandOpen(index) => {
            crate::background::open_background_command_output(app, index);
        }
        HitAction::BackgroundCommandCancel(index) => {
            crate::background::cancel_background_command_at(app, index);
        }
        HitAction::HelpRow(index) => {
            app.selected_help = index.min(crate::view::help::help_entry_count().saturating_sub(1));
            crate::view::help::ensure_help_visible(app);
        }
        HitAction::AboutLink(index) => {
            app.selected_about_link = index;
            crate::view::about::open_selected_link(app);
        }
        HitAction::OpenUpdateAvailable => {
            if app.available_update.is_some() {
                replace_modal(app, crate::state::ModalKind::UpdateAvailable);
            }
        }
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
            app.theme_filter_cursor = 0;
            replace_modal(app, crate::state::ModalKind::ThemePicker);
        }
        HitAction::ThemeSelect(index) => {
            if let Some(theme) = crate::theme::ThemeId::ALL.get(index) {
                app.selected_theme = index;
                app.set_theme(*theme);
            }
        }
        HitAction::ChatMessage(index) => {
            let source = crate::state::ChatLineSource::Message(index);
            let already = app
                .selected_chat_source
                .as_ref()
                .is_some_and(|s| crate::chat_blocks::chat_sources_match(s, &source));
            crate::chat_blocks::select_chat_block(app, source);
            // Second click on an already-selected user message opens actions.
            if already
                && app
                    .messages
                    .get(index)
                    .is_some_and(|message| message.role == crate::state::ChatRole::User)
            {
                open_message_actions(app, index);
            }
        }
        HitAction::ToolResult(id) => {
            let source = crate::state::ChatLineSource::ToolResult(id.clone());
            let already = app
                .selected_chat_source
                .as_ref()
                .is_some_and(|s| crate::chat_blocks::chat_sources_match(s, &source));
            crate::chat_blocks::select_chat_block(app, source);
            // Second click expands/collapses the tool block.
            if already {
                toggle_tool_result(app, &id);
            }
        }
        HitAction::ToolGroup(ids) => {
            let source = crate::state::ChatLineSource::ToolGroup(ids.clone());
            let already = app
                .selected_chat_source
                .as_ref()
                .is_some_and(|s| crate::chat_blocks::chat_sources_match(s, &source));
            crate::chat_blocks::select_chat_block(app, source);
            if already {
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
        }
        HitAction::Subagent(id) => {
            let source = crate::state::ChatLineSource::Subagent(id.clone());
            let already = app
                .selected_chat_source
                .as_ref()
                .is_some_and(|s| crate::chat_blocks::chat_sources_match(s, &source));
            crate::chat_blocks::select_chat_block(app, source);
            if already {
                app.open_subagent_view(id);
            }
        }
        HitAction::FocusComposer => {
            // Explicit restore: drop text drag + block selection + hover so
            // the next frame expands the composer and shows the input cursor.
            app.pending_chat_click = None;
            app.selection = None;
            app.hovered_chat_source = None;
            app.hover_index = None;
            crate::chat_blocks::clear_selected_block(app);
            // Ensure Normal chat view (not a subagent drill-in) so the
            // composer is eligible for focus again.
            if !matches!(app.chat_view, crate::state::ChatView::Parent) {
                app.close_subagent_view();
            }
        }
        HitAction::MessageAction(index) => {
            run_message_action(app, index);
        }
        HitAction::RewindCheckpoint(list_index) => {
            let checkpoints = crate::chat::rewind_checkpoints(app);
            if let Some((message_index, _)) = checkpoints.get(list_index) {
                run_rewind_checkpoint(app, *message_index);
            }
        }
        HitAction::ScrollTo { target, offset } => scroll_to(app, target, offset),
        HitAction::ScrollToBottom => {
            crate::view::chat::jump_to_latest(app);
        }
        HitAction::ContextUsage => {
            // Click opens the full usage modal (hover already reveals %).
            crate::usage::open_usage_modal(app);
        }
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
        // Image chips / lightbox keep are hover-primary; click open is handled
        // in the Down path. Dispatch no-ops keep accidental routes safe.
        HitAction::PreviewPendingImage(_) | HitAction::PreviewChatImage { .. } => {
            let _ = crate::view::image_preview::set_hover_from_action(app, &hit.action);
        }
        HitAction::ImageLightboxKeep => {
            crate::view::image_preview::keep_image_hover(app);
        }
        HitAction::PlanReviewLine(line) => {
            if let Some(r) = app.plan_review.as_mut() {
                // Shift-free click: set cursor. Second click on same line starts comment.
                let already = r.cursor_line == line;
                r.cursor_line = line;
                r.sel_anchor = None;
                r.clamp_cursor();
                r.ensure_cursor_visible(12);
                r.focus = crate::plan_review::PlanReviewFocus::Preview;
                if already {
                    crate::plan_review::begin_comment(app);
                }
            }
        }
        HitAction::PlanReviewApprove => crate::plan_review::approve_plan(app),
        HitAction::PlanReviewChanges => {
            if let Some(r) = app.plan_review.as_mut() {
                r.focus = crate::plan_review::PlanReviewFocus::Prompt;
            }
        }
        HitAction::PlanReviewComment => crate::plan_review::begin_comment(app),
        HitAction::PlanReviewQuit => crate::plan_review::quit_plan(app),
        HitAction::TogglePlanTopbar => {
            crate::plan_progress::toggle_plan_expanded(app);
        }
        HitAction::ExpandPlanMore => {
            crate::plan_progress::expand_plan_all_steps(app);
            app.hover_plan_more = false;
        }
    }
}

fn toggle_tool_result(app: &mut TuiApp, id: &str) {
    // Prefer policy-aware toggle when we still have the tool message.
    for msg in &app.messages {
        if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result)
            && result.invocation_id == id
        {
            crate::render::tool_policy::toggle_tool_body(
                inv,
                result,
                app.full_tool_view,
                &mut app.expanded_tool_results,
                &mut app.collapsed_tool_results,
            );
            app.chat_render_cache.borrow_mut().signature_hash = 0;
            return;
        }
    }
    // Fallback: plain expanded set toggle.
    if !app.expanded_tool_results.remove(id) {
        app.expanded_tool_results.insert(id.to_string());
        app.collapsed_tool_results.remove(id);
    } else {
        app.collapsed_tool_results.insert(id.to_string());
    }
    app.chat_render_cache.borrow_mut().signature_hash = 0;
}

/// Open Message Actions for a user message, restoring the last used choice.
pub(crate) fn open_message_actions(app: &mut TuiApp, message_index: usize) {
    app.message_action_target = Some(message_index);
    app.selected_message_action = last_message_action_index(app);
    replace_modal(app, crate::state::ModalKind::MessageActions);
}

fn last_message_action_index(app: &TuiApp) -> usize {
    let key = app.loaded_config.config.tui.last_message_action.as_str();
    crate::state::MessageAction::from_config_key(key)
        .map(|action| action.index())
        .unwrap_or(0)
        .min(crate::state::MessageAction::ALL.len().saturating_sub(1))
}

fn remember_message_action(app: &mut TuiApp, index: usize) {
    let Some(action) = crate::state::MessageAction::ALL.get(index).copied() else {
        return;
    };
    app.selected_message_action = index;
    let key = action.config_key().to_string();
    if app.loaded_config.config.tui.last_message_action == key {
        return;
    }
    app.loaded_config.config.tui.last_message_action = key;
    crate::persistence::save_preferences(app);
}

pub(crate) fn run_message_action(app: &mut TuiApp, index: usize) {
    let Some(action) = crate::state::MessageAction::ALL.get(index).copied() else {
        return;
    };
    let Some(message_index) = app.message_action_target else {
        close_active_modal(app);
        return;
    };

    // Remember the choice before closing so the next open lands on it.
    remember_message_action(app, index);

    match action {
        crate::state::MessageAction::CopyResponse => {
            crate::keybindings::copy_response_since_user_message(app, message_index);
            close_active_modal(app);
        }
        crate::state::MessageAction::Copy => {
            if let Some(text) = app
                .messages
                .get(message_index)
                .map(|message| message.content.clone())
            {
                if text.trim().is_empty() {
                    show_notification(app, "Message", "Message has no text to copy.");
                } else {
                    copy_text_to_clipboard(app, &text);
                }
            }
            close_active_modal(app);
        }
        crate::state::MessageAction::CopySession => {
            crate::keybindings::copy_session_transcript(app);
            close_active_modal(app);
        }
        crate::state::MessageAction::Revert => {
            match revert_to_user_message(app, message_index) {
                Ok(()) => show_notification(
                    app,
                    "Rewind",
                    "Restored chat and project files to this prompt.",
                ),
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
        Mode::BackgroundCommands => Some(ScrollTarget::BackgroundCommands),
        Mode::BackgroundCommandOutput => Some(ScrollTarget::BackgroundCommandOutput),
        Mode::MessageQueue => Some(ScrollTarget::MessageQueue),
        Mode::Help => Some(ScrollTarget::Help),
        Mode::PathMentions => Some(ScrollTarget::PathMentions),
        Mode::Rewind => Some(ScrollTarget::Rewind),
        Mode::Settings
        | Mode::ThemePicker
        | Mode::Thinking
        | Mode::Debug
        | Mode::MessageActions
        | Mode::OAuth
        | Mode::Usage
        | Mode::QueuedMessageEdit
        | Mode::ConfirmCancelTurn
        | Mode::ConfirmMcpMerge
        | Mode::About
        | Mode::UpdateAvailable => None,
        Mode::BackgroundModels | Mode::ModelRouting => {
            // Agents tab list (and legacy agent routes modal).
            Some(ScrollTarget::BackgroundModels)
        }
        Mode::BgModelPicker => Some(ScrollTarget::Models),
        Mode::Normal
        | Mode::ApiKeyEntry
        | Mode::Mcp
        | Mode::Extensions
        | Mode::AttachmentModels => None,
        Mode::Setup => None,
        Mode::ConfirmPlan | Mode::SudoPassword => None,
    }
}

fn scroll_by(app: &mut TuiApp, target: ScrollTarget, delta: isize) {
    match target {
        ScrollTarget::Commands => {
            // Viewport scroll first. Selection only moves when it leaves the
            // visible window (short lists keep selection stable under the wheel).
            let rows = crate::commands::command_rows(app);
            let len = rows.len();
            let (selected, scroll) =
                shifted_select_state(app.selected_command, app.command_scroll, len, delta, 10);
            app.selected_command = crate::commands::clamp_command_selection(&rows, selected);
            app.command_scroll = scroll;
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
        ScrollTarget::BackgroundCommands => {
            let len = app.background_commands.len();
            let visible = app.bg_command_visible_cards.max(1);
            let (selected, scroll) = shifted_select_state(
                app.bg_command_selected,
                app.bg_command_scroll,
                len,
                delta,
                visible,
            );
            app.bg_command_selected = selected;
            app.bg_command_scroll = scroll;
            crate::background::clamp_background_selection(app);
        }
        ScrollTarget::BackgroundModels => {
            // Only navigate the Agents list when that tab is active (or legacy modal).
            if app.mode == Mode::ModelRouting
                && app.model_routing_tab != crate::state::ModelRoutingTab::Agents
            {
                return;
            }
            let len = 5usize; // BG_MODEL_TASKS length
            let visible_tasks = 4usize;
            let (selected, scroll) = shifted_select_state(
                app.bg_models_selected,
                app.bg_models_scroll,
                len,
                delta,
                visible_tasks,
            );
            app.bg_models_selected = selected;
            app.bg_models_scroll = scroll;
        }
        ScrollTarget::BackgroundCommandOutput => {
            app.bg_command_output_follow = false;
            if delta.is_positive() {
                app.bg_command_output_scroll =
                    app.bg_command_output_scroll.saturating_add(delta as usize);
            } else {
                app.bg_command_output_scroll = app
                    .bg_command_output_scroll
                    .saturating_sub(delta.unsigned_abs());
            }
        }
        ScrollTarget::MessageQueue => {
            let len = app.queued_user_messages.len();
            let (selected, scroll) = shifted_select_state(
                app.queued_message_selected,
                app.queued_message_scroll,
                len,
                delta,
                10,
            );
            app.queued_message_selected = selected;
            app.queued_message_scroll = scroll;
        }
        ScrollTarget::Help => {
            let visible = app.help_visible_rows.get().max(3);
            let len = crate::view::help::help_entry_count();
            app.help_scroll = shifted_scroll(app.help_scroll, len, visible, delta);
            app.selected_help = app.selected_help.clamp(
                app.help_scroll,
                app.help_scroll
                    .saturating_add(visible.saturating_sub(1))
                    .min(len.saturating_sub(1)),
            );
        }
        ScrollTarget::PathMentions => {
            let len = crate::path_mentions::filtered_path_candidates(app).len();
            let (selected, scroll) =
                shifted_select_state(app.selected_path, app.path_scroll, len, delta, 12);
            app.selected_path = selected;
            app.path_scroll = scroll;
        }
        ScrollTarget::Rewind => {
            let len = crate::chat::rewind_checkpoints(app).len();
            let (selected, scroll) =
                shifted_select_state(app.selected_rewind, app.rewind_scroll, len, delta, 10);
            app.selected_rewind = selected;
            app.rewind_scroll = scroll;
        }
    }
}

fn scroll_to(app: &mut TuiApp, target: ScrollTarget, offset: usize) {
    match target {
        ScrollTarget::Commands => {
            let len = crate::commands::command_rows(app).len();
            app.command_scroll = offset.min(len.saturating_sub(10));
            app.selected_command = crate::commands::clamp_command_selection(
                &crate::commands::command_rows(app),
                app.selected_command
                    .clamp(app.command_scroll, app.command_scroll + 9),
            );
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
        ScrollTarget::BackgroundCommands => {
            let len = app.background_commands.len();
            app.bg_command_selected = offset.min(len.saturating_sub(1));
            app.bg_command_scroll = app.bg_command_selected;
            crate::background::clamp_background_selection(app);
        }
        ScrollTarget::BackgroundModels => {
            let len = 5usize;
            app.bg_models_selected = offset.min(len.saturating_sub(1));
            app.bg_models_scroll = app.bg_models_selected.saturating_sub(3);
        }
        ScrollTarget::BackgroundCommandOutput => {
            app.bg_command_output_follow = false;
            app.bg_command_output_scroll = offset;
        }
        ScrollTarget::MessageQueue => {
            let len = app.queued_user_messages.len();
            app.queued_message_selected = offset.min(len.saturating_sub(1));
            app.queued_message_scroll = app.queued_message_selected;
        }
        ScrollTarget::Help => {
            let len = crate::view::help::help_entry_count();
            let visible = app.help_visible_rows.get().max(3);
            app.help_scroll = offset.min(len.saturating_sub(visible));
            app.selected_help = app.selected_help.clamp(
                app.help_scroll,
                app.help_scroll
                    .saturating_add(visible.saturating_sub(1))
                    .min(len.saturating_sub(1)),
            );
        }
        ScrollTarget::PathMentions => {
            let len = crate::path_mentions::filtered_path_candidates(app).len();
            app.selected_path = offset.min(len.saturating_sub(1));
            app.path_scroll = app.selected_path.saturating_sub(11);
        }
        ScrollTarget::Rewind => {
            let len = crate::chat::rewind_checkpoints(app).len();
            app.selected_rewind = offset.min(len.saturating_sub(1));
            app.rewind_scroll = app.selected_rewind.saturating_sub(9);
        }
    }
}

/// Apply a Rewind modal selection (message index in chat).
pub(crate) fn run_rewind_checkpoint(app: &mut TuiApp, message_index: usize) {
    match revert_to_user_message(app, message_index) {
        Ok(()) => show_notification(
            app,
            "Rewind",
            "Restored chat and project files to this prompt.",
        ),
        Err(err) => push_diagnostic(app, err),
    }
    close_active_modal(app);
}

fn shifted_scroll(current: usize, len: usize, visible_rows: usize, delta: isize) -> usize {
    let max_scroll = len.saturating_sub(visible_rows);
    if delta.is_positive() {
        current.saturating_add(delta as usize).min(max_scroll)
    } else {
        current.saturating_sub(delta.unsigned_abs())
    }
}

/// Mouse-wheel list navigation: scroll the viewport, not the selection.
///
/// Keyboard PageUp/PageDown still use selection-first paging. Wheel should feel
/// like a scrollbar: the list moves under the cursor, and selection only
/// follows when it would leave the visible window.
fn shifted_select_state(
    selected: usize,
    scroll: usize,
    len: usize,
    delta: isize,
    visible_rows: usize,
) -> (usize, usize) {
    let mut state = SelectListState::new(selected, scroll);
    state.scroll_viewport(len, visible_rows, delta);
    (state.selected(), state.scroll())
}

fn scroll_models_by(app: &mut TuiApp, delta: isize) {
    let rows = build_model_rows(app);
    if rows.is_empty() {
        return;
    }
    let visible_rows = 14usize;
    let max_scroll = rows.len().saturating_sub(visible_rows);
    if delta.is_positive() {
        app.model_scroll = app
            .model_scroll
            .saturating_add(delta as usize)
            .min(max_scroll);
    } else {
        app.model_scroll = app.model_scroll.saturating_sub(delta.unsigned_abs());
    }

    // Keep the selected model on-screen without hopping selection by delta.
    let current_selected = active_model_list_selection(app);
    if let Some(selected_row) = selected_model_in_rows(&rows, current_selected) {
        let last_visible = app
            .model_scroll
            .saturating_add(visible_rows.saturating_sub(1))
            .min(rows.len().saturating_sub(1));
        if selected_row < app.model_scroll {
            select_model_near_row(app, &rows, app.model_scroll, true);
        } else if selected_row > last_visible {
            select_model_near_row(app, &rows, last_visible, false);
        }
    }
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

fn active_model_list_selection(app: &TuiApp) -> usize {
    if app.mode == Mode::BgModelPicker {
        app.bg_model_picker_selected
    } else {
        app.selected_model
    }
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
    let resolved = selected
        .or_else(|| first_model_index(rows))
        .unwrap_or_else(|| active_model_list_selection(app));
    if app.mode == Mode::BgModelPicker {
        app.bg_model_picker_selected = resolved;
    } else {
        app.selected_model = resolved;
    }
}

fn model_row_index(row: &ListRow) -> Option<usize> {
    match row {
        ListRow::Model { index } => Some(*index),
        ListRow::Header { .. } | ListRow::Spacer => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crossterm::event::{KeyModifiers, MouseEvent};
    use navi_sdk::{AgentEvent, QuestionOption, QuestionRequest, QuestionResponse};
    use ratatui::layout::Rect;
    use ratatui::prelude::Line;

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

    fn mouse_drag(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
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

    fn mouse_scroll_up(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn seed_chat_cache(app: &mut TuiApp, lines: &[&str], rect: Rect) {
        let mut cache = app.chat_render_cache.borrow_mut();
        cache.lines = lines
            .iter()
            .map(|line| Line::from((*line).to_string()))
            .collect();
        cache.chat_rect = Some(rect);
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

        // Short root palette fits one page: wheel is viewport-first, so selection
        // stays put when there is no overflow. Chat must not move either.
        assert_eq!(
            app.selected_command, 0,
            "wheel over a short Commands list must not jump selection"
        );
        assert_eq!(app.command_scroll, 0);
        assert_eq!(
            app.scroll_offset, 12,
            "chat scroll must not move while a modal list is active"
        );
    }

    #[test]
    fn mouse_wheel_scrolls_list_viewport_without_jumping_selection() {
        use navi_sdk::{SessionId, SessionSnapshotInfo};
        use std::path::PathBuf;

        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Sessions);
        app.saved_sessions = (0..30)
            .map(|i| SessionSnapshotInfo {
                id: SessionId::new(format!("session-{i}")),
                title: Some(format!("Session {i}")),
                project: PathBuf::from("/tmp/test-project"),
                created_at: i as u64,
                updated_at: i as u64,
            })
            .collect();
        // Sessions wheel uses visible_rows=10. Keep selection mid-window so the
        // first wheel step advances scroll without leaving the visible range.
        app.selected_session = 5;
        app.session_scroll = 0;
        app.scroll_offset = 9;

        handle_mouse(&mut app, mouse_scroll_down(4, 3));

        // delta = +3: viewport advances; selection remains while still visible
        // (window is 3..=12, selected 5 is still inside).
        assert_eq!(app.session_scroll, 3);
        assert_eq!(app.selected_session, 5);
        assert_eq!(app.scroll_offset, 9);

        // Scroll far enough that the old selection leaves the window
        // (scroll > 5 → selection is pulled up to the new top).
        handle_mouse(&mut app, mouse_scroll_down(4, 3)); // scroll 6
        assert_eq!(app.session_scroll, 6);
        assert_eq!(
            app.selected_session, 6,
            "selection follows only once it would leave the visible window"
        );
        handle_mouse(&mut app, mouse_scroll_down(4, 3));
        handle_mouse(&mut app, mouse_scroll_down(4, 3));
        assert!(
            app.session_scroll >= 10,
            "viewport should keep advancing, got {}",
            app.session_scroll
        );
        assert_eq!(
            app.selected_session, app.session_scroll,
            "selection tracks the top of the window after leaving it"
        );
    }

    #[test]
    fn mouse_wheel_scrolls_chat_by_lines_without_selecting_blocks() {
        let mut app = test_app("");
        app.messages.push(crate::state::ChatMessage::new(
            crate::state::ChatRole::Assistant,
            "chat".into(),
        ));
        seed_chat_cache(
            &mut app,
            &[
                "line 0", "line 1", "line 2", "line 3", "line 4", "line 5", "line 6",
            ],
            Rect::new(0, 0, 20, 3),
        );
        app.selected_chat_source = Some(crate::state::ChatLineSource::Message(0));
        app.selection = Some(SelectionState {
            start: (0, 0),
            end: (0, 1),
            active: false,
            bound_source: Some(crate::state::ChatLineSource::Message(0)),
        });

        handle_mouse(&mut app, mouse_scroll_up(4, 1));

        assert_eq!(app.scroll_offset, CHAT_WHEEL_LINES);
        assert!(app.selected_chat_source.is_none());
        assert!(app.selection.is_none());
        assert!(crate::view::input::composer_is_focused(&app));
    }

    #[test]
    fn mouse_wheel_during_active_drag_preserves_and_extends_selection() {
        let mut app = test_app("");
        seed_chat_cache(
            &mut app,
            &[
                "line 0", "line 1", "line 2", "line 3", "line 4", "line 5", "line 6",
            ],
            Rect::new(0, 0, 20, 3),
        );
        // Bottom page: lines 4..=6 visible. Start drag on "line 5".
        handle_mouse(&mut app, mouse_down(0, 1));
        assert!(app.selection.as_ref().is_some_and(|s| s.active));
        let start = app.selection.as_ref().unwrap().start;
        assert_eq!(start.0, 5);

        // Scroll up while still dragging — must keep selection and move end
        // to the line now under the cursor (older content).
        handle_mouse(&mut app, mouse_scroll_up(0, 1));

        let selection = app.selection.as_ref().expect("active drag survives wheel");
        assert!(selection.active, "drag must stay active across wheel");
        assert_eq!(selection.start, start, "anchor must not move");
        assert_eq!(app.scroll_offset, CHAT_WHEEL_LINES);
        // After +2 scroll, row 1 maps to line_index 5 - 2 = 3.
        assert_eq!(selection.end.0, 3);
        assert!(
            selection.end.0 < selection.start.0,
            "wheel-up during drag should extend selection toward older lines"
        );
    }

    #[test]
    fn composer_hit_restores_focus_after_scrollback_selection() {
        let mut app = test_app("draft");
        app.messages.push(crate::state::ChatMessage::new(
            crate::state::ChatRole::Assistant,
            "chat".into(),
        ));
        app.selected_chat_source = Some(crate::state::ChatLineSource::Message(0));
        app.selection = Some(SelectionState {
            start: (0, 0),
            end: (0, 1),
            active: false,
            bound_source: Some(crate::state::ChatLineSource::Message(0)),
        });
        app.register_hit(
            Rect::new(2, 8, 30, 2),
            90,
            "focus composer",
            HitAction::FocusComposer,
        );

        handle_mouse(&mut app, mouse_down(4, 8));

        assert!(app.selected_chat_source.is_none());
        assert!(app.selection.is_none());
        assert!(crate::view::input::composer_is_focused(&app));
    }

    #[test]
    fn focus_round_trip_chat_click_then_composer_click() {
        // Click chat → scrollback focused (composer collapses).
        // Click composer → focus restored.
        let mut app = test_app("draft text");
        app.messages.push(crate::state::ChatMessage::new(
            crate::state::ChatRole::Assistant,
            "hello world from assistant".into(),
        ));
        seed_chat_cache(
            &mut app,
            &["hello world from assistant"],
            Rect::new(0, 0, 40, 5),
        );
        {
            let mut cache = app.chat_render_cache.borrow_mut();
            cache.line_sources = vec![crate::state::ChatLineSource::Message(0)];
        }
        app.register_hit(Rect::new(0, 0, 40, 5), 5, "chat", HitAction::ChatMessage(0));
        app.register_hit(
            Rect::new(0, 6, 40, 3),
            90,
            "focus composer",
            HitAction::FocusComposer,
        );

        assert!(crate::view::input::composer_is_focused(&app));

        // Click chat line.
        handle_mouse(&mut app, mouse_down(2, 1));
        assert_eq!(
            app.selected_chat_source,
            Some(crate::state::ChatLineSource::Message(0))
        );
        assert!(!crate::view::input::composer_is_focused(&app));
        // End pure click.
        handle_mouse(&mut app, mouse_up(2, 1));

        // Click composer area.
        handle_mouse(&mut app, mouse_down(2, 7));
        assert!(
            app.selected_chat_source.is_none(),
            "composer click must clear chat block focus"
        );
        assert!(
            crate::view::input::composer_is_focused(&app),
            "composer click must restore input focus"
        );
    }

    #[test]
    fn mouse_drag_updates_active_selection_before_mouse_up() {
        let mut app = test_app("");
        seed_chat_cache(&mut app, &["hello world"], Rect::new(0, 0, 20, 1));

        handle_mouse(&mut app, mouse_down(0, 0));
        handle_mouse(&mut app, mouse_drag(5, 0));

        let selection = app.selection.as_ref().expect("selection");
        assert!(selection.active);
        assert_eq!(selection.start, (0, 0));
        assert_eq!(selection.end, (0, 5));
        assert_eq!(selected_text(&app).as_deref(), Some("hello"));
    }

    #[test]
    fn mouse_drag_near_top_scrolls_and_maps_endpoint_after_scroll() {
        let mut app = test_app("");
        seed_chat_cache(
            &mut app,
            &["line 0", "line 1", "line 2", "line 3", "line 4"],
            Rect::new(0, 0, 20, 3),
        );

        handle_mouse(&mut app, mouse_down(0, 1));
        handle_mouse(&mut app, mouse_drag(4, 0));

        let selection = app.selection.as_ref().expect("selection");
        // Edge drag scrolls by CHAT_WHEEL_LINES (2).
        assert_eq!(app.scroll_offset, CHAT_WHEEL_LINES);
        assert_eq!(selection.start, (3, 0));
        // After +2 scroll, row 0 maps to line 0.
        assert_eq!(selection.end, (0, 4));
    }

    #[test]
    fn mouse_drag_near_bottom_scrolls_and_maps_endpoint_after_scroll() {
        let mut app = test_app("");
        seed_chat_cache(
            &mut app,
            &["line 0", "line 1", "line 2", "line 3", "line 4"],
            Rect::new(0, 0, 20, 3),
        );
        app.scroll_offset = 2;

        handle_mouse(&mut app, mouse_down(0, 1));
        handle_mouse(&mut app, mouse_drag(4, 2));

        let selection = app.selection.as_ref().expect("selection");
        // Edge drag scrolls by CHAT_WHEEL_LINES toward newer content.
        assert_eq!(app.scroll_offset, 0);
        assert_eq!(selection.start, (1, 0));
        // After -2 scroll (clamped to 0), row 2 maps to line 4.
        assert_eq!(selection.end, (4, 4));
    }

    #[test]
    fn mouse_down_up_without_drag_events_still_selects_text() {
        // Terminals that only emit press+release (no Drag) must still select.
        let mut app = test_app("");
        seed_chat_cache(&mut app, &["hello world"], Rect::new(0, 0, 20, 1));

        handle_mouse(&mut app, mouse_down(0, 0));
        handle_mouse(&mut app, mouse_up(5, 0));

        assert_eq!(selected_text(&app).as_deref(), Some("hello"));
        assert!(!app.selection.as_ref().expect("selection").active);
    }

    #[test]
    fn mouse_moved_extends_active_text_selection() {
        let mut app = test_app("");
        seed_chat_cache(&mut app, &["hello world"], Rect::new(0, 0, 20, 1));

        handle_mouse(&mut app, mouse_down(0, 0));
        handle_mouse(&mut app, mouse_moved(7, 0));

        let selection = app.selection.as_ref().expect("selection");
        assert!(selection.active);
        assert_eq!(selection.start, (0, 0));
        assert_eq!(selection.end, (0, 7));
        assert_eq!(selected_text(&app).as_deref(), Some("hello w"));
    }

    #[test]
    fn mouse_down_does_not_scroll_when_starting_text_selection() {
        let mut app = test_app("");
        seed_chat_cache(
            &mut app,
            &["line 0", "line 1", "line 2", "line 3", "line 4", "line 5"],
            Rect::new(0, 0, 20, 3),
        );
        app.scroll_offset = 0;
        app.messages.push(crate::state::ChatMessage::new(
            crate::state::ChatRole::Assistant,
            "chat".into(),
        ));
        // Register a chat hit that would previously call select_chat_block (scroll).
        app.register_hit(Rect::new(0, 0, 20, 3), 5, "chat", HitAction::ChatMessage(0));
        // Seed line sources so bound_source resolves.
        {
            let mut cache = app.chat_render_cache.borrow_mut();
            cache.line_sources = vec![crate::state::ChatLineSource::Message(0); 6];
        }

        handle_mouse(&mut app, mouse_down(2, 1));

        assert_eq!(
            app.scroll_offset, 0,
            "starting a drag must not auto-scroll the viewport"
        );
        assert!(app.selection.as_ref().is_some_and(|s| s.active));
        assert_eq!(app.selection.as_ref().unwrap().start, (4, 2));
    }

    #[test]
    fn mouse_up_on_hit_region_finishes_active_selection() {
        let mut app = test_app("");
        seed_chat_cache(&mut app, &["hello world"], Rect::new(0, 0, 20, 1));
        app.register_hit(
            Rect::new(4, 0, 8, 1),
            5,
            "chat hit",
            HitAction::ChatMessage(0),
        );

        handle_mouse(&mut app, mouse_down(0, 0));
        handle_mouse(&mut app, mouse_drag(5, 0));
        handle_mouse(&mut app, mouse_up(5, 0));

        assert_eq!(selected_text(&app).as_deref(), Some("hello"));
        assert!(!app.selection.as_ref().expect("selection").active);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn mouse_drag_on_full_line_chat_hit_selects_text() {
        // Regression: chat lines register full-width hits; drag must still select text.
        let mut app = test_app("");
        seed_chat_cache(&mut app, &["hello world"], Rect::new(0, 0, 20, 1));
        app.messages.push(crate::state::ChatMessage::new(
            crate::state::ChatRole::Assistant,
            "hello world".into(),
        ));
        // Full line hit — matches real chat hit registration.
        app.register_hit(
            Rect::new(0, 0, 20, 1),
            5,
            "assistant line",
            HitAction::ChatMessage(0),
        );

        handle_mouse(&mut app, mouse_down(0, 0));
        assert!(
            app.selection.as_ref().is_some_and(|s| s.active),
            "drag selection must start even when a chat hit covers the line"
        );
        handle_mouse(&mut app, mouse_drag(5, 0));
        handle_mouse(&mut app, mouse_up(5, 0));

        assert_eq!(selected_text(&app).as_deref(), Some("hello"));
        assert!(!app.selection.as_ref().expect("selection").active);
    }

    #[test]
    fn mouse_move_after_wheel_does_not_restore_list_scroll_to_hovered_item() {
        use navi_sdk::{SessionId, SessionSnapshotInfo};
        use std::path::PathBuf;

        // Use a long Sessions list so the wheel actually advances the viewport.
        // (Short Command palettes have max_scroll=0 under viewport-first scrolling.)
        let mut app = test_app("");
        replace_modal(&mut app, crate::state::ModalKind::Sessions);
        app.saved_sessions = (0..30)
            .map(|i| SessionSnapshotInfo {
                id: SessionId::new(format!("session-{i}")),
                title: Some(format!("Session {i}")),
                project: PathBuf::from("/tmp/test-project"),
                created_at: i as u64,
                updated_at: i as u64,
            })
            .collect();
        app.selected_session = 5;
        app.session_scroll = 0;
        // Hit region for the first visible row (index 0) — still under the
        // cursor after the viewport moves; hover must not snap scroll back.
        app.register_hit(
            Rect::new(2, 3, 12, 1),
            20,
            "session first row",
            HitAction::Session(0),
        );

        handle_mouse(&mut app, mouse_scroll_down(4, 3));
        let scrolled = app.session_scroll;
        let selected_after_scroll = app.selected_session;
        assert!(scrolled > 0, "wheel must advance viewport before hover");
        handle_mouse(&mut app, mouse_moved(4, 3));

        // Hover highlighting must not snap selection/scroll back to the row under the cursor.
        assert_eq!(app.session_scroll, scrolled);
        assert_eq!(app.selected_session, selected_after_scroll);
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

    fn seed_pending_image_chip(app: &mut crate::app::TuiApp) {
        app.pending_images.push(crate::state::PendingImage {
            media_type: "image/png".into(),
            data: "AAAA".into(),
            width: Some(10),
            height: Some(10),
        });
        app.register_hit(
            Rect::new(0, 0, 10, 1),
            20,
            "preview pending",
            HitAction::PreviewPendingImage(0),
        );
    }

    #[test]
    fn image_chip_hover_opens_lightbox() {
        let mut app = test_app("");
        seed_pending_image_chip(&mut app);

        assert!(handle_mouse(&mut app, mouse_moved(1, 0)));
        assert!(
            app.image_hover.is_some(),
            "hovering [Image N] must open the lightbox"
        );
        // Second move on same chip must not thrash / force unnecessary work.
        assert!(!handle_mouse(&mut app, mouse_moved(2, 0)));
        assert!(app.image_hover.is_some());
        assert!(app.image_hover_close_deadline.is_none());
    }

    #[test]
    fn image_lightbox_keep_cancels_leave_close() {
        let mut app = test_app("");
        seed_pending_image_chip(&mut app);
        handle_mouse(&mut app, mouse_moved(1, 0));
        assert!(app.image_hover.is_some());

        // Leave sticky zone → arm grace close.
        handle_mouse(&mut app, mouse_moved(50, 20));
        assert!(app.image_hover.is_some(), "grace: still open after leave");
        assert!(app.image_hover_close_deadline.is_some());

        // Enter lightbox body before grace expires → cancel close.
        app.register_hit(
            Rect::new(40, 10, 20, 10),
            100,
            "lightbox",
            HitAction::ImageLightboxKeep,
        );
        handle_mouse(&mut app, mouse_moved(45, 12));
        assert!(app.image_hover.is_some());
        assert!(
            app.image_hover_close_deadline.is_none(),
            "resting on the lightbox must cancel leave-close"
        );
    }

    #[test]
    fn image_hover_leave_grace_then_poll_closes() {
        let mut app = test_app("");
        seed_pending_image_chip(&mut app);
        handle_mouse(&mut app, mouse_moved(1, 0));
        handle_mouse(&mut app, mouse_moved(50, 20));
        assert!(app.image_hover_close_deadline.is_some());

        // Force deadline into the past.
        app.image_hover_close_deadline =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
        assert!(crate::view::image_preview::poll_image_hover_close(&mut app));
        assert!(app.image_hover.is_none());
        assert!(app.image_hover_close_deadline.is_none());
    }

    #[test]
    fn click_outside_image_lightbox_closes_it() {
        let mut app = test_app("");
        seed_pending_image_chip(&mut app);
        handle_mouse(&mut app, mouse_moved(1, 0));
        assert!(app.image_hover.is_some());

        handle_mouse(&mut app, mouse_down(50, 20));
        assert!(
            app.image_hover.is_none(),
            "click outside must close the image lightbox"
        );
    }

    #[test]
    fn esc_closes_image_lightbox() {
        use crossterm::event::KeyCode;
        let mut app = test_app("");
        seed_pending_image_chip(&mut app);
        handle_mouse(&mut app, mouse_moved(1, 0));
        assert!(app.image_hover.is_some());

        let should_quit = handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!should_quit);
        assert!(
            app.image_hover.is_none(),
            "Esc must close the image lightbox"
        );
    }

    #[test]
    fn click_plan_topbar_toggles_expanded() {
        let mut app = test_app("");
        app.active_plan = Some(crate::state::ActivePlanUiState {
            plan_id: "p1".into(),
            title: "Demo".into(),
            steps: vec![crate::state::ActivePlanStepUi {
                description: "step one".into(),
                completed: false,
            }],
            status: "active".into(),
            expanded: false,
            show_all_steps: false,
            completed_at: None,
        });
        app.register_hit(
            Rect::new(0, 0, 40, 3),
            40,
            "plan topbar",
            HitAction::TogglePlanTopbar,
        );
        handle_mouse(&mut app, mouse_down(2, 1));
        assert!(
            app.active_plan.as_ref().is_some_and(|p| p.expanded),
            "click must expand plan topbar"
        );
        handle_mouse(&mut app, mouse_down(2, 1));
        assert!(
            app.active_plan.as_ref().is_some_and(|p| !p.expanded),
            "second click must collapse plan topbar"
        );
    }

    #[test]
    fn click_plan_more_expands_all_steps() {
        let mut app = test_app("");
        let steps = (1..=10)
            .map(|i| crate::state::ActivePlanStepUi {
                description: format!("step {i}"),
                completed: true,
            })
            .collect();
        app.active_plan = Some(crate::state::ActivePlanUiState {
            plan_id: "p1".into(),
            title: "Big".into(),
            steps,
            status: "completed".into(),
            expanded: true,
            show_all_steps: false,
            completed_at: None,
        });
        // Higher-z "+N more" hit wins over the whole-bar toggle.
        app.register_hit(
            Rect::new(0, 0, 40, 12),
            40,
            "plan topbar",
            HitAction::TogglePlanTopbar,
        );
        app.register_hit(
            Rect::new(0, 10, 40, 1),
            50,
            "expand more",
            HitAction::ExpandPlanMore,
        );
        handle_mouse(&mut app, mouse_down(2, 10));
        let plan = app.active_plan.as_ref().expect("plan");
        assert!(plan.expanded, "must stay expanded");
        assert!(
            plan.show_all_steps,
            "must show all steps after +N more click"
        );
    }
}
