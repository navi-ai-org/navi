mod chat;
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
mod mouse;
mod notifications;
mod panels;
mod persistence;
mod plugin_approval;
mod plugins;
mod providers;
mod render;
mod runtime;
mod session;
mod state;
mod stream;
mod theme;
mod tools;
mod usage;
mod view;

pub use self::app::TuiApp;
pub use self::event_loop::{CrosstermInput, InputSource, run, run_loop};

#[doc(hidden)]
pub mod testing;

#[cfg(test)]
pub(crate) mod tests;
