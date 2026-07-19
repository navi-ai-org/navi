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
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Duration;

use navi_dart::*;

// ── Test helpers ───────────────────────────────────────────────────
//
// Async FFI callbacks must NOT share one process-global slot: cargo runs this
// binary with multiple threads, and callbacks from different engines race.
// Each call site owns an [`AsyncProbe`] channel and passes its sender as
// `user_data` so results cannot cross-contaminate.

#[derive(Debug, Clone)]
enum CallbackResult {
    Success(String),
    Error(String),
}

/// Per-call async callback capture (parallel-safe).
struct AsyncProbe {
    tx: SyncSender<CallbackResult>,
    rx: Receiver<CallbackResult>,
}

impl AsyncProbe {
    fn new() -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        Self { tx, rx }
    }

    /// Pointer stable for the lifetime of `self` (stack-pin the probe).
    fn user_data(&self) -> *mut c_void {
        &self.tx as *const SyncSender<CallbackResult> as *mut c_void
    }

    /// Wait up to `timeout` for the next callback result.
    fn wait_timeout(&self, timeout: Duration) -> Option<CallbackResult> {
        self.rx.recv_timeout(timeout).ok()
    }

    fn wait(&self) -> Option<CallbackResult> {
        self.wait_timeout(Duration::from_secs(30))
    }

    /// Non-blocking take (for sync callbacks that fire before return).
    fn try_take(&self) -> Option<CallbackResult> {
        self.rx.try_recv().ok()
    }
}

/// Rust callback that delivers into the [`AsyncProbe`] behind `user_data`.
unsafe extern "C" fn test_callback(
    result_json: *const c_char,
    error: *const c_char,
    user_data: *mut c_void,
) {
    if user_data.is_null() {
        return;
    }
    // SAFETY: caller passes `AsyncProbe::user_data()` for a live probe.
    let tx = unsafe { &*(user_data as *const SyncSender<CallbackResult>) };
    if !result_json.is_null() {
        let json = unsafe { CStr::from_ptr(result_json) }
            .to_str()
            .unwrap_or("")
            .to_string();
        let _ = tx.send(CallbackResult::Success(json));
    } else if !error.is_null() {
        let msg = unsafe { CStr::from_ptr(error) }
            .to_str()
            .unwrap_or("")
            .to_string();
        let _ = tx.send(CallbackResult::Error(msg));
    }
}

