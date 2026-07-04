// FFI-safe types, helpers, and shared definitions for navi-dart.

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

// ── Thread-local error ─────────────────────────────────────────────

thread_local! {
    static LAST_ERROR: Mutex<Option<CString>> = const { Mutex::new(None) };
}

pub(crate) fn set_last_error(msg: &str) {
    LAST_ERROR.with(|e| {
        *e.lock().unwrap() = CString::new(msg).ok();
    });
}

/// Returns a pointer to the last error message on the calling thread,
/// or null if no error occurred since the last call.
///
/// The pointer is valid until the next FFI call on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_last_error() -> *const c_char {
    LAST_ERROR.with(|e| {
        let lock = e.lock().unwrap();
        match lock.as_ref() {
            Some(cstr) => cstr.as_ptr(),
            None => ptr::null(),
        }
    })
}

/// Frees a C string allocated by this library.
///
/// Passing a pointer not allocated by `navi-dart` is undefined behavior.
/// Passing null is a safe no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

// ── Callback types ─────────────────────────────────────────────────

/// Async result callback.
///
/// On success: `result_json` is a valid C string, `error` is null.
/// On failure: `result_json` is null, `error` is a valid C string.
/// Pointers are only valid during the callback invocation.
/// Must be safe to call from any thread (Dart's `NativeCallable.listener` is).
pub type NaviAsyncCallback =
    unsafe extern "C" fn(result_json: *const c_char, error: *const c_char, user_data: *mut c_void);

/// Event stream callback.
///
/// Called for each event. `event_json` is the serialized RuntimeEvent.
/// Return 0 to continue, non-zero to stop the subscription.
pub type NaviEventCallback =
    unsafe extern "C" fn(event_json: *const c_char, user_data: *mut c_void) -> i32;

// ── Send wrapper for user_data ─────────────────────────────────────

/// Wrapper to make raw pointers `Send` for moving into tokio tasks.
///
/// Safety: the caller must ensure the pointed-to data remains valid
/// for the duration of the async operation.
pub(crate) struct SendPtr(pub *mut c_void);
unsafe impl Send for SendPtr {}

// ── Helpers ────────────────────────────────────────────────────────

/// Converts a `*const c_char` to a Rust `&str`. Returns null pointer on null input.
pub(crate) unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Allocates a `CString` from a Rust string and returns the raw pointer.
/// The caller must free it with `navi_string_free`.
pub(crate) fn string_to_c(s: String) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// Serializes a value to JSON and returns the raw C string pointer.
/// Returns null on serialization failure (error is set via `set_last_error`).
pub(crate) fn to_json_ptr<T: serde::Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => string_to_c(json),
        Err(e) => {
            set_last_error(&format!("JSON serialization failed: {e}"));
            ptr::null_mut()
        }
    }
}

/// Calls an async callback with success.
///
/// Safety: `callback` must be a valid function pointer for the duration of this call.
pub(crate) unsafe fn call_success(callback: NaviAsyncCallback, json: &str, user_data: *mut c_void) {
    let c_json = CString::new(json).unwrap_or_default();
    unsafe { callback(c_json.as_ptr(), ptr::null(), user_data) };
}

/// Calls an async callback with an error.
///
/// Safety: `callback` must be a valid function pointer for the duration of this call.
pub(crate) unsafe fn call_error(callback: NaviAsyncCallback, msg: &str, user_data: *mut c_void) {
    let c_err = CString::new(msg).unwrap_or_default();
    unsafe { callback(ptr::null(), c_err.as_ptr(), user_data) };
}

/// Serializes a value and calls the async callback with the result.
pub(crate) unsafe fn reply_json<T: serde::Serialize>(
    callback: NaviAsyncCallback,
    result: &T,
    user_data: *mut c_void,
) {
    match serde_json::to_string(result) {
        Ok(json) => unsafe { call_success(callback, &json, user_data) },
        Err(e) => unsafe { call_error(callback, &format!("JSON serialization failed: {e}"), user_data) },
    }
}

/// Parses a NaviConfigSaveTarget from an optional string.
pub(crate) fn parse_save_target(value: Option<&str>) -> navi_sdk::NaviConfigSaveTarget {
    match value {
        Some("project") => navi_sdk::NaviConfigSaveTarget::Project,
        Some("global") => navi_sdk::NaviConfigSaveTarget::Global,
        Some("none") => navi_sdk::NaviConfigSaveTarget::None,
        _ => navi_sdk::NaviConfigSaveTarget::Auto,
    }
}
