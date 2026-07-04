// C ABI engine surface for Dart FFI.
//
// Every public `extern "C"` function here is callable from Dart via dart:ffi.
// Opaque `NaviDartEngine` handles are created/freed via `navi_engine_new` / `navi_engine_free`.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr;

use navi_core::{ApprovalDecision, ContextPacket, QuestionResponse};
use navi_sdk::{
    NaviEngine, NaviEngineBuilder, NaviModelSelectionRequest, NaviSessionRequest, NaviTurnRequest,
};
use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use crate::event_stream::NaviEventSubscription;
use crate::types::{
    call_error, call_success, cstr_to_str, parse_save_target, reply_json, set_last_error,
    to_json_ptr, NaviAsyncCallback, NaviEventCallback, SendPtr,
};

// ── Engine Handle ──────────────────────────────────────────────────

/// Opaque engine handle. Create with `navi_engine_new`, free with `navi_engine_free`.
pub struct NaviDartEngine {
    pub(crate) runtime: Runtime,
    pub(crate) inner: NaviEngine,
}

/// Creates a new engine for the given project directory.
///
/// Returns an opaque handle or null on error. Call `navi_last_error()` for details.
/// The handle must be freed with `navi_engine_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_new(project_dir: *const c_char) -> *mut NaviDartEngine {
    let dir = match unsafe { cstr_to_str(project_dir) } {
        Some(s) => s,
        None => {
            set_last_error("project_dir is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };

    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            set_last_error(&format!("failed to create tokio runtime: {e}"));
            return ptr::null_mut();
        }
    };

    let inner = match runtime.block_on(async { NaviEngineBuilder::from_project(dir).build() }) {
        Ok(engine) => engine,
        Err(e) => {
            set_last_error(&format!("failed to build engine: {e}"));
            return ptr::null_mut();
        }
    };

    Box::into_raw(Box::new(NaviDartEngine { runtime, inner }))
}

