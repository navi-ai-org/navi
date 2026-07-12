//! Built-in engine implementations.
//!
//! Prefer registering an external CloakBrowser Rust binding via
//! [`crate::set_engine_factory`]. CDP is only a temporary fallback.

#[cfg(feature = "cdp-fallback")]
pub mod cdp;

#[cfg(feature = "cloakbrowser")]
pub mod cloakbrowser;
