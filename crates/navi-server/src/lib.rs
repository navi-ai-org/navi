#![recursion_limit = "512"]
mod routes;
mod server;
mod state;
mod web;

#[cfg(test)]
mod http_tests;

pub use server::NaviServer;
pub use state::NaviServerConfig;
pub use web::{AssetSource, web_filter};