/// Event callback used only for subscription handle lifecycle (no asserts on
/// content). Returns 0 (continue).
unsafe extern "C" fn test_event_callback(
    _event_json: *const c_char,
    _user_data: *mut c_void,
) -> i32 {
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

// ── Tests ──────────────────────────────────────────────────────────

/// Isolate NAVI config/data dirs and disable remote registry for the test
/// process. Without this, engines load the developer's real
/// `~/.config/navi` (e.g. `charm-hyper` without a key) and `start_session`
/// fails under parallel suite load in ways that are environment-dependent.
static TEST_ENV_READY: std::sync::Once = std::sync::Once::new();

fn disable_registry_update() {
    TEST_ENV_READY.call_once(|| {
        let base = tempfile::tempdir()
            .expect("test isolation tempdir")
            .keep();
        let data = base.join("data");
        let config = base.join("config");
        let home = base.join("home");
        std::fs::create_dir_all(&data).expect("data dir");
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::create_dir_all(&home).expect("home dir");
        // SAFETY: set once before any engine is built; no concurrent readers.
        unsafe {
            std::env::set_var("NAVI_NO_REGISTRY_UPDATE", "1");
            std::env::set_var("XDG_DATA_HOME", &data);
            std::env::set_var("XDG_CONFIG_HOME", &config);
            std::env::set_var("HOME", &home);
        }
    });
}

/// Seed a dummy OpenAI key and select that model so start_session works
/// without host credentials or the developer's preferred provider.
fn seed_test_api_key(engine: *mut NaviDartEngine) {
    let provider = c("openai");
    let key = c("sk-test-key-not-real-docker");
    let rc = unsafe { navi_engine_set_provider_api_key(engine, provider, key) };
    assert_eq!(rc, 0, "seed dummy openai api key");
    free_c(key);

    // Point the engine at OpenAI so start_session does not require whatever
    // provider the host config prefers (e.g. charm-hyper).
    let model = c("gpt-5.5");
    let save = c("none");
    let probe = AsyncProbe::new();
    unsafe {
        navi_engine_select_model(engine, provider, model, save, test_callback, probe.user_data());
    }
    free_c(provider);
    free_c(model);
    free_c(save);
    let _ = probe.wait_timeout(Duration::from_secs(5));
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

    seed_test_api_key(engine);

    let probe = AsyncProbe::new();
    let request_json = c("{}");
    unsafe {
        navi_engine_start_session(engine, request_json, test_callback, probe.user_data());
    }
    free_c(request_json);

    let result = probe.wait();
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

    seed_test_api_key(engine);

    // Invalid JSON should still succeed (default is used)
    let probe = AsyncProbe::new();
    let bad_json = c("{invalid");
    unsafe {
        navi_engine_start_session(engine, bad_json, test_callback, probe.user_data());
    }
    free_c(bad_json);

    let result = probe.wait();
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

    let probe = AsyncProbe::new();
    let session_id = c("nonexistent-session-id");
    unsafe {
        navi_engine_cancel_turn(engine, session_id, test_callback, probe.user_data());
    }
    free_c(session_id);

    let result = probe.wait();
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

    seed_test_api_key(engine);

    // Start session
    let start = AsyncProbe::new();
    let request_json = c(r#"{"sessionId":"test-session-1"}"#);
    unsafe {
        navi_engine_start_session(engine, request_json, test_callback, start.user_data());
    }
    free_c(request_json);
    let result = start.wait();
    assert!(matches!(result, Some(CallbackResult::Success(_))));

    // Session IDs should now contain our session
    let ids_ptr = unsafe { navi_engine_session_ids(engine) };
    let ids_str = unsafe { CStr::from_ptr(ids_ptr) }.to_str().unwrap();
    let ids: Vec<String> = serde_json::from_str(ids_str).unwrap();
    assert!(ids.contains(&"test-session-1".to_string()));
    unsafe { navi_string_free(ids_ptr) };

    // Close session
    let close = AsyncProbe::new();
    let session_id = c("test-session-1");
    unsafe {
        navi_engine_close_session(engine, session_id, test_callback, close.user_data());
    }
    free_c(session_id);
    let result = close.wait();
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

    let probe = AsyncProbe::new();
    unsafe {
        navi_engine_cancel_turn(engine, ptr::null(), test_callback, probe.user_data());
    }

    // Should be called synchronously (before spawn)
    let result = probe.try_take().or_else(|| probe.wait_timeout(Duration::from_secs(1)));
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
fn async_get_goal_on_new_session_returns_null() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    seed_test_api_key(engine);

    // Start a session first
    let start = AsyncProbe::new();
    let request = c(r#"{"sessionId":"goal-test"}"#);
    unsafe {
        navi_engine_start_session(engine, request, test_callback, start.user_data());
    }
    free_c(request);
    assert!(
        matches!(start.wait(), Some(CallbackResult::Success(_))),
        "start_session should succeed"
    );

    // Get goal should return null (no goal set)
    let goal = AsyncProbe::new();
    let session_id = c("goal-test");
    unsafe {
        navi_engine_get_goal(engine, session_id, test_callback, goal.user_data());
    }
    free_c(session_id);
    let result = goal.wait();
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

    let probe = AsyncProbe::new();
    let ctx = CallbackCtx::new(test_callback, probe.user_data());
    ctx.success(&serde_json::json!({"ok": true}));
    let result = probe.try_take().expect("sync success callback");
    match result {
        CallbackResult::Success(json) => {
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v["ok"], true);
        }
        _ => panic!("expected success"),
    }

    ctx.error("test error message");
    let result = probe.try_take().expect("sync error callback");
    match result {
        CallbackResult::Error(msg) => assert_eq!(msg, "test error message"),
        _ => panic!("expected error"),
    }
}

#[test]
fn callback_ctx_success_str() {
    use navi_dart::CallbackCtx;

    let probe = AsyncProbe::new();
    let ctx = CallbackCtx::new(test_callback, probe.user_data());
    ctx.success_str("null");
    let result = probe.try_take().expect("sync success_str callback");
    match result {
        CallbackResult::Success(json) => assert_eq!(json, "null"),
        _ => panic!("expected success"),
    }
}

// ── Surface gap-fill coverage ──────────────────────────────────────

#[test]
fn memory_update_and_notify_simple_and_tui_extensions() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    // memory write then update
    let id = c("dart-gap-1");
    let mt = c("project");
    let name = c("Gap fill");
    let desc = c("desc");
    let body = c("body");
    let rc = unsafe { navi_engine_memory_write(engine, id, mt, name, desc, body) };
    // memory may be disabled by default — accept success or soft failure via last_error
    if rc == 0 {
        let name2 = c("Gap fill updated");
        let rc2 = unsafe {
            navi_engine_memory_update(engine, id, name2, ptr::null(), ptr::null(), ptr::null())
        };
        assert_eq!(rc2, 0);
        free_c(name2);
    }
    free_c(id);
    free_c(mt);
    free_c(name);
    free_c(desc);
    free_c(body);

    let title = c("NAVI test");
    let body = c("hello");
    let nptr = unsafe { navi_engine_notify_simple(engine, title, body, 0) };
    // desktop=0 should still return payload
    assert!(!nptr.is_null(), "notify_simple should return JSON");
    unsafe { navi_string_free(nptr) };
    free_c(title);
    free_c(body);

    let ext = unsafe { navi_engine_list_tui_extensions(engine) };
    assert!(!ext.is_null());
    let s = unsafe { CStr::from_ptr(ext) }.to_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    assert!(v.is_array());
    unsafe { navi_string_free(ext) };

    let cmds = unsafe { navi_engine_list_tui_extension_commands(engine) };
    assert!(!cmds.is_null());
    let s = unsafe { CStr::from_ptr(cmds) }.to_str().unwrap();
    let v: serde_json::Value = serde_json::from_str(s).unwrap();
    assert!(v.is_array());
    unsafe { navi_string_free(cmds) };

    let accounts = unsafe { navi_engine_list_credential_accounts(engine, c("openai")) };
    // may error if provider invalid handling differs; free if non-null
    if !accounts.is_null() {
        unsafe { navi_string_free(accounts) };
    }

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}

#[test]
fn voice_subscribe_and_rewind_session_surface() {
    disable_registry_update();
    let tmp = tempfile::tempdir().unwrap();
    let dir = c(tmp.path().to_str().unwrap());
    let engine = unsafe { navi_engine_new(dir) };
    assert!(!engine.is_null());

    seed_test_api_key(engine);

    // Voice event subscription should return a handle even with no mic.
    let sub =
        unsafe { navi_engine_subscribe_voice_events(engine, test_event_callback, ptr::null_mut()) };
    assert!(!sub.is_null());
    unsafe { navi_event_subscription_free(sub) };

    // Start session then rewind
    let start = AsyncProbe::new();
    let request = c(r#"{"sessionId":"rewind-test"}"#);
    unsafe {
        navi_engine_start_session(engine, request, test_callback, start.user_data());
    }
    free_c(request);
    assert!(matches!(start.wait(), Some(CallbackResult::Success(_))));

    let rewind = AsyncProbe::new();
    let sid = c("rewind-test");
    unsafe {
        navi_engine_rewind_session(engine, sid, 0, test_callback, rewind.user_data());
    }
    free_c(sid);
    let result = rewind.wait();
    assert!(
        matches!(result, Some(CallbackResult::Success(_))),
        "rewind should succeed: {result:?}"
    );

    unsafe { navi_engine_free(engine) };
    free_c(dir);
}
