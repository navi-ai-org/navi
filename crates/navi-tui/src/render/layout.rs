use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Modifier, Span, Style};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::theme::*;
use crate::ui::layout::ModalSpec;
use crate::ui::list::SelectListState;

pub(crate) fn command_scroll_offset(selected: usize, visible_rows: usize) -> usize {
    SelectListState::scroll_offset_for_selected(selected, visible_rows)
}

pub(crate) fn modal_block(title: &'static str) -> Block<'static> {
    Block::new()
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default().fg(red()),
        )))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent()))
        .style(modal_style())
}

/// Paint every cell in `area` with spaces and `style`.
///
/// Unlike `Buffer::set_style`, this overwrites symbols so underlying chat
/// content cannot bleed through modal surfaces.
pub(crate) fn opaque_fill(frame: &mut Frame<'_>, area: Rect, style: Style) {
    let area = area.intersection(frame.area());
    if area.is_empty() {
        return;
    }
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(style);
        }
    }
}

pub(crate) fn fill_modal_surface(frame: &mut Frame<'_>, area: Rect) {
    opaque_fill(frame, area, modal_style());
}

pub(crate) fn fill_modal_scrim(frame: &mut Frame<'_>, area: Rect) {
    let area = area.intersection(frame.area());
    if area.is_empty() {
        return;
    }
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

pub(crate) fn clear_modal_area(frame: &mut Frame<'_>, area: Rect) {
    fill_modal_surface(frame, area);
    strip_dim(frame, area);
}

pub(crate) fn strip_dim(frame: &mut Frame<'_>, area: Rect) {
    let area = area.intersection(frame.area());
    if area.is_empty() {
        return;
    }
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].modifier.remove(Modifier::DIM);
        }
    }
}

pub(crate) fn modal_list_highlight_style() -> Style {
    active_item_style()
}

pub(crate) fn truncate_display(value: &str, max_chars: usize) -> String {
    let mut result = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        result.push_str("\n<truncated>");
    }
    result
}

pub(crate) fn command_row(label: &str, shortcut: &str, width: usize) -> String {
    let shortcut_width = 12usize.min(width.saturating_sub(1));
    let label_width = width.saturating_sub(shortcut_width + 1);
    format!(
        "{:<label_width$} {:<shortcut_width$}",
        fit_text(label, label_width),
        fit_text(shortcut, shortcut_width)
    )
}

pub(crate) fn model_row_simple(name: &str, configured: bool, width: usize) -> String {
    let marker_width = 3usize.min(width);
    let name_width = width.saturating_sub(marker_width + 4);
    let marker = if configured { "✓" } else { "" };

    format!(
        "    {:<name_width$} {:<marker_width$}",
        fit_text(name, name_width),
        marker
    )
}

fn fit_text(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut text = value.chars().take(width).collect::<String>();
    if value.chars().count() > width && width > 1 {
        text.pop();
        text.push('…');
    }
    text
}

pub(crate) fn modal_rect(area: Rect, max_width: u16, height: u16) -> Rect {
    ModalSpec::fixed(max_width, height).rect(area)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::theme::{ThemeId, with_palette};

    use super::*;

    #[test]
    fn opaque_fill_replaces_symbols_and_style() {
        with_palette(&ThemeId::Lain.palette(), || {
            let backend = TestBackend::new(5, 1);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| {
                    for x in 0..5 {
                        frame.buffer_mut()[(x, 0)].set_symbol("Z");
                    }
                    opaque_fill(frame, Rect::new(1, 0, 3, 1), modal_style());
                })
                .expect("draw");

            let buf = terminal.backend().buffer();
            assert_eq!(buf[(0, 0)].symbol(), "Z");
            assert_eq!(buf[(1, 0)].symbol(), " ");
            assert_eq!(buf[(2, 0)].symbol(), " ");
            assert_eq!(buf[(3, 0)].symbol(), " ");
            assert_eq!(buf[(4, 0)].symbol(), "Z");
            assert_eq!(buf[(2, 0)].bg, modal_bg());
        });
    }
}
