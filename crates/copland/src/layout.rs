use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub const VIEWPORT_HORIZONTAL_MARGIN: u16 = 1;

pub fn viewport_rect(area: Rect) -> Rect {
    inset_rect(area, VIEWPORT_HORIZONTAL_MARGIN, 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootLayoutHeights {
    pub header: u16,
    pub image_preview: u16,
    pub input_activity: u16,
    pub input: u16,
    pub input_hint: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootLayout {
    pub header: Rect,
    pub chat: Rect,
    pub image_preview: Rect,
    pub input_activity: Rect,
    pub input: Rect,
    pub input_hint: Rect,
}

pub fn root_layout(area: Rect, heights: RootLayoutHeights) -> RootLayout {
    let header_height = heights.header.min(area.height);
    let bottom_requested = heights
        .image_preview
        .saturating_add(heights.input_activity)
        .saturating_add(heights.input)
        .saturating_add(heights.input_hint);
    let remaining_after_header = area.height.saturating_sub(header_height);
    let bottom_height = bottom_requested.min(remaining_after_header);
    let chat_height = remaining_after_header.saturating_sub(bottom_height);

    let mut y = area.y;
    let header = Rect::new(area.x, y, area.width, header_height);
    y = y.saturating_add(header_height);
    let chat = Rect::new(area.x, y, area.width, chat_height);
    y = y.saturating_add(chat_height);

    let mut remaining_bottom = bottom_height;
    let image_preview = take_vertical(
        area.x,
        area.width,
        &mut y,
        &mut remaining_bottom,
        heights.image_preview,
    );
    let input_activity = take_vertical(
        area.x,
        area.width,
        &mut y,
        &mut remaining_bottom,
        heights.input_activity,
    );
    let input = take_vertical(
        area.x,
        area.width,
        &mut y,
        &mut remaining_bottom,
        heights.input,
    );
    let input_hint = take_vertical(
        area.x,
        area.width,
        &mut y,
        &mut remaining_bottom,
        heights.input_hint,
    );

    RootLayout {
        header,
        chat,
        image_preview,
        input_activity,
        input,
        input_hint,
    }
}

fn take_vertical(
    x: u16,
    width: u16,
    y: &mut u16,
    remaining: &mut u16,
    requested_height: u16,
) -> Rect {
    let height = requested_height.min(*remaining);
    let rect = Rect::new(x, *y, width, height);
    *y = y.saturating_add(height);
    *remaining = remaining.saturating_sub(height);
    rect
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
pub struct ModalSpec {
    pub max_width: u16,
    pub height: u16,
    pub min_width: u16,
    pub min_height: u16,
    pub horizontal_margin: u16,
    pub vertical_margin: u16,
}

impl ModalSpec {
    pub fn fixed(max_width: u16, height: u16) -> Self {
        Self {
            max_width,
            height,
            min_width: 40,
            min_height: 10,
            horizontal_margin: 8,
            vertical_margin: 4,
        }
    }

    pub fn rect(self, area: Rect) -> Rect {
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

pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
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
    fn root_layout_never_overlaps_chat_and_composer() {
        let area = Rect::new(1, 0, 98, 12);
        let layout = root_layout(
            area,
            RootLayoutHeights {
                header: 1,
                image_preview: 0,
                input_activity: 3,
                input: 5,
                input_hint: 1,
            },
        );

        assert_eq!(layout.header.y, 0);
        assert_eq!(layout.header.height, 1);
        assert_eq!(layout.chat.y, 1);
        assert_eq!(layout.chat.height, 2);
        assert_eq!(layout.input_activity.y, 3);
        assert_eq!(layout.input.y, 6);
        assert_eq!(layout.input_hint.y, 11);
        assert_eq!(
            layout.input_hint.y + layout.input_hint.height,
            area.y + area.height
        );
    }

    #[test]
    fn root_layout_clips_footer_stack_inside_tiny_viewport() {
        let area = Rect::new(0, 0, 80, 4);
        let layout = root_layout(
            area,
            RootLayoutHeights {
                header: 1,
                image_preview: 0,
                input_activity: 3,
                input: 5,
                input_hint: 1,
            },
        );

        assert_eq!(layout.header.height, 1);
        assert_eq!(layout.chat.height, 0);
        assert_eq!(layout.input_activity.height, 3);
        assert_eq!(layout.input.height, 0);
        assert_eq!(layout.input_hint.height, 0);
        assert_eq!(
            layout.input_activity.y + layout.input_activity.height,
            area.y + area.height
        );
    }
}
