use ratatui::layout::{Margin, Rect};
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::TuiApp;
use crate::notifications::visible_notification;
use crate::theme::{ACCENT, PANEL, PINK, TEXT};

pub(super) fn render_notification(frame: &mut Frame<'_>, app: &TuiApp, area: Rect) {
    let Some(notification) = visible_notification(app) else {
        return;
    };

    let message_width = notification
        .message
        .chars()
        .count()
        .max(notification.title.chars().count())
        .saturating_add(8);
    let available_width = area.width.saturating_sub(4).max(1);
    let width = (message_width.clamp(26, 68) as u16).min(available_width);
    let height = area.height.clamp(1, 3);
    let x = area.x + area.width.saturating_sub(width + 2);
    let y = area.y
        + area
            .height
            .saturating_sub(9)
            .min(area.height.saturating_sub(height));
    let rect = Rect::new(x, y, width, height);
    let inner = rect.inner(Margin {
        horizontal: 2,
        vertical: 1,
    });

    frame.render_widget(Clear, rect);
    frame.render_widget(
        Block::new()
            .title(Line::from(vec![Span::styled(
                format!(" {} ", notification.title),
                Style::default().fg(PINK).add_modifier(Modifier::BOLD),
            )]))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .style(Style::default().bg(PANEL)),
        rect,
    );
    frame.render_widget(
        Paragraph::new(notification.message.clone())
            .style(Style::default().fg(TEXT).bg(PANEL))
            .wrap(Wrap { trim: true }),
        inner,
    );
}
