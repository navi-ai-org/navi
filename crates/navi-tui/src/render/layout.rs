use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span, Style};
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
        .style(Style::default().fg(text()).bg(panel()))
}

pub(crate) fn clear_modal_area(frame: &mut Frame<'_>, area: Rect) {
    use ratatui::widgets::Clear;
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .style(Style::default().fg(panel()).bg(panel())),
        area,
    );
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
