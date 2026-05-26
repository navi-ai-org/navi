use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModalSpec {
    pub(crate) max_width: u16,
    pub(crate) height: u16,
    pub(crate) min_width: u16,
    pub(crate) min_height: u16,
    pub(crate) horizontal_margin: u16,
    pub(crate) vertical_margin: u16,
}

impl ModalSpec {
    pub(crate) fn fixed(max_width: u16, height: u16) -> Self {
        Self {
            max_width,
            height,
            min_width: 40,
            min_height: 10,
            horizontal_margin: 8,
            vertical_margin: 4,
        }
    }

    pub(crate) fn rect(self, area: Rect) -> Rect {
        let width = area
            .width
            .saturating_sub(self.horizontal_margin)
            .min(self.max_width)
            .max(self.min_width);
        let height = area
            .height
            .saturating_sub(self.vertical_margin)
            .min(self.height)
            .max(self.min_height);
        centered_rect(area, width, height)
    }
}

pub(crate) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(area.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_spec_centers_with_bounds() {
        let area = Rect::new(0, 0, 100, 40);
        let rect = ModalSpec::fixed(72, 20).rect(area);

        assert_eq!(rect.width, 72);
        assert_eq!(rect.height, 20);
        assert_eq!(rect.x, 14);
        assert_eq!(rect.y, 10);
    }

    #[test]
    fn modal_spec_respects_small_terminals() {
        let area = Rect::new(0, 0, 30, 8);
        let rect = ModalSpec::fixed(72, 20).rect(area);

        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 8);
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
    }
}
