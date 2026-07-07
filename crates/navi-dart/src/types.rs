// FFI-safe types, helpers, and shared definitions for navi-dart.

use std::ffi::{CStr, CString, c_void};
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

// ── Callback types ─────────────────────────────────────────────────

/// Async result callback.
///
/// On success: `result_json` is valid, `error` is null.
/// On failure: `result_json` is null, `error` is valid.
pub type NaviAsyncCallback =
    unsafe extern "C" fn(result_json: *const c_char, error: *const c_char, user_data: *mut c_void);

/// Event stream callback.
/// Return 0 to continue, non-zero to stop.
pub type NaviEventCallback =
    unsafe extern "C" fn(event_json: *const c_char, user_data: *mut c_void) -> i32;

// ── Send-safe callback context ─────────────────────────────────────

/// Wraps an async callback and its user_data into a `Send`-safe struct.
///
/// Function pointers are `Send`, and `SendPtr` implements `Send` via
/// `unsafe impl`, so `CallbackCtx` is automatically `Send`. This lets
/// us move the entire context into a `tokio::spawn` future without
/// exposing the raw `*mut c_void` to the Send checker.
pub struct CallbackCtx {
    cb: NaviAsyncCallback,
    ud: SendPtr,
}

impl CallbackCtx {
    pub fn new(callback: NaviAsyncCallback, user_data: *mut c_void) -> Self {
        Self {
            cb: callback,
            ud: SendPtr(user_data),
        }
    }

    /// Calls the callback with a JSON success result.
    pub fn success<T: serde::Serialize>(&self, result: &T) {
        let json = serde_json::to_string(result).unwrap_or_default();
        self.success_str(&json);
    }

    /// Calls the callback with a raw JSON string success result.
    pub fn success_str(&self, json: &str) {
        let c_json = CString::new(json).unwrap_or_default();
        unsafe { (self.cb)(c_json.as_ptr(), ptr::null(), self.ud.0) };
    }

    /// Calls the callback with an error message.
    pub fn error(&self, msg: &str) {
        let c_err = CString::new(msg).unwrap_or_default();
        unsafe { (self.cb)(ptr::null(), c_err.as_ptr(), self.ud.0) };
    }
}

// SAFETY: `cb` is a function pointer (inherently Send + Sync).
// `ud` wraps `*mut c_void` with `unsafe impl Send`.
unsafe impl Send for CallbackCtx {}

/// Wrapper to make raw pointers `Send` for moving into tokio tasks.
pub struct SendPtr(pub *mut c_void);
unsafe impl Send for SendPtr {}

/// Wraps an event callback and its user_data into a `Send`-safe struct.
pub struct EventCtx {
    pub cb: NaviEventCallback,
    pub ud: SendPtr,
}

impl EventCtx {
    pub fn new(callback: NaviEventCallback, user_data: *mut c_void) -> Self {
        Self {
            cb: callback,
            ud: SendPtr(user_data),
        }
    }

    /// Calls the event callback. Returns true if the subscriber wants to stop.
    pub fn emit(&self, event_json: &str) -> bool {
        let c_json = CString::new(event_json).unwrap_or_default();
        unsafe { (self.cb)(c_json.as_ptr(), self.ud.0) != 0 }
    }
}

// SAFETY: same reasoning as CallbackCtx.
unsafe impl Send for EventCtx {}

// ── Helpers ────────────────────────────────────────────────────────

/// Converts a `*const c_char` to a Rust `&str`.
pub unsafe fn cstr_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Serializes a value to JSON and returns the raw C string pointer.
/// Returns null on serialization failure (error set via `set_last_error`).
pub fn to_json_ptr<T: serde::Serialize>(value: &T) -> *mut c_char {
    match serde_json::to_string(value) {
        Ok(json) => CString::new(json).unwrap_or_default().into_raw(),
        Err(e) => {
            set_last_error(&format!("JSON serialization failed: {e}"));
            ptr::null_mut()
        }
    }
}

/// Parses a NaviConfigSaveTarget from an optional string.
pub fn parse_save_target(value: Option<&str>) -> navi_sdk::NaviConfigSaveTarget {
    match value {
        Some("project") => navi_sdk::NaviConfigSaveTarget::Project,
        Some("global") => navi_sdk::NaviConfigSaveTarget::Global,
        Some("none") => navi_sdk::NaviConfigSaveTarget::None,
        _ => navi_sdk::NaviConfigSaveTarget::Auto,
    }
}
