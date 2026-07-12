#![recursion_limit = "512"]
mod routes;
mod server;
mod state;

#[cfg(test)]
mod http_tests;

pub use server::NaviServer;
pub use state::NaviServerConfig;
