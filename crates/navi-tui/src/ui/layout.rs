use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(crate) const VIEWPORT_HORIZONTAL_MARGIN: u16 = 1;

pub(crate) fn viewport_rect(area: Rect) -> Rect {
    inset_rect(area, VIEWPORT_HORIZONTAL_MARGIN, 0)
}

pub(crate) fn split_left_right(
    area: Rect,
    min_left_width: u16,
    preferred_right_width: u16,
) -> (Rect, Rect) {
    let right_width = if area.width > min_left_width {
        preferred_right_width.min(area.width - min_left_width)
    } else {
        0
    };
    let left_width = area.width.saturating_sub(right_width);
    let left = Rect::new(area.x, area.y, left_width, area.height);
    let right = Rect::new(area.x + left_width, area.y, right_width, area.height);
    (left, right)
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

    #[test]
    fn split_left_right_does_not_exceed_area() {
        let area = Rect::new(1, 0, 30, 1);
        let (left, right) = split_left_right(area, 20, 42);

        assert_eq!(left.x, 1);
        assert_eq!(left.width, 20);
        assert_eq!(right.x, 21);
        assert_eq!(right.width, 10);
        assert_eq!(right.x + right.width, area.x + area.width);
    }

    #[test]
    fn split_left_right_drops_right_column_when_too_narrow() {
        let area = Rect::new(1, 0, 18, 1);
        let (left, right) = split_left_right(area, 20, 42);

        assert_eq!(left, area);
        assert_eq!(right.width, 0);
        assert_eq!(right.x, area.x + area.width);
    }
}
