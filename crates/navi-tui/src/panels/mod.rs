//! Bridge between copland's Panel system and navi-tui's existing render functions.
//!
//! This module provides:
//! - [`NaviPanelContext`]: a concrete `PanelContext` that gives panels access
//! to `&mut TuiApp` (safe because the TUI event loop is single-threaded).
//! - [`ModalPanel`]: a Panel wrapper for existing render functions, enabling
//! incremental migration from the hardcoded `render_inner` match to the
//! dynamic PanelManager.

use std::any::Any;
use std::sync::Mutex;

use copland::keymap::KeyOutcome;
use copland::panel::{Panel, PanelContext, PanelSize};
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::TuiApp;
use crate::state::Mode;
use crate::view;
use crate::view::setup;

pub(crate) mod regions;

/// Register all existing modals as overlay panels in the PanelManager.
///
/// This is the bridge step: instead of a hardcoded match in render_inner,
/// each modal is a ModalPanel overlay that only renders when its Mode is active.
/// The PanelManager handles z-ordering and rendering order.
///
/// Key handling still goes through the existing keybinding system for now.
/// This will be migrated panel-by-panel in subsequent steps.
pub fn register_modal_panels(app: &mut TuiApp) {
    use view::background_commands;
    use view::command_palette;
    use view::debug;
    use view::help;
    use view::modals;
    use view::model_picker;
    use view::plugins;
    use view::provider_settings;
    use view::sessions;
    use view::skills;

    let pm = &mut app.panel_manager;

    // Command palette
    pm.add_overlay(Box::new(ModalPanel::new(
        "command-palette",
        Mode::Commands,
        command_palette::render,
        64,
        14,
    )));
    // @ path mentions
    pm.add_overlay(Box::new(ModalPanel::new(
        "path-mentions",
        Mode::PathMentions,
        modals::render_path_mentions,
        72,
        16,
    )));
    // Model picker
    pm.add_overlay(Box::new(ModalPanel::new(
        "model-picker",
        Mode::Models,
        model_picker::render,
        72,
        22,
    )));
    // API key entry
    pm.add_overlay(Box::new(ModalPanel::new(
        "api-key-entry",
        Mode::ApiKeyEntry,
        modals::render_api_key_entry,
        72,
        11,
    )));
    // Thinking picker
    pm.add_overlay(Box::new(ModalPanel::new(
        "thinking-picker",
        Mode::Thinking,
        modals::render_thinking_picker,
        40,
        10,
    )));
    // Sessions
    pm.add_overlay(Box::new(ModalPanel::new(
        "sessions",
        Mode::Sessions,
        sessions::render,
        72,
        16,
    )));
    // Settings hub (sectioned list of toggles + deep-links)
    pm.add_overlay(Box::new(ModalPanel::new(
        "settings",
        Mode::Settings,
        modals::render_settings,
        76,
        28,
    )));
    // Providers
    pm.add_overlay(Box::new(ModalPanel::new(
        "providers",
        Mode::Providers,
        provider_settings::render,
        110,
        26,
    )));
    // Usage
    pm.add_overlay(Box::new(ModalPanel::new(
        "usage",
        Mode::Usage,
        modals::render_usage,
        78,
        18,
    )));
    // Debug
    pm.add_overlay(Box::new(ModalPanel::new(
        "debug",
        Mode::Debug,
        debug::render,
        76,
        18,
    )));
    // Help — keyboard cheatsheet (sectioned + scrollable)
    pm.add_overlay(Box::new(ModalPanel::new(
        "help",
        Mode::Help,
        help::render,
        78,
        24,
    )));
    // About NAVI
    pm.add_overlay(Box::new(ModalPanel::new(
        "about",
        Mode::About,
        view::about::render,
        72,
        16,
    )));
    // Self-update available
    pm.add_overlay(Box::new(ModalPanel::new(
        "update-available",
        Mode::UpdateAvailable,
        view::update_modal::render,
        72,
        16,
    )));
    // Skills
    pm.add_overlay(Box::new(ModalPanel::new(
        "skills",
        Mode::Skills,
        skills::render,
        72,
        20,
    )));
    // Plugins
    pm.add_overlay(Box::new(ModalPanel::new(
        "plugins",
        Mode::Plugins,
        plugins::render,
        76,
        22,
    )));
    // Plugin approval
    pm.add_overlay(Box::new(ModalPanel::new(
        "plugin-approval",
        Mode::PluginApproval,
        modals::render_plugin_approval,
        84,
        24,
    )));
    // Question
    pm.add_overlay(Box::new(ModalPanel::new(
        "question",
        Mode::Question,
        modals::render_question,
        78,
        22,
    )));
    // Theme picker
    pm.add_overlay(Box::new(ModalPanel::new(
        "theme-picker",
        Mode::ThemePicker,
        modals::render_theme_picker,
        40,
        12,
    )));
    // Message actions
    pm.add_overlay(Box::new(ModalPanel::new(
        "message-actions",
        Mode::MessageActions,
        modals::render_message_actions,
        58,
        10,
    )));
    // OAuth
    pm.add_overlay(Box::new(ModalPanel::new(
        "oauth",
        Mode::OAuth,
        modals::render_oauth,
        78,
        12,
    )));
    // Background commands
    pm.add_overlay(Box::new(ModalPanel::new(
        "background-commands",
        Mode::BackgroundCommands,
        background_commands::render,
        80,
        20,
    )));
    // Background command output
    pm.add_overlay(Box::new(ModalPanel::new(
        "bg-cmd-output",
        Mode::BackgroundCommandOutput,
        background_commands::render_output,
        110,
        30,
    )));

    // Unified model routing (Chat / Agents / Attachments)
    pm.add_overlay(Box::new(ModalPanel::new(
        "model-routing",
        Mode::ModelRouting,
        modals::render_model_routing,
        78,
        18,
    )));
    // Extensions hub
    pm.add_overlay(Box::new(ModalPanel::new(
        "extensions",
        Mode::Extensions,
        modals::render_extensions_hub,
        72,
        12,
    )));
    // Background models
    pm.add_overlay(Box::new(ModalPanel::new(
        "background-models",
        Mode::BackgroundModels,
        modals::render_background_models,
        70,
        14,
    )));
    // Bg model picker (reuses model_picker::render)
    pm.add_overlay(Box::new(ModalPanel::new(
        "bg-model-picker",
        Mode::BgModelPicker,
        model_picker::render,
        72,
        22,
    )));
    // Attachment models
    pm.add_overlay(Box::new(ModalPanel::new(
        "attachment-models",
        Mode::AttachmentModels,
        modals::render_attachment_models,
        70,
        12,
    )));
    // Message queue
    pm.add_overlay(Box::new(ModalPanel::new(
        "message-queue",
        Mode::MessageQueue,
        modals::render_message_queue,
        72,
        18,
    )));
    // Queued message edit
    pm.add_overlay(Box::new(ModalPanel::new(
        "queued-msg-edit",
        Mode::QueuedMessageEdit,
        modals::render_queued_message_edit,
        76,
        20,
    )));
    // Confirm cancel turn
    pm.add_overlay(Box::new(ModalPanel::new(
        "confirm-cancel",
        Mode::ConfirmCancelTurn,
        modals::render_confirm_cancel_turn,
        62,
        9,
    )));
    // Confirm / review plan (needs &mut for mouse hits)
    pm.add_overlay(Box::new(ModalPanelMut::new(
        "confirm-plan",
        Mode::ConfirmPlan,
        modals::render_confirm_plan,
        78,
        22,
    )));
    // Sudo password (masked)
    pm.add_overlay(Box::new(ModalPanel::new(
        "sudo-password",
        Mode::SudoPassword,
        modals::render_sudo_password,
        64,
        12,
    )));

    // --- Special-case modals that need &mut TuiApp ---
    // MCP modal needs &mut TuiApp and a theme palette reference.
    pm.add_overlay(Box::new(ModalPanelMut::new(
        "mcp",
        Mode::Mcp,
        |frame, app, area| {
            crate::ui::mcp::draw_mcp_modal(frame, area, app);
        },
        86,
        20,
    )));
    // Setup modal needs &mut TuiApp and full content area.
    pm.add_overlay(Box::new(ModalPanelMut::new_with_area(
        "setup",
        Mode::Setup,
        |frame, app, area| {
            setup::render_setup(frame, app, area);
        },
    )));
}

