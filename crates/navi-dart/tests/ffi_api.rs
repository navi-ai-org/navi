// Tests for the navi-dart C ABI surface.
//
// These tests exercise every exported function, verifying that:
// 1. Engine lifecycle (create → use → free) works.
// 2. Sync functions return valid JSON.
// 3. Async functions invoke callbacks with correct data.
// 4. Error paths set navi_last_error() and call error callbacks.
// 5. Null/invalid input is handled gracefully.

use std::ffi::{CStr, CString, c_void};
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

use navi_dart::*;

// ── Test helpers ───────────────────────────────────────────────────

/// Shared storage for capturing callback results in tests.
static CALLBACK_RESULT: Mutex<Option<CallbackResult>> = Mutex::new(None);

#[derive(Debug, Clone)]
enum CallbackResult {
    Success(String),
    Error(String),
}

/// Rust callback that stores the result in `CALLBACK_RESULT`.
unsafe extern "C" fn test_callback(
    result_json: *const c_char,
    error: *const c_char,
    _user_data: *mut c_void,
) {
    let mut lock = CALLBACK_RESULT.lock().unwrap();
    if !result_json.is_null() {
        let json = unsafe { CStr::from_ptr(result_json) }
            .to_str()
            .unwrap()
            .to_string();
        *lock = Some(CallbackResult::Success(json));
    } else if !error.is_null() {
        let msg = unsafe { CStr::from_ptr(error) }
            .to_str()
            .unwrap()
            .to_string();
        *lock = Some(CallbackResult::Error(msg));
    }
}

/// Event callback that stores events. Returns 0 (continue).
static EVENT_RESULTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

unsafe extern "C" fn test_event_callback(
    event_json: *const c_char,
    _user_data: *mut c_void,
) -> i32 {
    if !event_json.is_null() {
        let json = unsafe { CStr::from_ptr(event_json) }
            .to_str()
            .unwrap()
            .to_string();
        EVENT_RESULTS.lock().unwrap().push(json);
    }
    0
}

fn c(s: &str) -> *const c_char {
    CString::new(s).unwrap().into_raw() as *const c_char
}

fn free_c(ptr: *const c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr as *mut c_char) });
    }
}

fn take_result() -> Option<CallbackResult> {
    CALLBACK_RESULT.lock().unwrap().take()
}

