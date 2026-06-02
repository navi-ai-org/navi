/// Canonical WIT definitions for the NAVI plugin interface.
///
/// This module exposes the raw WIT text for tooling and documentation.
/// It does not generate code; for that, use `wit-bindgen` in the future
/// when Component Model support is added to the runtime.

/// The canonical WIT interface definition for `navi-plugin`.
pub const WIT_SOURCE: &str = include_str!("../wit/navi-plugin.wit");
