use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::TuiApp;
use crate::theme::{accent, ghost, modal_bg};
use crate::ui::interaction::ScrollTarget;

pub(crate) fn render_scrollbar(
    frame: &mut Frame<'_>,
    app: &TuiApp,
    area: Rect,
    total_items: usize,
    offset: usize,
    target: ScrollTarget,
) {
    let visible_items = area.height as usize;
    if area.width == 0 || area.height == 0 || total_items <= visible_items || visible_items == 0 {
        return;
    }

    let bar = Rect::new(area.x + area.width - 1, area.y, 1, area.height);
    let thumb = copland::list::scrollbar_thumb(bar, total_items, visible_items, offset);
    let lines = (0..bar.height)
        .map(|row| {
            let y = bar.y + row;
            let style = if y >= thumb.y && y < thumb.y.saturating_add(thumb.height) {
                Style::default()
                    .fg(accent())
                    .bg(modal_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ghost()).bg(modal_bg())
            };
            let glyph = if y >= thumb.y && y < thumb.y.saturating_add(thumb.height) {
                "█"
            } else {
                "│"
            };
            Line::from(Span::styled(glyph, style))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(modal_bg())),
        bar,
    );

    for row in 0..bar.height {
        let click_offset =
            copland::list::scrollbar_offset_for_row(bar, total_items, visible_items, row);
        app.register_hit(
            Rect::new(bar.x, bar.y + row, 1, 1),
            80,
            "scrollbar",
            crate::ui::interaction::HitAction::ScrollTo {
                target,
                offset: click_offset,
            },
        );
    }
}