/// Render all region panels (header, chat, input, etc.) via the PanelManager.
pub fn render_regions(frame: &mut Frame, app: &mut TuiApp, area: Rect) {
    let ctx = NaviPanelContext::new(app, area);
    app.panel_manager.render_regions(frame, &ctx);
}

/// Render all overlay panels (modals, plugin panels) via the PanelManager.
pub fn render_overlays(frame: &mut Frame, app: &mut TuiApp, area: Rect) {
    let ctx = NaviPanelContext::new(app, area);
    app.panel_manager.render_overlays(frame, &ctx);
}

/// Concrete PanelContext for navi-tui.
///
/// Carries a mutable pointer to TuiApp. This is safe because:
/// 1. The TUI event loop is single-threaded and synchronous.
/// 2. The context is created fresh each frame and discarded after render.
/// 3. Only one panel renders at a time (PanelManager iterates sequentially).
pub struct NaviPanelContext {
    app: *mut TuiApp,
    area: Rect,
}

impl NaviPanelContext {
    pub fn new(app: &mut TuiApp, area: Rect) -> Self {
        Self {
            app: app as *mut TuiApp,
            area,
        }
    }

    /// Get mutable access to TuiApp. Safe within the single-threaded render loop.
    #[allow(dead_code)]
    pub fn app_mut(&self) -> &mut TuiApp {
        unsafe { &mut *self.app }
    }

