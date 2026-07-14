//! Region panels for the main TUI layout.
//!
//! These panels replace the hardcoded render calls in `render_inner` with
//! composable copland `Panel` implementations that the `PanelManager` lays
//! out and renders dynamically.

use copland::panel::{Panel, PanelContext, PanelSize};
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::TuiApp;
use crate::view;

use super::NaviPanelContext;

/// Helper to downcast ctx and get (&TuiApp, &mut TuiApp) pair.
fn app_refs<'a>(ctx: &'a dyn PanelContext) -> (&'a TuiApp, &'a mut TuiApp) {
    let navi_ctx = ctx
        .as_any()
        .downcast_ref::<NaviPanelContext>()
        .expect("Region panels require NaviPanelContext");
    (navi_ctx.app(), navi_ctx.app_mut())
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

pub struct HeaderPanel;

impl Panel for HeaderPanel {
    fn id(&self) -> &str {
        "header"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (app, _) = app_refs(ctx);
        view::render_header(frame, app, area);
    }

    fn preferred_size(&self) -> PanelSize {
        PanelSize::Fixed(1)
    }
}

// ---------------------------------------------------------------------------
// Plan topbar (under header, above chat — Grok-style N/M chip)
// ---------------------------------------------------------------------------

pub struct PlanTopbarPanel;

impl Panel for PlanTopbarPanel {
    fn id(&self) -> &str {
        "plan-topbar"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (_, app) = app_refs(ctx);
        view::plan_topbar::render_plan_topbar(frame, app, area);
    }

    fn preferred_size_in_context(&self, ctx: &dyn PanelContext) -> PanelSize {
        let (app, _) = app_refs(ctx);
        PanelSize::Fixed(view::plan_topbar::plan_topbar_height(app))
    }
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

pub struct ChatPanel;

impl Panel for ChatPanel {
    fn id(&self) -> &str {
        "chat"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (_, app) = app_refs(ctx);
        view::chat::render_chat_area(frame, app, area);
    }

    fn preferred_size(&self) -> PanelSize {
        PanelSize::Flex
    }
}

// ---------------------------------------------------------------------------
// Input activity
// ---------------------------------------------------------------------------

pub struct InputActivityPanel;

impl Panel for InputActivityPanel {
    fn id(&self) -> &str {
        "input-activity"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (_, app) = app_refs(ctx);
        view::input::render_input_activity(frame, app, area);
    }

    fn preferred_size_in_context(&self, ctx: &dyn PanelContext) -> PanelSize {
        let (app, _) = app_refs(ctx);
        PanelSize::Fixed(view::input::composer_activity_height(app))
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

pub struct InputPanel;

impl Panel for InputPanel {
    fn id(&self) -> &str {
        "input"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (_, app) = app_refs(ctx);
        view::input::render_input(frame, app, area);
    }

    fn preferred_size_in_context(&self, ctx: &dyn PanelContext) -> PanelSize {
        let (app, _) = app_refs(ctx);
        let content_area = ctx.area();
        let input_width = content_area.width.saturating_sub(4) as usize;
        // Height follows animated expand/collapse (min: 1 content + borders + meta = 4).
        let h = view::input::composer_height(app, input_width);
        // On tiny viewports, still allow growth but cap total composer share.
        let max_for_viewport = (content_area.height / 2).max(4);
        PanelSize::Fixed(h.clamp(4, max_for_viewport))
    }
}

// ---------------------------------------------------------------------------
// Input hint
// ---------------------------------------------------------------------------

pub struct InputHintPanel;

impl Panel for InputHintPanel {
    fn id(&self) -> &str {
        "input-hint"
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let (_, app) = app_refs(ctx);
        view::input::render_input_hint(frame, app, area);
    }

    fn preferred_size_in_context(&self, ctx: &dyn PanelContext) -> PanelSize {
        let (app, _) = app_refs(ctx);
        let content_area = ctx.area();
        let compact = content_area.width < 64 || content_area.height < 18;
        let h = if compact {
            0
        } else {
            view::input::composer_hint_height(app)
        };
        PanelSize::Fixed(h)
    }
}

/// Register all region panels with the PanelManager.
pub fn register_region_panels(app: &mut TuiApp) {
    let pm = &mut app.panel_manager;
    pm.add_region(Box::new(HeaderPanel));
    pm.add_region(Box::new(PlanTopbarPanel));
    pm.add_region(Box::new(ChatPanel));
    pm.add_region(Box::new(InputActivityPanel));
    pm.add_region(Box::new(InputPanel));
    pm.add_region(Box::new(InputHintPanel));
}
