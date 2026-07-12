mod chat;
mod chat_blocks;
pub mod clipboard;
mod ui;

mod app;
mod background;
mod browser;
mod commands;
mod dispatch;
mod errors;
mod event_loop;
mod input;
mod keybindings;
mod mcp_status;
mod mouse;
mod notifications;
pub(crate) mod panels;
mod path_mentions;
mod persistence;
mod plan_review;
mod plugin_approval;
mod plugins;
mod providers;
mod render;
mod runtime;
mod session;
mod settings;
mod state;
mod stream;
mod theme;
mod tools;
mod update_check;
mod usage;
mod view;

pub use self::app::TuiApp;
pub use self::event_loop::{CrosstermInput, InputSource, run, run_loop};

#[doc(hidden)]
pub mod testing;

#[cfg(test)]
pub(crate) mod tests;