    /// Get immutable access to TuiApp.
    pub fn app(&self) -> &TuiApp {
        unsafe { &*self.app }
    }
}

impl PanelContext for NaviPanelContext {
    fn area(&self) -> Rect {
        self.area
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Type alias for a render function that draws a modal into a given area.
type ModalRenderFn = fn(&mut Frame, &TuiApp, Rect);

/// A Panel wrapper for an existing modal render function.
///
/// This allows incremental migration: each modal's render function is
/// wrapped in a `ModalPanel` and registered with the `PanelManager` as
/// an overlay. The panel is only visible when the corresponding `Mode`
/// is active.
pub struct ModalPanel {
    id: String,
    mode: Mode,
    render_fn: ModalRenderFn,
    modal_width: u16,
    modal_height: u16,
    z: i16,
}

impl ModalPanel {
    pub fn new(id: &str, mode: Mode, render_fn: ModalRenderFn, width: u16, height: u16) -> Self {
        Self {
            id: id.to_string(),
            mode,
            render_fn,
            modal_width: width,
            modal_height: height,
            z: 10,
        }
    }

    #[allow(dead_code)]
    pub fn with_z(mut self, z: i16) -> Self {
        self.z = z;
        self
    }

    fn is_current_mode(&self, app: &TuiApp) -> bool {
        app.mode == self.mode
    }

    fn modal_rect(&self, area: Rect) -> Rect {
        crate::render::modal_rect(area, self.modal_width, self.modal_height)
    }
}

impl Panel for ModalPanel {
    fn id(&self) -> &str {
        &self.id
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let navi_ctx = ctx
            .as_any()
            .downcast_ref::<NaviPanelContext>()
            .expect("ModalPanel requires NaviPanelContext");
        if !self.is_current_mode(navi_ctx.app()) {
            return;
        }
        let modal_area = self.modal_rect(area);
        (self.render_fn)(frame, navi_ctx.app(), modal_area);
    }

    fn handle_key(&self, _key: &KeyEvent, ctx: &dyn PanelContext) -> KeyOutcome {
        let navi_ctx = ctx
            .as_any()
            .downcast_ref::<NaviPanelContext>()
            .expect("ModalPanel requires NaviPanelContext");
        if !self.is_current_mode(navi_ctx.app()) {
            return KeyOutcome::Ignored;
        }
        // Key handling is still done by the existing keybinding system.
        // ModalPanel is render-only for now.
        KeyOutcome::Ignored
    }

    fn preferred_size(&self) -> PanelSize {
        PanelSize::Flex
    }

    fn is_visible(&self) -> bool {
        // Visibility is checked at render time against the current mode,
        // because the PanelManager doesn't have access to TuiApp here.
        // We return true and short-circuit in render if the mode doesn't match.
        true
    }

    fn z_order(&self) -> i16 {
        self.z
    }
}

/// Type alias for a render function that draws a modal into a given area,
/// with mutable access to TuiApp.
type ModalRenderFnMut = fn(&mut Frame, &mut TuiApp, Rect);

/// A Panel wrapper for modals that need `&mut TuiApp`.
///
/// Like `ModalPanel`, but the render function receives `&mut TuiApp` instead
/// of `&TuiApp`. Used for modals like MCP and Setup that need mutable access.
pub struct ModalPanelMut {
    id: String,
    mode: Mode,
    render_fn: ModalRenderFnMut,
    modal_width: u16,
    modal_height: u16,
    use_full_area: bool,
    z: i16,
}

impl ModalPanelMut {
    pub fn new(id: &str, mode: Mode, render_fn: ModalRenderFnMut, width: u16, height: u16) -> Self {
        Self {
            id: id.to_string(),
            mode,
            render_fn,
            modal_width: width,
            modal_height: height,
            use_full_area: false,
            z: 10,
        }
    }

    /// Use the full content area instead of a centered modal rect.
    pub fn new_with_area(id: &str, mode: Mode, render_fn: ModalRenderFnMut) -> Self {
        Self {
            id: id.to_string(),
            mode,
            render_fn,
            modal_width: 0,
            modal_height: 0,
            use_full_area: true,
            z: 10,
        }
    }
}

impl Panel for ModalPanelMut {
    fn id(&self) -> &str {
        &self.id
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        let navi_ctx = ctx
            .as_any()
            .downcast_ref::<NaviPanelContext>()
            .expect("ModalPanelMut requires NaviPanelContext");
        if navi_ctx.app().mode != self.mode {
            return;
        }
        let render_area = if self.use_full_area {
            area
        } else {
            crate::render::modal_rect(area, self.modal_width, self.modal_height)
        };
        (self.render_fn)(frame, navi_ctx.app_mut(), render_area);
    }

    fn handle_key(&self, _key: &KeyEvent, ctx: &dyn PanelContext) -> KeyOutcome {
        let navi_ctx = ctx
            .as_any()
            .downcast_ref::<NaviPanelContext>()
            .expect("ModalPanelMut requires NaviPanelContext");
        if navi_ctx.app().mode != self.mode {
            return KeyOutcome::Ignored;
        }
        KeyOutcome::Ignored
    }

    fn preferred_size(&self) -> PanelSize {
        PanelSize::Flex
    }

    fn is_visible(&self) -> bool {
        true
    }

    fn z_order(&self) -> i16 {
        self.z
    }
}

/// Adapter that wraps a plugin's `TuiComponent` as a copland `Panel`.
///
/// This is the bridge between the plugin API's `TuiComponent` trait and
/// copland's `Panel` trait, allowing plugin-registered components to be
/// rendered by the `PanelManager`.
pub struct PluginPanelAdapter {
    id: String,
    inner: Mutex<Box<dyn navi_plugin_api::TuiComponent>>,
}

impl PluginPanelAdapter {
    pub fn new(component: Box<dyn navi_plugin_api::TuiComponent>) -> Self {
        let id = component.id().to_string();
        Self {
            id,
            inner: Mutex::new(component),
        }
    }
}

impl Panel for PluginPanelAdapter {
    fn id(&self) -> &str {
        &self.id
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &dyn PanelContext) {
        self.inner
            .lock()
            .expect("PluginPanelAdapter mutex poisoned")
            .render(frame, area, ctx);
    }

    fn handle_key(&self, key: &KeyEvent, ctx: &dyn PanelContext) -> KeyOutcome {
        // Use interior mutability: Panel::handle_key takes &self, but
        // TuiComponent::handle_key takes &mut self. The Mutex lets us
        // get &mut access. This is safe because the TUI event loop is
        // single-threaded — the mutex is never contended.
        let mut inner = self
            .inner
            .lock()
            .expect("PluginPanelAdapter mutex poisoned");
        inner.handle_key(key, ctx)
    }

    fn preferred_size(&self) -> PanelSize {
        self.inner
            .lock()
            .expect("PluginPanelAdapter mutex poisoned")
            .preferred_size()
    }

    fn is_visible(&self) -> bool {
        self.inner
            .lock()
            .expect("PluginPanelAdapter mutex poisoned")
            .is_visible()
    }

    fn z_order(&self) -> i16 {
        self.inner
            .lock()
            .expect("PluginPanelAdapter mutex poisoned")
            .z_order()
    }
}

/// Load TUI component panels from native plugins and register them
/// with the TUI's `PanelManager`.
///
/// This should be called after the session is started. It calls
/// `NaviEngine::take_tui_panels` to get plugin-registered components
/// and wraps them in `PluginPanelAdapter` for the `PanelManager`.
pub fn load_plugin_panels(app: &mut TuiApp) {
    if app.session_id.as_str().is_empty() {
        return;
    }
    let engine = app.engine();
    if let Ok(panels) = engine.take_tui_panels(app.session_id.as_str()) {
        for component in panels {
            app.panel_manager
                .add_overlay(Box::new(PluginPanelAdapter::new(component)));
        }
    }
}
