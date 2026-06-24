use crate::TuiApp;
use ratatui::layout::Rect;
use ratatui::prelude::Frame;

pub(crate) fn render_setup(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    let _ = (frame, app, area);
}
