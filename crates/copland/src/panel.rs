//! Dynamic panel system for composable terminal UIs.
//!
//! A [`Panel`] is a self-contained UI region that knows how to render itself
//! and handle key events. Panels are managed by [`PanelManager`] which
//! handles layout, z-ordering, and event routing.
//!
//! There are two kinds of panels:
//! - **Region panels** occupy a fixed area in the main layout (header, chat,
//!   input, sidebar, etc.).
//! - **Overlay panels** float on top — modals, popovers, notifications.
//!
//! This design allows plugins to register custom panels without modifying
//! the host application's render code.

use std::any::Any;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::keymap::KeyOutcome;

/// Size preference for a panel, used by the layout engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSize {
    /// Exact height in rows.
    Fixed(u16),
    /// Minimum height, can grow to fill available space.
    Min(u16),
    /// Flexible — takes whatever space remains after fixed/min panels.
    Flex,
}

/// Context passed to panels during render and key handling.
///
/// This is intentionally trait-based so the host application can provide its
/// own concrete context type with app-specific state (theme, interaction
/// registry, etc.) without copland depending on it.
///
/// Extends `Any` so panels can downcast to the host's concrete context type
/// to access app-specific state.
pub trait PanelContext: Any {
    /// The area available for rendering.
    fn area(&self) -> Rect;

    /// Upcast to `Any` for downcasting.
    fn as_any(&self) -> &dyn Any;
}

/// A self-contained UI panel.
///
/// Implement this trait to create a composable UI region. The host
/// application's `PanelManager` calls `render` and `handle_key` as
/// appropriate.
pub trait Panel: Send + Sync {
    /// Unique identifier for this panel instance.
    fn id(&self) -> &str;

    /// Render the panel into the given area.
    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext);

    /// Handle a key event. Returns `Handled` if the panel consumed the event,
    /// `Ignored` if it should bubble to the next panel, or `Quit` to request
    /// application exit.
    fn handle_key(&self, key: &crossterm::event::KeyEvent, ctx: &dyn PanelContext) -> KeyOutcome {
        let _ = (key, ctx);
        KeyOutcome::Ignored
    }

    /// Preferred size for layout negotiation.
    fn preferred_size(&self) -> PanelSize {
        PanelSize::Flex
    }

    /// Preferred size given the current context. Override this when the panel's
    /// size depends on app state (e.g. dynamic input height). Defaults to
    /// `preferred_size()` for backward compatibility.
    fn preferred_size_in_context(&self, ctx: &dyn PanelContext) -> PanelSize {
        let _ = ctx;
        self.preferred_size()
    }

    /// Whether this panel should be visible in the current state.
    fn is_visible(&self) -> bool {
        true
    }

    /// Z-order for overlapping panels. Higher = on top.
    /// Region panels default to 0, overlay panels should override.
    fn z_order(&self) -> i16 {
        0
    }
}

/// Manages a collection of region and overlay panels.
///
/// Region panels are laid out vertically in registration order. Overlay
/// panels are rendered on top, sorted by z-order, and receive key events
/// first (top-most first).
pub struct PanelManager {
    regions: Vec<Box<dyn Panel>>,
    overlays: Vec<Box<dyn Panel>>,
}

impl Default for PanelManager {
    fn default() -> Self {
        Self {
            regions: Vec::new(),
            overlays: Vec::new(),
        }
    }
}

