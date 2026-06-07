use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::TuiApp;
use crate::notifications::show_notification;
use crate::state::SelectionState;

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
                line_text.chars().count()
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

fn copy_selection_to_clipboard(app: &mut TuiApp) {
    if let Some(selected_text) = selected_text(app) {
        // ALWAYS send OSC 52 as a robust fallback for terminals
        use base64::prelude::*;
        let b64 = BASE64_STANDARD.encode(&selected_text);
        print!("\x1B]52;c;{}\x07", b64);
        let _ = std::io::Write::flush(&mut std::io::stdout());

        show_notification(app, "Clipboard", "Texto copiado (OSC 52)".to_string());
    }
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
            app.scroll_offset = app.scroll_offset.saturating_sub(3);
        }
        MouseEventKind::ScrollUp => {
            app.scroll_offset = app.scroll_offset.saturating_add(3);
        }
        MouseEventKind::Down(MouseButton::Left) => {
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
            let pos = map_mouse_to_text(app, mouse.column, mouse.row);
            if finish_selection(app, pos) {
                copy_selection_to_clipboard(app);
            }
        }
        _ => {}
    }
}
