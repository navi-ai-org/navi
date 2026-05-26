mod chat;
mod ui;

mod app;
mod commands;
mod dispatch;
mod errors;
mod event_loop;
mod input;
mod keybindings;
mod mouse;
mod notifications;
mod persistence;
mod providers;
mod render;
mod runtime;
mod session;
mod state;
mod stream;
mod theme;
mod tools;
mod view;

pub use self::app::TuiApp;
pub use self::event_loop::run;

#[cfg(test)]
mod tests;