impl PanelManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a region panel. Regions are laid out top-to-bottom.
    pub fn add_region(&mut self, panel: Box<dyn Panel>) {
        self.regions.push(panel);
    }

    /// Register an overlay panel (modal, popover, notification).
    /// Overlays render on top of regions and receive keys first.
    pub fn add_overlay(&mut self, panel: Box<dyn Panel>) {
        self.overlays.push(panel);
    }

    /// Remove a region panel by id. Returns the removed panel if found.
    pub fn remove_region(&mut self, id: &str) -> Option<Box<dyn Panel>> {
        self.regions
            .iter()
            .position(|p| p.id() == id)
            .map(|i| self.regions.remove(i))
    }

    /// Remove an overlay panel by id. Returns the removed panel if found.
    pub fn remove_overlay(&mut self, id: &str) -> Option<Box<dyn Panel>> {
        self.overlays
            .iter()
            .position(|p| p.id() == id)
            .map(|i| self.overlays.remove(i))
    }

    /// Remove any panel (region or overlay) by id.
    pub fn remove(&mut self, id: &str) -> Option<Box<dyn Panel>> {
        self.remove_overlay(id).or_else(|| self.remove_region(id))
    }

    /// Find a panel by id.
    pub fn get(&self, id: &str) -> Option<&dyn Panel> {
        self.overlays
            .iter()
            .chain(self.regions.iter())
            .find(|p| p.id() == id)
            .map(|p| p.as_ref())
    }

    /// Check if any overlay is active (modal open).
    pub fn has_overlay(&self) -> bool {
        self.overlays.iter().any(|p| p.is_visible())
    }

    /// Iterate visible overlays sorted by z-order (highest first).
    #[cfg(test)]
    fn visible_overlays_sorted(&self) -> Vec<&dyn Panel> {
        let mut visible: Vec<&dyn Panel> = self
            .overlays
            .iter()
            .filter(|p| p.is_visible())
            .map(|p| p.as_ref())
            .collect();
        visible.sort_by_key(|p| std::cmp::Reverse(p.z_order()));
        visible
    }

    /// Sort overlays in-place by z-order (highest first) and return mutable refs.
    fn visible_overlays_mut(&mut self) -> &mut [Box<dyn Panel>] {
        self.overlays
            .sort_by_key(|p| std::cmp::Reverse(p.z_order()));
        &mut self.overlays
    }

    /// Render all panels: regions first (top-to-bottom), then overlays.
    pub fn render(&mut self, frame: &mut Frame, ctx: &dyn PanelContext) {
        self.render_regions(frame, ctx);
        self.render_overlays(frame, ctx);
    }

    /// Render only region panels (top-to-bottom).
    pub fn render_regions(&mut self, frame: &mut Frame, ctx: &dyn PanelContext) {
        let area = ctx.area();
        let region_areas = self.layout_regions(area, ctx);

        for (panel, rect) in self.regions.iter_mut().zip(region_areas.iter()) {
            if panel.is_visible() {
                panel.render(frame, *rect, ctx);
            }
        }
    }

    /// Render only overlay panels (sorted by z-order, highest first).
    pub fn render_overlays(&mut self, frame: &mut Frame, ctx: &dyn PanelContext) {
        let area = ctx.area();

        for overlay in self
            .visible_overlays_mut()
            .iter_mut()
            .filter(|p| p.is_visible())
        {
            // Overlays render into the full area — they position themselves.
            overlay.render(frame, area, ctx);
        }
    }

    /// Route a key event to the top-most visible overlay first, then to
    /// regions in reverse order (bottom-most first).
    pub fn handle_key(
        &mut self,
        key: &crossterm::event::KeyEvent,
        ctx: &dyn PanelContext,
    ) -> KeyOutcome {
        // Overlays get priority (top-most first).
        for overlay in self
            .visible_overlays_mut()
            .iter_mut()
            .filter(|p| p.is_visible())
        {
            let outcome = overlay.handle_key(key, ctx);
            if outcome.is_handled() {
                return outcome;
            }
        }

        // Regions get keys in reverse order (last/bottom-most first).
        for region in self.regions.iter_mut().rev().filter(|p| p.is_visible()) {
            let outcome = region.handle_key(key, ctx);
            if outcome.is_handled() {
                return outcome;
            }
        }

        KeyOutcome::Ignored
    }

    /// Compute vertical layout for region panels based on their preferred sizes.
    ///
    /// Layout algorithm:
    /// 1. Reserve space for `Fixed` panels.
    /// 2. Give `Min` panels their minimum height.
    /// 3. Distribute remaining space among `Min` and `Flex` panels.
    ///    `Min` panels grow first (up to reasonable bounds), then `Flex`.
    fn layout_regions(&self, area: Rect, ctx: &dyn PanelContext) -> Vec<Rect> {
        if self.regions.is_empty() {
            return Vec::new();
        }

        // Collect visible panel info with context-aware sizes.
        let visible: Vec<(bool, PanelSize)> = self
            .regions
            .iter()
            .map(|p| (p.is_visible(), p.preferred_size_in_context(ctx)))
            .collect();

        let mut fixed_total: u16 = 0;
        let mut min_total: u16 = 0;

        for (vis, size) in &visible {
            if !vis {
                continue;
            }
            match size {
                PanelSize::Fixed(h) => fixed_total = fixed_total.saturating_add(*h),
                PanelSize::Min(h) => min_total = min_total.saturating_add(*h),
                PanelSize::Flex => {}
            }
        }

        let available = area.height;
        let after_fixed = available.saturating_sub(fixed_total);
        let after_min = after_fixed.saturating_sub(min_total);

        // Distribute leftover space among Min and Flex panels.
        let flexible_count = visible
            .iter()
            .filter(|(vis, size)| *vis && matches!(size, PanelSize::Min(_) | PanelSize::Flex))
            .count()
            .max(1);

        let extra_each = after_min / flexible_count as u16;
        let mut extra_remainder = after_min % flexible_count as u16;

        let mut rects = Vec::with_capacity(visible.len());
        let mut y = area.y;

        for (vis, size) in &visible {
            if !vis {
                rects.push(Rect::new(area.x, y, area.width, 0));
                continue;
            }
            let height = match size {
                PanelSize::Fixed(h) => *h,
                PanelSize::Min(h) => {
                    let extra = extra_each
                        + if extra_remainder > 0 {
                            extra_remainder -= 1;
                            1
                        } else {
                            0
                        };
                    h.saturating_add(extra)
                }
                PanelSize::Flex => {
                    let extra = extra_each
                        + if extra_remainder > 0 {
                            extra_remainder -= 1;
                            1
                        } else {
                            0
                        };
                    extra
                }
            };
            let height = height.min(area.height.saturating_sub(y.saturating_sub(area.y)));
            rects.push(Rect::new(area.x, y, area.width, height));
            y = y.saturating_add(height);
        }

        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    struct TestCtx {
        area: Rect,
    }

    impl PanelContext for TestCtx {
        fn area(&self) -> Rect {
            self.area
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    struct StubPanel {
        id: String,
        size: PanelSize,
        visible: bool,
        z: i16,
    }

    impl Panel for StubPanel {
        fn id(&self) -> &str {
            &self.id
        }
        fn render(&mut self, _frame: &mut Frame, _area: Rect, _ctx: &dyn PanelContext) {}
        fn preferred_size(&self) -> PanelSize {
            self.size
        }
        fn is_visible(&self) -> bool {
            self.visible
        }
        fn z_order(&self) -> i16 {
            self.z
        }
    }

    #[test]
    fn region_layout_fixed_panels() {
        let mut mgr = PanelManager::new();
        mgr.add_region(Box::new(StubPanel {
            id: "header".into(),
            size: PanelSize::Fixed(1),
            visible: true,
            z: 0,
        }));
        mgr.add_region(Box::new(StubPanel {
            id: "chat".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 0,
        }));
        mgr.add_region(Box::new(StubPanel {
            id: "input".into(),
            size: PanelSize::Fixed(3),
            visible: true,
            z: 0,
        }));

        let ctx = TestCtx {
            area: Rect::new(0, 0, 80, 20),
        };
        let areas = mgr.layout_regions(Rect::new(0, 0, 80, 20), &ctx);
        assert_eq!(areas[0].height, 1);
        assert_eq!(areas[1].height, 16); // 20 - 1 - 3
        assert_eq!(areas[2].height, 3);
    }

    #[test]
    fn region_layout_min_panels_share_extra() {
        let mut mgr = PanelManager::new();
        mgr.add_region(Box::new(StubPanel {
            id: "a".into(),
            size: PanelSize::Fixed(1),
            visible: true,
            z: 0,
        }));
        mgr.add_region(Box::new(StubPanel {
            id: "b".into(),
            size: PanelSize::Min(5),
            visible: true,
            z: 0,
        }));
        mgr.add_region(Box::new(StubPanel {
            id: "c".into(),
            size: PanelSize::Min(5),
            visible: true,
            z: 0,
        }));

        let ctx = TestCtx {
            area: Rect::new(0, 0, 80, 16),
        };
        let areas = mgr.layout_regions(Rect::new(0, 0, 80, 16), &ctx);
        assert_eq!(areas[0].height, 1);
        // 15 remaining, min 5 + 5 = 10, 5 extra → split 3+2
        assert_eq!(areas[1].height, 8); // 5 + 3
        assert_eq!(areas[2].height, 7); // 5 + 2
    }

    #[test]
    fn overlay_priority_by_z_order() {
        let mut mgr = PanelManager::new();
        mgr.add_overlay(Box::new(StubPanel {
            id: "low".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 1,
        }));
        mgr.add_overlay(Box::new(StubPanel {
            id: "high".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 10,
        }));

        let overlays: Vec<&str> = mgr
            .visible_overlays_sorted()
            .iter()
            .map(|p| p.id())
            .collect();
        assert_eq!(overlays, vec!["high", "low"]);
    }

    #[test]
    fn remove_by_id() {
        let mut mgr = PanelManager::new();
        mgr.add_region(Box::new(StubPanel {
            id: "a".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 0,
        }));
        mgr.add_overlay(Box::new(StubPanel {
            id: "b".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 1,
        }));

        assert!(mgr.remove("b").is_some());
        assert!(mgr.remove("a").is_some());
        assert!(mgr.remove("c").is_none());
    }

    #[test]
    fn has_overlay_checks_visibility() {
        let mut mgr = PanelManager::new();
        mgr.add_overlay(Box::new(StubPanel {
            id: "hidden".into(),
            size: PanelSize::Flex,
            visible: false,
            z: 1,
        }));
        assert!(!mgr.has_overlay());

        mgr.add_overlay(Box::new(StubPanel {
            id: "shown".into(),
            size: PanelSize::Flex,
            visible: true,
            z: 1,
        }));
        assert!(mgr.has_overlay());
    }

    struct KeyPanel {
        id: String,
        handled: bool,
    }

    impl Panel for KeyPanel {
        fn id(&self) -> &str {
            &self.id
        }
        fn render(&mut self, _frame: &mut Frame, _area: Rect, _ctx: &dyn PanelContext) {}
        fn handle_key(&self, _key: &KeyEvent, _ctx: &dyn PanelContext) -> KeyOutcome {
            if self.handled {
                KeyOutcome::Handled
            } else {
                KeyOutcome::Ignored
            }
        }
    }

    #[test]
    fn key_routing_overlay_first() {
        let mut mgr = PanelManager::new();
        mgr.add_region(Box::new(KeyPanel {
            id: "region".into(),
            handled: true,
        }));
        mgr.add_overlay(Box::new(KeyPanel {
            id: "overlay".into(),
            handled: true,
        }));

        let ctx = TestCtx {
            area: Rect::new(0, 0, 80, 20),
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let outcome = mgr.handle_key(&key, &ctx);
        assert!(outcome.is_handled());
    }

    #[test]
    fn key_routing_falls_through_to_regions() {
        let mut mgr = PanelManager::new();
        mgr.add_overlay(Box::new(KeyPanel {
            id: "overlay".into(),
            handled: false,
        }));
        mgr.add_region(Box::new(KeyPanel {
            id: "region".into(),
            handled: true,
        }));

        let ctx = TestCtx {
            area: Rect::new(0, 0, 80, 20),
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let outcome = mgr.handle_key(&key, &ctx);
        assert!(outcome.is_handled());
    }

    #[test]
    fn key_routing_ignored_when_nothing_handles() {
        let mut mgr = PanelManager::new();
        mgr.add_overlay(Box::new(KeyPanel {
            id: "overlay".into(),
            handled: false,
        }));
        mgr.add_region(Box::new(KeyPanel {
            id: "region".into(),
            handled: false,
        }));

        let ctx = TestCtx {
            area: Rect::new(0, 0, 80, 20),
        };
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let outcome = mgr.handle_key(&key, &ctx);
        assert!(!outcome.is_handled());
    }
}