/// Creates a new engine configured as a learning tutor.
///
/// Uses permissive tool security, the learning harness, tutor prompt builder,
/// and study-aware compaction defaults.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_new_learning_tutor(
    project_dir: *const c_char,
) -> *mut NaviDartEngine {
    let dir = match unsafe { cstr_to_str(project_dir) } {
        Some(s) => s,
        None => {
            set_last_error("project_dir is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };

    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            set_last_error(&format!("failed to create tokio runtime: {e}"));
            return ptr::null_mut();
        }
    };

    let inner = match runtime.block_on(async {
        NaviEngineBuilder::from_project(dir)
            .learning_tutor()
            .build()
    }) {
        Ok(engine) => engine,
        Err(e) => {
            set_last_error(&format!("failed to build engine: {e}"));
            return ptr::null_mut();
        }
    };

    Box::into_raw(Box::new(NaviDartEngine { runtime, inner }))
}

/// Frees an engine handle. Passing null is a safe no-op.
///
/// After this call the handle must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_free(engine: *mut NaviDartEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

// ── Sessions ───────────────────────────────────────────────────────

/// Starts a new session. `request_json` is a JSON `NaviSessionRequest` (may be null/empty).
///
/// The callback receives the session info as JSON on success or an error string on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_start_session(
    engine: *mut NaviDartEngine,
    request_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let request: NaviSessionRequest = unsafe { cstr_to_str(request_json) }
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.start_session(request).await {
            Ok(info) => unsafe { reply_json(callback, &info, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Closes an active session. Callback receives `true` if a session was removed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_close_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.close_session(&sid).await {
            Ok(removed) => {
                let json = serde_json::to_string(&removed).unwrap_or_default();
                unsafe { call_success(callback, &json, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Returns the IDs of all active (in-memory) sessions as a JSON string array.
///
/// Returns a `*mut c_char` that must be freed with `navi_string_free`, or null on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_session_ids(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    let ids = engine.inner.session_ids();
    to_json_ptr(&ids)
}

// ── Turns ──────────────────────────────────────────────────────────

/// Sends a user message to an active session.
///
/// `request_json` is a JSON `NaviTurnRequest`.
/// Callback receives the `NaviTurnResponse` as JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_send_turn(
    engine: *mut NaviDartEngine,
    request_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let request_str = match unsafe { cstr_to_str(request_json) } {
        Some(s) => s,
        None => {
            unsafe { call_error(callback, "request_json is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let request: NaviTurnRequest = match serde_json::from_str(request_str) {
        Ok(r) => r,
        Err(e) => {
            unsafe { call_error(callback, &format!("invalid turn request: {e}"), user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.send_turn(request).await {
            Ok(response) => unsafe { reply_json(callback, &response, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Cancels the currently active turn for the given session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_cancel_turn(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.cancel_turn(&sid).await {
            Ok(()) => unsafe { call_success(callback, "null", user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Takes a point-in-time snapshot of session state for persistence.
///
/// Callback receives the `SessionSnapshot` as JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_snapshot_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.snapshot_session(&sid).await {
            Ok(snapshot) => unsafe { reply_json(callback, &snapshot, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Approvals & Questions ──────────────────────────────────────────

/// Resolves a pending tool approval request.
///
/// Callback receives `true` if the approval was consumed, `false` otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_resolve_approval(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    approval_id: *const c_char,
    approved: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let aid = match unsafe { cstr_to_str(approval_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "approval_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let decision = if approved != 0 {
        ApprovalDecision::Approved { id: aid }
    } else {
        ApprovalDecision::Denied { id: aid }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.resolve_approval(&sid, decision).await {
            Ok(consumed) => {
                let json = serde_json::to_string(&consumed).unwrap_or_default();
                unsafe { call_success(callback, &json, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Resolves a pending interactive question.
///
/// `response_json` is a JSON `QuestionResponse`.
/// Callback receives `true` if the response was consumed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_resolve_question(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    response_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let response: QuestionResponse = match unsafe { cstr_to_str(response_json) }
        .and_then(|s| serde_json::from_str(s).ok())
    {
        Some(r) => r,
        None => {
            unsafe { call_error(callback, "invalid response_json", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.resolve_question(&sid, response).await {
            Ok(consumed) => {
                let json = serde_json::to_string(&consumed).unwrap_or_default();
                unsafe { call_success(callback, &json, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Context ────────────────────────────────────────────────────────

/// Adds a context packet (file, selection, memory, etc.) to an active session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_add_context_packet(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    packet_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let packet: ContextPacket = match unsafe { cstr_to_str(packet_json) }
        .and_then(|s| serde_json::from_str(s).ok())
    {
        Some(p) => p,
        None => {
            unsafe { call_error(callback, "invalid packet_json", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.add_context_packet(&sid, packet).await {
            Ok(()) => unsafe { call_success(callback, "null", user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Models ─────────────────────────────────────────────────────────

/// Lists all available models across configured providers.
///
/// Returns a `*mut c_char` with JSON array of `NaviModelInfo`, or null on error.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_models(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    let models = engine.inner.list_models();
    to_json_ptr(&models)
}

/// Changes the model used by an active session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_model(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    provider: *const c_char,
    model: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let prov = match unsafe { cstr_to_str(provider) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "provider is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let mdl = match unsafe { cstr_to_str(model) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "model is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.set_model(&sid, &prov, &mdl).await {
            Ok(()) => unsafe { call_success(callback, "null", user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Selects a model for the engine and optionally persists the config change.
///
/// `provider_id` and `model` are required C strings. `save_target` is an optional
/// C string: "project", "global", "none", or null/"auto".
///
/// Returns a JSON `NaviModelSelectionResult` via callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_select_model(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    model: *const c_char,
    save_target: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "provider_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let mdl = match unsafe { cstr_to_str(model) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "model is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });

    let request = NaviModelSelectionRequest {
        provider_id: pid,
        model: mdl,
        save_target: target,
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.select_model(request) {
            Ok(result) => {
                let json = json!({
                    "providerId": result.provider_id,
                    "model": result.model,
                    "contextWindowTokens": result.context_window_tokens,
                    "providerConfigured": result.provider_configured,
                    "savedTo": result.saved_to.map(|p| p.display().to_string()),
                });
                let json_str = serde_json::to_string(&json).unwrap_or_default();
                unsafe { call_success(callback, &json_str, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Events ─────────────────────────────────────────────────────────

/// Subscribes to the event stream for a session.
///
/// The `callback` is invoked for each `RuntimeEvent` (serialized as JSON).
/// Return 0 from the callback to continue, non-zero to stop.
///
/// Returns an opaque subscription handle or null on error.
/// Free with `navi_event_subscription_free` to stop receiving events.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_subscribe_events(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviEventCallback,
    user_data: *mut c_void,
) -> *mut NaviEventSubscription {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            set_last_error("session_id is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };

    let receiver = match engine.inner.subscribe_events(&sid) {
        Ok(r) => r,
        Err(e) => {
            set_last_error(&e.to_string());
            return ptr::null_mut();
        }
    };

    let ud = SendPtr(user_data);
    let task = engine.runtime.spawn(async move {
        event_loop(receiver, callback, ud).await;
    });

    Box::into_raw(Box::new(NaviEventSubscription { _task: task }))
}

/// Stops an event subscription and frees its handle. Passing null is a safe no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_event_subscription_free(sub: *mut NaviEventSubscription) {
    if !sub.is_null() {
        let sub = unsafe { Box::from_raw(sub) };
        sub._task.abort();
    }
}

// ── Goals ──────────────────────────────────────────────────────────

/// Returns the current goal for a session as JSON, or null if no goal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_get_goal(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.get_goal(&sid).await {
            Ok(goal) => unsafe { reply_json(callback, &goal, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Sets a goal for a session. `objective` is the goal text, `token_budget` is
/// optional (-1 means null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_goal(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    objective: *const c_char,
    token_budget: i64,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let obj = match unsafe { cstr_to_str(objective) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "objective is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let budget = if token_budget < 0 {
        None
    } else {
        Some(token_budget)
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.set_goal(&sid, obj, budget).await {
            Ok(goal) => unsafe { reply_json(callback, &goal, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Clears the goal for a session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_clear_goal(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.clear_goal(&sid).await {
            Ok(()) => unsafe { call_success(callback, "null", user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Background Tasks ───────────────────────────────────────────────

/// Lists all active background bash commands for a session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_background_commands(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.list_background_commands(&sid).await {
            Ok(cmds) => unsafe { reply_json(callback, &cmds, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Polls a specific background bash command.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_poll_background_command(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    task_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let tid = match unsafe { cstr_to_str(task_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "task_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.poll_background_command(&sid, &tid).await {
            Ok(snapshot) => unsafe { reply_json(callback, &snapshot, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Cancels a specific background bash command.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_cancel_background_command(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    task_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let tid = match unsafe { cstr_to_str(task_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "task_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.cancel_background_command(&sid, &tid).await {
            Ok(snapshot) => unsafe { reply_json(callback, &snapshot, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Providers & Credentials ────────────────────────────────────────

/// Lists all configured providers with their credential status.
///
/// Returns a `*mut c_char` with JSON array of `NaviProviderAccountInfo`, or null on error.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_provider_accounts(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.list_provider_accounts() {
        Ok(accounts) => to_json_ptr(&accounts),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

/// Returns credential status for a specific provider.
///
/// Returns a `*mut c_char` with JSON `NaviProviderCredentialStatus`, or null on error.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_credential_status(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s,
        None => {
            set_last_error("provider_id is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };
    match engine.inner.credential_status(pid) {
        Ok(status) => to_json_ptr(&status),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

/// Stores an API key for the given provider in the credential store.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_provider_api_key(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    api_key: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s,
        None => {
            set_last_error("provider_id is null or invalid UTF-8");
            return -1;
        }
    };
    let key = match unsafe { cstr_to_str(api_key) } {
        Some(s) => s,
        None => {
            set_last_error("api_key is null or invalid UTF-8");
            return -1;
        }
    };
    match engine.inner.set_provider_api_key(pid, key) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

/// Deletes a stored API key. Returns 1 if a key was removed, 0 if not found, -1 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_delete_provider_api_key(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s,
        None => {
            set_last_error("provider_id is null or invalid UTF-8");
            return -1;
        }
    };
    match engine.inner.delete_provider_api_key(pid) {
        Ok(deleted) => if deleted { 1 } else { 0 },
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

/// Fetches OpenAI/ChatGPT usage windows for the selected OpenAI account.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_usage_report(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.usage_report().await {
            Ok(report) => unsafe { reply_json(callback, &report, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Skills ─────────────────────────────────────────────────────────

/// Lists discovered skills from project and global skill directories.
///
/// Returns a `*mut c_char` with JSON array of `NaviSkillInfo`, or null on error.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_skills(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.list_skills() {
        Ok(skills) => to_json_ptr(&skills),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

/// Sets the active skills for an existing session.
///
/// `skills_json` is a JSON string array of skill IDs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_session_skills(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    skills_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let skills: Vec<String> = match unsafe { cstr_to_str(skills_json) }
        .and_then(|s| serde_json::from_str(s).ok())
    {
        Some(v) => v,
        None => {
            unsafe { call_error(callback, "invalid skills_json", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.set_session_skills(&sid, skills).await {
            Ok(()) => unsafe { call_success(callback, "null", user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── MCP ────────────────────────────────────────────────────────────

/// Lists MCP servers connected to the given session.
///
/// Returns a `*mut c_char` with JSON, or null on error. Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_mcp_servers(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s,
        None => {
            set_last_error("session_id is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };
    match engine.inner.list_mcp_servers(sid) {
        Ok(servers) => to_json_ptr(&servers),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

/// Lists tool names provided by MCP servers in the given session.
///
/// Returns a `*mut c_char` with JSON string array, or null on error.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_mcp_tools(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s,
        None => {
            set_last_error("session_id is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };
    match engine.inner.list_mcp_tools(sid) {
        Ok(tools) => to_json_ptr(&tools),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

// ── Provider Sync ──────────────────────────────────────────────────

/// Fetches the latest model list from a specific provider and updates config.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_sync_provider_models(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    save_target: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "provider_id is null or invalid UTF-8", user_data) };
            return;
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_provider_models(&pid, target).await {
            Ok(report) => {
                let json = json!({
                    "savedTo": report.saved_to.map(|p| p.display().to_string()),
                    "updated": report.updated,
                    "failed": report.failed,
                    "skipped": report.skipped,
                });
                let json_str = serde_json::to_string(&json).unwrap_or_default();
                unsafe { call_success(callback, &json_str, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Fetches the latest model lists from all configured providers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_sync_models(
    engine: *mut NaviDartEngine,
    save_target: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_models(target).await {
            Ok(report) => {
                let json = json!({
                    "savedTo": report.saved_to.map(|p| p.display().to_string()),
                    "updated": report.updated,
                    "failed": report.failed,
                    "skipped": report.skipped,
                });
                let json_str = serde_json::to_string(&json).unwrap_or_default();
                unsafe { call_success(callback, &json_str, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Registry ───────────────────────────────────────────────────────

/// Syncs the provider registry into the local SQLite cache.
///
/// Pass `force` as non-zero to force a full re-sync.
/// Callback receives `true` if the cache was updated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_sync_registry(
    engine: *mut NaviDartEngine,
    force: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_registry(force != 0).await {
            Ok(updated) => {
                let json = serde_json::to_string(&updated).unwrap_or_default();
                unsafe { call_success(callback, &json, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Plugins ────────────────────────────────────────────────────────

/// Reloads WASM plugin tools on every active in-memory session.
///
/// Callback receives a JSON string array of warnings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_reload_wasm_plugins(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.reload_wasm_plugins().await {
            Ok(warnings) => unsafe { reply_json(callback, &warnings, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Saved Sessions ─────────────────────────────────────────────────

/// Lists all persisted sessions with their titles and timestamps.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_saved_sessions(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.list_saved_sessions_async().await {
            Ok(sessions) => unsafe { reply_json(callback, &sessions, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Loads a persisted session by ID and reopens it in memory.
///
/// Callback receives the `SessionSnapshot` as JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_load_saved_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.load_saved_session_async(&sid).await {
            Ok(snapshot) => unsafe { reply_json(callback, &snapshot, user_data) },
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

/// Deletes a persisted session. Callback receives `true` if a session was removed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_delete_saved_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            unsafe { call_error(callback, "session_id is null or invalid UTF-8", user_data) };
            return;
        }
    };

    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.delete_saved_session_async(&sid).await {
            Ok(deleted) => {
                let json = serde_json::to_string(&deleted).unwrap_or_default();
                unsafe { call_success(callback, &json, user_data) }
            }
            Err(e) => unsafe { call_error(callback, &e.to_string(), user_data) },
        }
    });
}

// ── Config ─────────────────────────────────────────────────────────

/// Returns a snapshot of the current loaded configuration as JSON.
///
/// Returns a `*mut c_char` that must be freed with `navi_string_free`, or null on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_loaded_config(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    let config = engine.inner.loaded_config();
    let json = json!({
        "model": {
            "provider": config.config.model.provider,
            "name": config.config.model.name,
        },
        "attachmentModels": {
            "image": config.config.attachment_models.image,
            "audio": config.config.attachment_models.audio,
            "video": config.config.attachment_models.video,
            "document": config.config.attachment_models.document,
        },
        "globalConfigPath": config.global_config_path,
        "projectConfigPath": config.project_config_path,
        "dataDir": config.data_dir,
    });
    to_json_ptr(&json)
}

// ── Internal helpers ───────────────────────────────────────────────

async fn event_loop(
    mut receiver: broadcast::Receiver<RuntimeEvent>,
    callback: NaviEventCallback,
    ud: SendPtr,
) {
    loop {
        match receiver.recv().await {
            Ok(event) => match serde_json::to_string(&event) {
                Ok(json) => {
                    let c_json = CString::new(json).unwrap_or_default();
                    let should_stop = unsafe { callback(c_json.as_ptr(), ud.0) };
                    if should_stop != 0 {
                        break;
                    }
                }
                Err(_) => continue,
            },
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}
