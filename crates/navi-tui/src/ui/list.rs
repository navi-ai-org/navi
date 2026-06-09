use ratatui::layout::Rect;
use ratatui::prelude::{Frame, Line, Span};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

use crate::TuiApp;
use crate::theme::{accent, ghost, modal_bg};
use crate::ui::interaction::{HitAction, ScrollTarget};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SelectListState {
    selected: usize,
    scroll: usize,
}

impl SelectListState {
    pub(crate) fn new(selected: usize, scroll: usize) -> Self {
        Self { selected, scroll }
    }

    pub(crate) fn selected(self) -> usize {
        self.selected
    }

    pub(crate) fn scroll(self) -> usize {
        self.scroll
    }

    pub(crate) fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub(crate) fn reset(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    pub(crate) fn select_next(&mut self, len: usize) {
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
    }

    pub(crate) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(crate) fn page_next(&mut self, len: usize, amount: usize) {
        if len > 0 {
            self.selected = (self.selected + amount).min(len - 1);
        }
    }

    pub(crate) fn page_previous(&mut self, amount: usize) {
        self.selected = self.selected.saturating_sub(amount);
    }

    pub(crate) fn sync_scroll(&mut self, visible_rows: usize) {
        let visible_rows = visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_rows {
            self.scroll = self.selected.saturating_sub(visible_rows - 1);
        }
    }

    pub(crate) fn sync_scroll_with_context(
        &mut self,
        visible_rows: usize,
        trailing_context: usize,
    ) {
        let visible_rows = visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_rows {
            self.scroll = self
                .selected
                .saturating_sub(visible_rows.saturating_sub(trailing_context).max(1));
        }
    }

    pub(crate) fn clamp_scroll(&mut self, len: usize, visible_rows: usize) {
        self.scroll = self.scroll.min(len.saturating_sub(visible_rows));
    }

    pub(crate) fn scroll_offset_for_selected(selected: usize, visible_rows: usize) -> usize {
        let mut state = Self::new(selected, 0);
        state.sync_scroll(visible_rows);
        state.scroll()
    }
}

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
    let thumb = scrollbar_thumb(bar, total_items, visible_items, offset);
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
        let click_offset = scrollbar_offset_for_row(bar, total_items, visible_items, row);
        app.register_hit(
            Rect::new(bar.x, bar.y + row, 1, 1),
            80,
            "scrollbar",
            HitAction::ScrollTo {
                target,
                offset: click_offset,
            },
        );
    }
}

fn scrollbar_thumb(area: Rect, total_items: usize, visible_items: usize, offset: usize) -> Rect {
    let height = area.height as usize;
    let thumb_height = ((visible_items * height).div_ceil(total_items))
        .max(1)
        .min(height);
    let max_offset = total_items.saturating_sub(visible_items).max(1);
    let max_thumb_top = height.saturating_sub(thumb_height);
    let thumb_top = offset.min(max_offset) * max_thumb_top / max_offset;
    Rect::new(
        area.x,
        area.y + thumb_top as u16,
        area.width,
        thumb_height as u16,
    )
}

fn scrollbar_offset_for_row(
    area: Rect,
    total_items: usize,
    visible_items: usize,
    row: u16,
) -> usize {
    let height = area.height as usize;
    let max_offset = total_items.saturating_sub(visible_items);
    if height <= 1 || max_offset == 0 {
        return 0;
    }
    (row as usize * max_offset).div_ceil(height.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_clamps_to_available_items() {
        let mut state = SelectListState::new(10, 5);

        state.clamp(3);

        assert_eq!(state.selected(), 2);
        assert_eq!(state.scroll(), 5);

        state.clamp(0);
        assert_eq!(state.selected(), 0);
        assert_eq!(state.scroll(), 0);
    }

    #[test]
    fn scroll_sync_keeps_selection_visible() {
        assert_eq!(SelectListState::scroll_offset_for_selected(0, 6), 0);
        assert_eq!(SelectListState::scroll_offset_for_selected(5, 6), 0);
        assert_eq!(SelectListState::scroll_offset_for_selected(6, 6), 1);
        assert_eq!(SelectListState::scroll_offset_for_selected(11, 6), 6);
    }

    #[test]
    fn scrollbar_click_row_maps_to_scroll_offset() {
        let area = Rect::new(0, 0, 1, 10);

        assert_eq!(scrollbar_offset_for_row(area, 100, 10, 0), 0);
        assert_eq!(scrollbar_offset_for_row(area, 100, 10, 9), 90);
    }
}