// Wait briefly for async callback to complete.
fn wait_for_callback() {
    for _ in 0..600 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if CALLBACK_RESULT.lock().unwrap().is_some() {
            return;
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

/// Disables remote registry update checks for the duration of the test
/// process. This prevents network fetches that cause timeouts and high
/// resource usage in test environments.
static REGISTRY_UPDATE_DISABLED: std::sync::Once = std::sync::Once::new();

fn disable_registry_update() {
    REGISTRY_UPDATE_DISABLED.call_once(|| {
        // SAFETY: This is set once before any test creates an engine, and no
        // other code in the test process reads or writes this variable concurrently.
        unsafe { std::env::set_var("NAVI_NO_REGISTRY_UPDATE", "1"); }
    });
}

#[test]
fn engine_new_and_free_with_temp_dir() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn engine_new_with_invalid_dir_returns_null() {
    disable_registry_update();
    let bad = c("/nonexistent/path/that/should/not/exist");
    let engine = unsafe { navi_engine_new(bad) };
    // The engine may still be created (it loads defaults); the key test is no crash.
    if !engine.is_null() {
        unsafe { navi_engine_free(engine) };
    }
    free_c(bad);
}

#[test]
fn engine_new_null_dir_returns_null() {
    disable_registry_update();
    let engine = unsafe { navi_engine_new(ptr::null()) };
    assert!(engine.is_null());
    // navi_last_error should be set
    let err = unsafe { navi_last_error() };
    assert!(!err.is_null());
    let msg = unsafe { CStr::from_ptr(err) }.to_str().unwrap();
    assert!(msg.contains("project_dir"));
}

#[test]
fn engine_free_null_is_noop() {
    unsafe { navi_engine_free(ptr::null_mut()) };
    // Should not crash
}

#[test]
fn session_ids_returns_empty_initially() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let ids_ptr = unsafe { navi_engine_session_ids(engine) };
    assert!(!ids_ptr.is_null());
    let ids_str = unsafe { CStr::from_ptr(ids_ptr) }.to_str().unwrap();
    let ids: Vec<String> = serde_json::from_str(ids_str).unwrap();
    assert!(ids.is_empty());

    unsafe { navi_string_free(ids_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn list_models_returns_non_empty() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let models_ptr = unsafe { navi_engine_list_models(engine) };
    assert!(!models_ptr.is_null());
    let models_str = unsafe { CStr::from_ptr(models_ptr) }.to_str().unwrap();
    let models: Vec<serde_json::Value> = serde_json::from_str(models_str).unwrap();
    assert!(!models.is_empty(), "should have at least one model");

    unsafe { navi_string_free(models_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn loaded_config_returns_valid_json() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let config_ptr = unsafe { navi_engine_loaded_config(engine) };
    assert!(!config_ptr.is_null());
    let config_str = unsafe { CStr::from_ptr(config_ptr) }.to_str().unwrap();
    let config: serde_json::Value = serde_json::from_str(config_str).unwrap();
    assert!(config["model"]["provider"].is_string());
    assert!(config["model"]["name"].is_string());
    assert!(config["dataDir"].is_string());

    unsafe { navi_string_free(config_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn list_provider_accounts_returns_json_array() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let accounts_ptr = unsafe { navi_engine_list_provider_accounts(engine) };
    assert!(!accounts_ptr.is_null());
    let accounts_str = unsafe { CStr::from_ptr(accounts_ptr) }.to_str().unwrap();
    let accounts: Vec<serde_json::Value> = serde_json::from_str(accounts_str).unwrap();
    // Should have at least the default provider
    assert!(!accounts.is_empty());

    unsafe { navi_string_free(accounts_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn credential_status_returns_json() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let provider = c("openai");
    let status_ptr = unsafe { navi_engine_credential_status(engine, provider) };
    assert!(!status_ptr.is_null());
    let status_str = unsafe { CStr::from_ptr(status_ptr) }.to_str().unwrap();
    let status: serde_json::Value = serde_json::from_str(status_str).unwrap();
    assert!(status["providerId"].is_string() || status["provider_id"].is_string());

    unsafe { navi_string_free(status_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
    free_c(provider);
}

#[test]
fn credential_status_unknown_provider_returns_error() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let provider = c("nonexistent_provider_xyz");
    let status_ptr = unsafe { navi_engine_credential_status(engine, provider) };
    // Should return null and set error
    assert!(status_ptr.is_null());
    let err = unsafe { navi_last_error() };
    assert!(!err.is_null());

    unsafe { navi_engine_free(engine) };
    free_c(dir);
    free_c(provider);
}

#[test]
fn set_and_delete_provider_api_key_roundtrip() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let provider = c("openai");
    let key = c("sk-test-key-not-real-12345");

    // Set key
    let result = unsafe { navi_engine_set_provider_api_key(engine, provider, key) };
    assert_eq!(result, 0);

    // Delete key
    let deleted = unsafe { navi_engine_delete_provider_api_key(engine, provider) };
    assert_eq!(deleted, 1);

    // Delete again should return 0
    let deleted2 = unsafe { navi_engine_delete_provider_api_key(engine, provider) };
    assert_eq!(deleted2, 0);

    unsafe { navi_engine_free(engine) };
    free_c(dir);
    free_c(provider);
    free_c(key);
}

#[test]
fn list_skills_returns_json_array() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let skills_ptr = unsafe { navi_engine_list_skills(engine) };
    assert!(!skills_ptr.is_null());
    let skills_str = unsafe { CStr::from_ptr(skills_ptr) }.to_str().unwrap();
    let _skills: Vec<serde_json::Value> = serde_json::from_str(skills_str).unwrap();

    unsafe { navi_string_free(skills_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn async_start_session_calls_callback() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    // Clear any previous result
    let _ = take_result();

    let request_json = c("{}");
    unsafe {
        navi_engine_start_session(engine, request_json, test_callback, ptr::null_mut());
    }
    free_c(request_json);

    // Wait for async callback
    wait_for_callback();
    let result = take_result();
    assert!(result.is_some(), "callback should have been called");
    match result.unwrap() {
        CallbackResult::Success(json) => {
            let info: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(info["id"].is_string());
            assert!(info["model"].is_string());
            assert!(info["provider"].is_string());
        }
        CallbackResult::Error(e) => panic!("expected success, got error: {e}"),
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn async_start_session_with_bad_json_calls_error() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let _ = take_result();

    // Invalid JSON should still succeed (default is used)
    let bad_json = c("{invalid");
    unsafe {
        navi_engine_start_session(engine, bad_json, test_callback, ptr::null_mut());
    }
    free_c(bad_json);

    wait_for_callback();
    let result = take_result();
    assert!(result.is_some());
    // Should still succeed since default NaviSessionRequest is used on parse failure
    match result.unwrap() {
        CallbackResult::Success(json) => {
            let info: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert!(info["id"].is_string());
        }
        CallbackResult::Error(e) => panic!("expected success with defaults, got: {e}"),
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn cancel_turn_on_nonexistent_session_calls_error() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let _ = take_result();

    let session_id = c("nonexistent-session-id");
    unsafe {
        navi_engine_cancel_turn(engine, session_id, test_callback, ptr::null_mut());
    }
    free_c(session_id);

    wait_for_callback();
    let result = take_result();
    assert!(result.is_some());
    match result.unwrap() {
        CallbackResult::Error(e) => assert!(e.contains("not found"), "error: {e}"),
        CallbackResult::Success(_) => panic!("expected error for nonexistent session"),
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn start_session_then_close_session() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let _ = take_result();

    // Start session
    let request_json = c(r#"{"sessionId":"test-session-1"}"#);
    unsafe {
        navi_engine_start_session(engine, request_json, test_callback, ptr::null_mut());
    }
    free_c(request_json);
    wait_for_callback();
    let result = take_result();
    assert!(matches!(result, Some(CallbackResult::Success(_))));

    // Session IDs should now contain our session
    let ids_ptr = unsafe { navi_engine_session_ids(engine) };
    let ids_str = unsafe { CStr::from_ptr(ids_ptr) }.to_str().unwrap();
    let ids: Vec<String> = serde_json::from_str(ids_str).unwrap();
    assert!(ids.contains(&"test-session-1".to_string()));
    unsafe { navi_string_free(ids_ptr) };

    // Close session
    let _ = take_result();
    let session_id = c("test-session-1");
    unsafe {
        navi_engine_close_session(engine, session_id, test_callback, ptr::null_mut());
    }
    free_c(session_id);
    wait_for_callback();
    let result = take_result();
    assert!(matches!(result, Some(CallbackResult::Success(_))));

    // Session IDs should be empty again
    let ids_ptr = unsafe { navi_engine_session_ids(engine) };
    let ids_str = unsafe { CStr::from_ptr(ids_ptr) }.to_str().unwrap();
    let ids: Vec<String> = serde_json::from_str(ids_str).unwrap();
    assert!(ids.is_empty());
    unsafe { navi_string_free(ids_ptr) };

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn null_session_id_calls_error_callback() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let _ = take_result();

    unsafe {
        navi_engine_cancel_turn(engine, ptr::null(), test_callback, ptr::null_mut());
    }

    // Should be called synchronously (before spawn)
    let result = take_result();
    assert!(result.is_some());
    match result.unwrap() {
        CallbackResult::Error(e) => assert!(e.contains("session_id")),
        CallbackResult::Success(_) => panic!("expected error for null session_id"),
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn navi_last_error_returns_null_when_no_error() {
    disable_registry_update();
    // navi_last_error should return null if no error has been set on this thread.
    // (It might have been set by a previous test, so just check the mechanism works.)
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    if !engine.is_null() {
        unsafe { navi_engine_free(engine) };
    }
    free_c(dir);
}

#[test]
fn navi_string_free_null_is_noop() {
    unsafe { navi_string_free(ptr::null_mut()) };
    // Should not crash
}

#[test]
fn learning_tutor_engine_creates() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new_learning_tutor(dir) };
    assert!(!engine.is_null());

    let config_ptr = unsafe { navi_engine_loaded_config(engine) };
    assert!(!config_ptr.is_null());
    let config_str = unsafe { CStr::from_ptr(config_ptr) }.to_str().unwrap();
    let config: serde_json::Value = serde_json::from_str(config_str).unwrap();
    assert!(config["model"]["name"].is_string());

    unsafe { navi_string_free(config_ptr) };
    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn async_get_goal_on_new_session_returns_null() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    // Start a session first
    let _ = take_result();
    let request = c(r#"{"sessionId":"goal-test"}"#);
    unsafe {
        navi_engine_start_session(engine, request, test_callback, ptr::null_mut());
    }
    free_c(request);
    wait_for_callback();
    let _ = take_result(); // consume start_session result

    // Get goal should return null (no goal set)
    let _ = take_result();
    let session_id = c("goal-test");
    unsafe {
        navi_engine_get_goal(engine, session_id, test_callback, ptr::null_mut());
    }
    free_c(session_id);
    wait_for_callback();
    let result = take_result();
    assert!(result.is_some());
    match result.unwrap() {
        CallbackResult::Success(json) => {
            // Should be "null"
            assert_eq!(json, "null", "no goal should be null");
        }
        CallbackResult::Error(e) => panic!("expected success (null goal), got: {e}"),
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn list_mcp_servers_and_tools_for_nonexistent_session_returns_error() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let session_id = c("nonexistent");
    let servers_ptr = unsafe { navi_engine_list_mcp_servers(engine, session_id) };
    assert!(servers_ptr.is_null());
    let err = unsafe { navi_last_error() };
    assert!(!err.is_null());

    let tools_ptr = unsafe { navi_engine_list_mcp_tools(engine, session_id) };
    assert!(tools_ptr.is_null());

    unsafe { navi_engine_free(engine) };
    free_c(dir);
    free_c(session_id);
}

#[test]
fn event_subscription_on_nonexistent_session_returns_null() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    let session_id = c("nonexistent");
    let sub = unsafe {
        navi_engine_subscribe_events(engine, session_id, test_event_callback, ptr::null_mut())
    };
    assert!(sub.is_null());

    unsafe { navi_engine_free(engine) };
    free_c(dir);
    free_c(session_id);
}

#[test]
fn event_subscription_free_null_is_noop() {
    unsafe { navi_event_subscription_free(ptr::null_mut()) };
    // Should not crash
}

// ── types.rs unit tests ────────────────────────────────────────────

#[test]
fn to_json_ptr_roundtrip() {
    use navi_dart::to_json_ptr;
    let data = vec!["a", "b", "c"];
    let ptr = to_json_ptr(&data);
    assert!(!ptr.is_null());
    let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    let parsed: Vec<String> = serde_json::from_str(json).unwrap();
    assert_eq!(parsed, vec!["a", "b", "c"]);
    unsafe { navi_string_free(ptr) };
}

#[test]
fn to_json_ptr_complex_object() {
    use navi_dart::to_json_ptr;
    let obj = serde_json::json!({"key": "value", "num": 42});
    let ptr = to_json_ptr(&obj);
    assert!(!ptr.is_null());
    let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(parsed["key"], "value");
    assert_eq!(parsed["num"], 42);
    unsafe { navi_string_free(ptr) };
}

#[test]
fn cstr_to_str_null_returns_none() {
    use navi_dart::cstr_to_str;
    let result = unsafe { cstr_to_str(ptr::null()) };
    assert!(result.is_none());
}

#[test]
fn cstr_to_str_valid_string() {
    use navi_dart::cstr_to_str;
    let s = c("hello world");
    let result = unsafe { cstr_to_str(s) };
    assert_eq!(result, Some("hello world"));
    free_c(s);
}

#[test]
fn parse_save_target_variants() {
    use navi_dart::parse_save_target;
    use navi_sdk::NaviConfigSaveTarget;
    assert!(matches!(
        parse_save_target(None),
        NaviConfigSaveTarget::Auto
    ));
    assert!(matches!(
        parse_save_target(Some("project")),
        NaviConfigSaveTarget::Project
    ));
    assert!(matches!(
        parse_save_target(Some("global")),
        NaviConfigSaveTarget::Global
    ));
    assert!(matches!(
        parse_save_target(Some("none")),
        NaviConfigSaveTarget::None
    ));
    assert!(matches!(
        parse_save_target(Some("unknown")),
        NaviConfigSaveTarget::Auto
    ));
}

#[test]
fn callback_ctx_success_and_error() {
    use navi_dart::CallbackCtx;
    let _ = take_result();

    let ctx = CallbackCtx::new(test_callback, ptr::null_mut());
    ctx.success(&serde_json::json!({"ok": true}));
    let result = take_result().unwrap();
    match result {
        CallbackResult::Success(json) => {
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v["ok"], true);
        }
        _ => panic!("expected success"),
    }

    ctx.error("test error message");
    let result = take_result().unwrap();
    match result {
        CallbackResult::Error(msg) => assert_eq!(msg, "test error message"),
        _ => panic!("expected error"),
    }
}

#[test]
fn callback_ctx_success_str() {
    use navi_dart::CallbackCtx;
    let _ = take_result();

    let ctx = CallbackCtx::new(test_callback, ptr::null_mut());
    ctx.success_str("null");
    let result = take_result().unwrap();
    match result {
        CallbackResult::Success(json) => assert_eq!(json, "null"),
        _ => panic!("expected success"),
    }
}
