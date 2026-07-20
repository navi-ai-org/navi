use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectListState {
    selected: usize,
    scroll: usize,
}

impl SelectListState {
    pub fn new(selected: usize, scroll: usize) -> Self {
        Self { selected, scroll }
    }

    pub fn selected(self) -> usize {
        self.selected
    }

    pub fn scroll(self) -> usize {
        self.scroll
    }

    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn reset(&mut self) {
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn select_next(&mut self, len: usize) {
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn page_next(&mut self, len: usize, amount: usize) {
        if len > 0 {
            self.selected = (self.selected + amount).min(len - 1);
        }
    }

    pub fn page_previous(&mut self, amount: usize) {
        self.selected = self.selected.saturating_sub(amount);
    }

    /// Scroll the viewport without treating the wheel as "jump N selected items".
    ///
    /// Selection only moves when it would leave the visible window after the
    /// scroll, so short lists still keep a sensible selection while long lists
    /// behave like a normal scrollbar.
    pub fn scroll_viewport(&mut self, len: usize, visible_rows: usize, delta: isize) {
        let visible_rows = visible_rows.max(1);
        if len == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }

        let max_scroll = len.saturating_sub(visible_rows);
        if delta.is_positive() {
            self.scroll = self.scroll.saturating_add(delta as usize).min(max_scroll);
        } else {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs());
        }

        let last_visible = self
            .scroll
            .saturating_add(visible_rows.saturating_sub(1))
            .min(len.saturating_sub(1));
        if self.selected < self.scroll {
            self.selected = self.scroll;
        } else if self.selected > last_visible {
            self.selected = last_visible;
        } else {
            self.selected = self.selected.min(len - 1);
        }
    }

    pub fn sync_scroll(&mut self, visible_rows: usize) {
        let visible_rows = visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_rows {
            self.scroll = self.selected.saturating_sub(visible_rows - 1);
        }
    }

    pub fn sync_scroll_with_context(&mut self, visible_rows: usize, trailing_context: usize) {
        let visible_rows = visible_rows.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_rows {
            self.scroll = self
                .selected
                .saturating_sub(visible_rows.saturating_sub(trailing_context).max(1));
        }
    }

    pub fn clamp_scroll(&mut self, len: usize, visible_rows: usize) {
        self.scroll = self.scroll.min(len.saturating_sub(visible_rows));
    }

    pub fn scroll_offset_for_selected(selected: usize, visible_rows: usize) -> usize {
        let mut state = Self::new(selected, 0);
        state.sync_scroll(visible_rows);
        state.scroll()
    }
}

/// Computes the scrollbar thumb rect within a bar area.
pub fn scrollbar_thumb(
    area: Rect,
    total_items: usize,
    visible_items: usize,
    offset: usize,
) -> Rect {
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

/// Maps a click row within a scrollbar to the corresponding scroll offset.
pub fn scrollbar_offset_for_row(
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

    #[test]
    fn scroll_viewport_moves_offset_without_jumping_selection() {
        let mut state = SelectListState::new(2, 0);

        // Selection stays put while it remains on-screen.
        state.scroll_viewport(20, 5, 2);
        assert_eq!(state.scroll(), 2);
        assert_eq!(state.selected(), 2);

        // Selection is pulled into the new window only when it would leave it.
        state.scroll_viewport(20, 5, 3);
        assert_eq!(state.scroll(), 5);
        assert_eq!(state.selected(), 5);
    }

    #[test]
    fn scroll_viewport_clamps_to_max_offset() {
        let mut state = SelectListState::new(0, 0);
        state.scroll_viewport(10, 4, 100);
        assert_eq!(state.scroll(), 6);
        // Selection is pulled into the new window once it would leave it.
        assert_eq!(state.selected(), 6);

        state.scroll_viewport(10, 4, -100);
        assert_eq!(state.scroll(), 0);
        // Selection follows back into the visible window (last_visible = 3).
        assert_eq!(state.selected(), 3);
    }
}
