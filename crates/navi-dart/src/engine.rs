// C ABI engine surface for Dart FFI.
//
// Every public `extern "C"` function here is callable from Dart via dart:ffi.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr;

use navi_core::{ApprovalDecision, ContextPacket, QuestionResponse, RuntimeEvent};
use navi_sdk::{
    NaviEngine, NaviEngineBuilder, NaviModelSelectionRequest, NaviSessionRequest, NaviTurnRequest,
};
use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

use crate::event_stream::NaviEventSubscription;
use crate::types::{
    CallbackCtx, EventCtx, NaviAsyncCallback, NaviEventCallback, cstr_to_str, parse_save_target,
    set_last_error, to_json_ptr,
};

// ── Engine Handle ──────────────────────────────────────────────────

/// Opaque engine handle.
pub struct NaviDartEngine {
    pub(crate) runtime: Runtime,
    pub(crate) inner: NaviEngine,
}

/// Creates a new engine for the given project directory.
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

/// Frees an engine handle. Passing null is a safe no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_free(engine: *mut NaviDartEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

// ── Sessions ───────────────────────────────────────────────────────

/// Starts a new session. `request_json` is a JSON `NaviSessionRequest`.
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
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.start_session(request).await {
            Ok(info) => ctx.success(&info),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Closes an active session.
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
            let ctx = CallbackCtx::new(callback, user_data);
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.close_session(&sid).await {
            Ok(removed) => ctx.success(&removed),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Returns the IDs of all active sessions as a JSON string array.
/// Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_session_ids(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.session_ids())
}

// ── Turns ──────────────────────────────────────────────────────────

/// Sends a user message to an active session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_send_turn(
    engine: *mut NaviDartEngine,
    request_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let request_str = match unsafe { cstr_to_str(request_json) } {
        Some(s) => s,
        None => {
            ctx.error("request_json is null or invalid UTF-8");
            return;
        }
    };
    let request: NaviTurnRequest = match serde_json::from_str(request_str) {
        Ok(r) => r,
        Err(e) => {
            ctx.error(&format!("invalid turn request: {e}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.send_turn(request).await {
            Ok(response) => ctx.success(&response),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Cancels the currently active turn.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_cancel_turn(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.cancel_turn(&sid).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Takes a point-in-time snapshot of session state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_snapshot_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.snapshot_session(&sid).await {
            Ok(snapshot) => ctx.success(&snapshot),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Approvals & Questions ──────────────────────────────────────────

/// Resolves a pending tool approval request.
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
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let aid = match unsafe { cstr_to_str(approval_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("approval_id is null or invalid UTF-8");
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
            Ok(consumed) => ctx.success(&consumed),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Resolves a pending interactive question.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_resolve_question(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    response_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let response: QuestionResponse =
        match unsafe { cstr_to_str(response_json) }.and_then(|s| serde_json::from_str(s).ok()) {
            Some(r) => r,
            None => {
                ctx.error("invalid response_json");
                return;
            }
        };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.resolve_question(&sid, response).await {
            Ok(consumed) => ctx.success(&consumed),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Context ────────────────────────────────────────────────────────

/// Adds a context packet to an active session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_add_context_packet(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    packet_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let packet: ContextPacket =
        match unsafe { cstr_to_str(packet_json) }.and_then(|s| serde_json::from_str(s).ok()) {
            Some(p) => p,
            None => {
                ctx.error("invalid packet_json");
                return;
            }
        };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.add_context_packet(&sid, packet).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Models ─────────────────────────────────────────────────────────

/// Lists all available models as a JSON array. Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_models(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.list_models())
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
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let prov = match unsafe { cstr_to_str(provider) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("provider is null or invalid UTF-8");
            return;
        }
    };
    let mdl = match unsafe { cstr_to_str(model) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("model is null or invalid UTF-8");
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.set_model(&sid, &prov, &mdl).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Selects a model and optionally persists the config change.
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
    let ctx = CallbackCtx::new(callback, user_data);
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("provider_id is null or invalid UTF-8");
            return;
        }
    };
    let mdl = match unsafe { cstr_to_str(model) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("model is null or invalid UTF-8");
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
                ctx.success(&json)
            }
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Events ─────────────────────────────────────────────────────────

/// Subscribes to the event stream for a session.
/// Return 0 from callback to continue, non-zero to stop.
/// Free returned handle with `navi_event_subscription_free`.
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
    let event_ctx = EventCtx::new(callback, user_data);
    let task = engine
        .runtime
        .spawn(async move { event_loop(receiver, event_ctx).await });
    Box::into_raw(Box::new(NaviEventSubscription { _task: task }))
}

/// Stops an event subscription and frees its handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_event_subscription_free(sub: *mut NaviEventSubscription) {
    if !sub.is_null() {
        let sub = unsafe { Box::from_raw(sub) };
        sub._task.abort();
    }
}

/// Subscribes to engine-global voice events (partial/final/error).
/// Return 0 from callback to continue, non-zero to stop.
/// Free returned handle with `navi_event_subscription_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_subscribe_voice_events(
    engine: *mut NaviDartEngine,
    callback: NaviEventCallback,
    user_data: *mut c_void,
) -> *mut NaviEventSubscription {
    let engine = unsafe { &*engine };
    let receiver = engine.inner.subscribe_voice_events();
    let event_ctx = EventCtx::new(callback, user_data);
    let task = engine
        .runtime
        .spawn(async move { voice_event_loop(receiver, event_ctx).await });
    Box::into_raw(Box::new(NaviEventSubscription { _task: task }))
}

// ── Goals ──────────────────────────────────────────────────────────

/// Returns the current goal for a session as JSON.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_get_goal(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.get_goal(&sid).await {
            Ok(goal) => ctx.success(&goal),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Sets a thread goal for a session.
///
/// - `token_budget < 0` means no budget; otherwise must be positive.
/// - `short_description` may be null (optional compact UI label).
///
/// While the goal is Active, subsequent turns auto-continue until complete,
/// blocked, budget-limited, paused, or cleared.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_goal(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    objective: *const c_char,
    token_budget: i64,
    short_description: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let obj = match unsafe { cstr_to_str(objective) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("objective is null or invalid UTF-8");
            return;
        }
    };
    let budget = if token_budget < 0 {
        None
    } else {
        Some(token_budget)
    };
    let short = unsafe { cstr_to_str(short_description) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .set_goal_with_short_description(&sid, obj, short, budget)
            .await
        {
            Ok(goal) => ctx.success(&goal),
            Err(e) => ctx.error(&e.to_string()),
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
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.clear_goal(&sid).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Background Tasks ───────────────────────────────────────────────

/// Lists all active background bash commands.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_background_commands(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.list_background_commands(&sid).await {
            Ok(cmds) => ctx.success(&cmds),
            Err(e) => ctx.error(&e.to_string()),
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
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let tid = match parse_str(task_id, "task_id") {
        Some(s) => s,
        None => {
            let ctx = CallbackCtx::new(callback, user_data);
            ctx.error("task_id is null or invalid UTF-8");
            return;
        }
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.poll_background_command(&sid, &tid).await {
            Ok(snapshot) => ctx.success(&snapshot),
            Err(e) => ctx.error(&e.to_string()),
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
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let tid = match parse_str(task_id, "task_id") {
        Some(s) => s,
        None => {
            let ctx = CallbackCtx::new(callback, user_data);
            ctx.error("task_id is null or invalid UTF-8");
            return;
        }
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.cancel_background_command(&sid, &tid).await {
            Ok(snapshot) => ctx.success(&snapshot),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Providers & Credentials ────────────────────────────────────────

/// Lists all configured providers with credential status. Returns JSON. Must be freed with `navi_string_free`.
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

/// Returns credential status for a specific provider as JSON. Must be freed with `navi_string_free`.
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

/// Stores an API key. Returns 0 on success, -1 on error.
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

/// Deletes a stored API key. Returns 1 if removed, 0 if not found, -1 on error.
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
        Ok(deleted) => {
            if deleted {
                1
            } else {
                0
            }
        }
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

/// Fetches usage windows for the selected OpenAI account.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_usage_report(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.usage_report().await {
            Ok(report) => ctx.success(&report),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Skills ─────────────────────────────────────────────────────────

/// Lists discovered skills. Returns JSON. Must be freed with `navi_string_free`.
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

/// Sets the active skills for a session. `skills_json` is a JSON string array.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_session_skills(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    skills_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let sid = match unsafe { cstr_to_str(session_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("session_id is null or invalid UTF-8");
            return;
        }
    };
    let skills: Vec<String> =
        match unsafe { cstr_to_str(skills_json) }.and_then(|s| serde_json::from_str(s).ok()) {
            Some(v) => v,
            None => {
                ctx.error("invalid skills_json");
                return;
            }
        };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.set_session_skills(&sid, skills).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── MCP ────────────────────────────────────────────────────────────

/// Lists MCP servers for a session. Returns JSON. Must be freed with `navi_string_free`.
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

/// Lists MCP tool names. Returns JSON string array. Must be freed with `navi_string_free`.
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

/// Fetches the latest model list from a specific provider.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_sync_provider_models(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    save_target: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let pid = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("provider_id is null or invalid UTF-8");
            return;
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_provider_models(&pid, target).await {
            Ok(report) => ctx.success(&json!({
                "savedTo": report.saved_to.map(|p| p.display().to_string()),
                "updated": report.updated,
                "failed": report.failed,
                "skipped": report.skipped,
            })),
            Err(e) => ctx.error(&e.to_string()),
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
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_models(target).await {
            Ok(report) => ctx.success(&json!({
                "savedTo": report.saved_to.map(|p| p.display().to_string()),
                "updated": report.updated,
                "failed": report.failed,
                "skipped": report.skipped,
            })),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Registry ───────────────────────────────────────────────────────

/// Syncs the provider registry. `force` != 0 forces full re-sync.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_sync_registry(
    engine: *mut NaviDartEngine,
    force: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.sync_registry(force != 0).await {
            Ok(updated) => ctx.success(&updated),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Plugins ────────────────────────────────────────────────────────

/// Reloads WASM plugin tools on every active session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_reload_wasm_plugins(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.reload_wasm_plugins().await {
            Ok(warnings) => ctx.success(&warnings),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Saved Sessions ─────────────────────────────────────────────────

/// Lists all persisted sessions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_saved_sessions(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.list_saved_sessions_async().await {
            Ok(sessions) => ctx.success(&sessions),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Loads a persisted session by ID and reopens it in memory.
///
/// Rebuilds provider history via [`navi_sdk::session_request_from_snapshot`],
/// rehydrating `view_image` attachments from disk (image bytes are not stored
/// on `ToolCompleted` events).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_load_saved_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        // Already live — free path for re-focusing a session.
        if inner.session_ids().iter().any(|id| id == &sid) {
            ctx.success(&serde_json::json!({
                "id": sid,
                "restored": true,
                "already_active": true,
            }));
            return;
        }

        let snapshot = match inner.load_saved_session_async(&sid).await {
            Ok(s) => s,
            Err(e) => {
                ctx.error(&e.to_string());
                return;
            }
        };

        let project = snapshot.project.clone();
        let created_at = snapshot.created_at;
        let updated_at = snapshot.updated_at;
        let data_dir = inner.loaded_config().data_dir;
        let req = navi_sdk::session_request_from_snapshot(&snapshot, Some(data_dir.as_path()));
        match inner.start_session(req).await {
            Ok(_) => ctx.success(&serde_json::json!({
                "id": sid,
                "project": project,
                "created_at": created_at,
                "updated_at": updated_at,
                "restored": true,
                "already_active": false,
            })),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

/// Deletes a persisted session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_delete_saved_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.delete_saved_session_async(&sid).await {
            Ok(deleted) => ctx.success(&deleted),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Config ─────────────────────────────────────────────────────────

/// Returns the loaded configuration as JSON. Must be freed with `navi_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_loaded_config(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    let config = engine.inner.loaded_config();
    to_json_ptr(&json!({
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
    }))
}

// ── Internal helpers ───────────────────────────────────────────────

async fn event_loop(mut receiver: broadcast::Receiver<RuntimeEvent>, ctx: EventCtx) {
    loop {
        match receiver.recv().await {
            Ok(event) => match serde_json::to_string(&event) {
                Ok(json) => {
                    if ctx.emit(&json) {
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

async fn voice_event_loop(mut receiver: broadcast::Receiver<navi_sdk::VoiceEvent>, ctx: EventCtx) {
    loop {
        match receiver.recv().await {
            Ok(event) => match serde_json::to_string(&event) {
                Ok(json) => {
                    if ctx.emit(&json) {
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

/// Helper to parse session_id and return a String, or call error and return None.
fn parse_sid(
    ptr: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) -> Option<String> {
    match unsafe { cstr_to_str(ptr) } {
        Some(s) => Some(s.to_string()),
        None => {
            CallbackCtx::new(callback, user_data).error("session_id is null or invalid UTF-8");
            None
        }
    }
}

/// Helper to parse a generic C string parameter.
fn parse_str(ptr: *const c_char, name: &str) -> Option<String> {
    match unsafe { cstr_to_str(ptr) } {
        Some(s) => Some(s.to_string()),
        None => {
            set_last_error(&format!("{name} is null or invalid UTF-8"));
            None
        }
    }
}
