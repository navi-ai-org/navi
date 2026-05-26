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
}
