// Re-export copland framework primitives so existing `crate::ui::*` imports
// continue to work during the incremental migration to the Panel system.
pub use copland::effect::UiEffect;
pub use copland::keymap::KeyOutcome;
pub use copland::layout::{ModalSpec, RootLayoutHeights, root_layout, viewport_rect};
pub use copland::list::SelectListState;
pub use copland::modal::ModalStack;
pub use copland::text_input::{
    TextInputRef, TextInputRenderSpec, floor_char_boundary, next_char_boundary,
    render_text_input_line,
};

// Re-exports only needed by tests.
#[cfg(test)]
pub use copland::text_input::{next_hump_boundary, previous_char_boundary, previous_hump_boundary};

// Navi-specific modules that depend on TuiApp or application-specific types.
pub(crate) mod interaction;
pub(crate) mod list;
pub(crate) mod mcp;
