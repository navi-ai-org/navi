//! HTTP route modules for navi-server.
//!
//! Each submodule exports a `routes(state, secret)` function returning a
//! boxed warp filter so domains can be developed and compiled independently.

mod auth;
mod memory;
mod plugins;
mod registry_models;
mod session_ops;
mod skills_mcp;
mod voice;

use crate::state::SharedState;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::reply::Reply;

/// Combine all domain route modules into a single boxed filter.
pub fn all_routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    auth::routes(state.clone(), secret)
        .or(memory::routes(state.clone(), secret))
        .or(voice::routes(state.clone(), secret))
        .or(plugins::routes(state.clone(), secret))
        .or(session_ops::routes(state.clone(), secret))
        .or(skills_mcp::routes(state.clone(), secret))
        .or(registry_models::routes(state, secret))
        .boxed()
}
