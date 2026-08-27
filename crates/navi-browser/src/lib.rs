//! Pluggable browser backend for NAVI.
//!
//! # Built-in backend
//!
//! Feature `cdp-fallback` (default) provides a Chrome/Chromium CDP process
//! engine, or connects to an existing `cdp_url`.
//!
//! # Custom engines
//!
//! Implement [`BrowserEngine`] + [`BrowserEngineFactory`] in an adapter crate and
//! register at process startup:
//!
//! ```ignore
//! navi_browser::set_engine_factory(std::sync::Arc::new(MyFactory));
//! ```
//!
//! See [`INTEGRATION.md`](../INTEGRATION.md) for the full contract.

mod config;
mod engine;
mod engines;
mod factory;
mod session;
mod url_policy;

pub use config::BrowserRuntimeConfig;
pub use engine::{
    BrowserEngine, BrowserEngineFactory, ContentKind, DoctorReport, EngineContext, EngineStatus,
    NavigateResult, SnapshotResult,
};
pub use factory::{
    clear_engine_factory, create_engine, doctor_report, primary_factory, register_fallback_factory,
    resolve_factory, set_engine_factory,
};
pub use session::BrowserSession;
pub use url_policy::{UrlPolicyError, validate_navigation_url};
