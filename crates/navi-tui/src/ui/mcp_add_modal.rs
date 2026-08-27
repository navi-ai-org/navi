use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::Line,
    widgets::{Block, Borders, Clear},
};

use crate::{app::TuiApp, theme};

pub(crate) fn draw_mcp_add_modal(f: &mut Frame, app: &mut TuiApp) {
    let area = f.area();
    let modal_width = (area.width * 80 / 100).max(40).min(100);
    let modal_height = (area.height * 80 / 100).max(10).min(30);

    let x = area.width.saturating_sub(modal_width) / 2;
    let y = area.height.saturating_sub(modal_height) / 2;
    let modal_area = Rect::new(x, y, modal_width, modal_height);

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .title(" Quick Add MCP ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::muted()));

    let inner_area = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // text input
            Constraint::Length(2), // hints
        ])
        .split(inner_area);

    let text_content = app.mcp_add_input.clone();
    let text_paragraph = ratatui::widgets::Paragraph::new(text_content)
        .wrap(ratatui::widgets::Wrap { trim: false })
        .style(theme::modal_style());
    f.render_widget(text_paragraph, chunks[0]);

    // Render cursor
    let mut cursor_x = chunks[0].x;
    let mut cursor_y = chunks[0].y;
    // VERY rough cursor approximation for now just to make it functional
    let text_before_cursor = &app.mcp_add_input[..app.mcp_add_cursor.min(app.mcp_add_input.len())];
    let mut lines = text_before_cursor.split('\n');
    let last_line = lines.next_back().unwrap_or("");
    cursor_x += ratatui::text::Span::raw(last_line).width() as u16;
    cursor_y +=
        (text_before_cursor.matches('\n').count() as u16).min(chunks[0].height.saturating_sub(1));

    f.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));

    let hints = vec![Line::from(vec![
        ratatui::text::Span::styled(
            "<Ctrl+Enter>",
            Style::default().fg(ratatui::style::Color::Yellow),
        ),
        ratatui::text::Span::raw(" Add    "),
        ratatui::text::Span::styled("<Esc>", Style::default().fg(ratatui::style::Color::Yellow)),
        ratatui::text::Span::raw(" Cancel"),
    ])];
    let hints_paragraph =
        ratatui::widgets::Paragraph::new(hints).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(hints_paragraph, chunks[1]);
}
