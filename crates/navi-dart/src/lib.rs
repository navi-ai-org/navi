// navi-dart: Dart FFI bindings for the NAVI agent runtime SDK.
//
// Exposes a C-compatible ABI (`extern "C"`) that dart:ffi can consume.
// Uses opaque pointers for engine handles and JSON strings for complex data.
// Async operations use callback function pointers for cross-thread notification.

mod engine;
mod event_stream;
mod surface;
mod types;

// Re-export the C ABI surface
pub use engine::*;
pub use event_stream::*;
pub use surface::*;
pub use types::*;
