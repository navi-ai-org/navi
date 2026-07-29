//! Built-in engine implementations.
//!
//! CDP is the only built-in backend. External engines can still be registered
//! via [`crate::set_engine_factory`].

#[cfg(feature = "cdp-fallback")]
pub mod cdp;
