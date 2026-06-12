use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(crate) const VIEWPORT_HORIZONTAL_MARGIN: u16 = 1;

pub(crate) fn viewport_rect(area: Rect) -> Rect {
    inset_rect(area, VIEWPORT_HORIZONTAL_MARGIN, 0)
}

fn inset_rect(area: Rect, horizontal: u16, vertical: u16) -> Rect {
    let horizontal = horizontal.min(area.width / 2);
    let vertical = vertical.min(area.height / 2);
    Rect::new(
        area.x + horizontal,
        area.y + vertical,
        area.width - horizontal * 2,
        area.height - vertical * 2,
    )
}

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

    #[test]
    fn viewport_rect_keeps_horizontal_safe_area() {
        let area = Rect::new(0, 0, 100, 20);
        let rect = viewport_rect(area);

        assert_eq!(rect.x, 1);
        assert_eq!(rect.width, 98);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.height, 20);
    }
}
