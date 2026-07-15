//! Extended C ABI surface: voice, memory ops, skills CRUD, plan mode,
//! plugins marketplace, MCP config, permissions, notify/update, registry list.
//!
//! Complements `engine.rs` so Dart/mobile can reach the full SDK.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr;

use navi_core::{
    GoalStatus, GoalTask, McpConfig, McpServerConfig, PlanReviewResponse, SkillWriteRequest,
    SudoPasswordResponse, TaskStatus,
};
use navi_sdk::{
    MemoryStatus, MemoryType, NaviConfigSaveTarget, NotificationUrgency, NotifyRequest,
    PermissionMode, VoiceConfigUpdate,
};
use serde_json::json;

use crate::engine::NaviDartEngine;
use crate::types::{
    CallbackCtx, NaviAsyncCallback, cstr_to_str, parse_save_target, set_last_error, to_json_ptr,
};

// ── Helpers ────────────────────────────────────────────────────────

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

fn parse_str(ptr: *const c_char, name: &str) -> Option<String> {
    match unsafe { cstr_to_str(ptr) } {
        Some(s) => Some(s.to_string()),
        None => {
            set_last_error(&format!("{name} is null or invalid UTF-8"));
            None
        }
    }
}

fn path_json(path: Option<std::path::PathBuf>) -> serde_json::Value {
    json!({ "savedTo": path.map(|p| p.display().to_string()) })
}

// ── Skills CRUD ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_get_skill(
    engine: *mut NaviDartEngine,
    skill_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let id = match parse_str(skill_id, "skill_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.get_skill(&id) {
        Ok(skill) => to_json_ptr(&skill),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_save_skill(
    engine: *mut NaviDartEngine,
    params_json: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let raw = match unsafe { cstr_to_str(params_json) } {
        Some(s) => s,
        None => {
            set_last_error("params_json is null or invalid UTF-8");
            return ptr::null_mut();
        }
    };
    let request: SkillWriteRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            set_last_error(&format!("invalid skill params: {e}"));
            return ptr::null_mut();
        }
    };
    match engine.inner.save_skill(request) {
        Ok(result) => to_json_ptr(&json!({
            "created": result.created,
            "path": result.path.display().to_string(),
            "skillId": result.skill.id,
        })),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_delete_skill(
    engine: *mut NaviDartEngine,
    skill_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(skill_id, "skill_id") {
        Some(s) => s,
        None => return 0,
    };
    match engine.inner.delete_skill(&id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

// ── Plan mode ──────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_agent_mode(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let sid = match parse_str(session_id, "session_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.agent_mode(&sid) {
        Ok(mode) => to_json_ptr(&mode.as_str()),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_enter_plan_mode(
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
        match inner.enter_plan_mode(&sid).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_exit_plan_mode(
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
        match inner.exit_plan_mode(&sid).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_resolve_plan_review(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    response_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let raw = match unsafe { cstr_to_str(response_json) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("response_json is null");
            return;
        }
    };
    let response: PlanReviewResponse = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            ctx.error(&format!("invalid plan review response: {e}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.resolve_plan_review(&sid, response).await {
            Ok(ok) => ctx.success(&ok),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_resolve_sudo_password(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    response_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let raw = match unsafe { cstr_to_str(response_json) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("response_json is null");
            return;
        }
    };
    let response: SudoPasswordResponse = match serde_json::from_str(&raw) {
        Ok(r) => r,
        Err(e) => {
            ctx.error(&format!("invalid sudo response: {e}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.resolve_sudo_password(&sid, response).await {
            Ok(ok) => ctx.success(&ok),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Goal updates ───────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_update_goal_status(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    status: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let status_str = match unsafe { cstr_to_str(status) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("status is null");
            return;
        }
    };
    let status = match status_str.to_lowercase().as_str() {
        "active" => GoalStatus::Active,
        "paused" => GoalStatus::Paused,
        "blocked" => GoalStatus::Blocked,
        "usage_limited" => GoalStatus::UsageLimited,
        "budget_limited" => GoalStatus::BudgetLimited,
        "complete" | "completed" | "done" => GoalStatus::Complete,
        other => {
            ctx.error(&format!("unknown goal status: {other}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.update_goal_status(&sid, status).await {
            Ok(goal) => ctx.success(&goal),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_update_goal_checklist(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    checklist_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let raw = match unsafe { cstr_to_str(checklist_json) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("checklist_json is null");
            return;
        }
    };
    let tasks: Vec<GoalTask> = match serde_json::from_str(&raw) {
        Ok(t) => t,
        Err(e) => {
            ctx.error(&format!("invalid checklist: {e}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.update_goal_checklist(&sid, tasks).await {
            Ok(goal) => ctx.success(&goal),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_update_goal_task_status(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    task_id: u32,
    status: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let status_str = match unsafe { cstr_to_str(status) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("status is null");
            return;
        }
    };
    let task_status = match status_str.to_lowercase().as_str() {
        "pending" => TaskStatus::Pending,
        "in_progress" | "inprogress" => TaskStatus::InProgress,
        "done" | "completed" => TaskStatus::Done,
        "verified" => TaskStatus::Verified,
        "skipped" => TaskStatus::Skipped,
        other => {
            ctx.error(&format!("unknown task status: {other}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .update_goal_task_status(&sid, task_id as usize, task_status)
            .await
        {
            Ok(goal) => ctx.success(&goal),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

// ── Voice ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_status(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.voice_status() {
        Ok(s) => to_json_ptr(&s),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_transcription_providers(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.voice_transcription_providers())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_voice_config(
    engine: *mut NaviDartEngine,
    update_json: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let raw = match unsafe { cstr_to_str(update_json) } {
        Some(s) => s,
        None => {
            set_last_error("update_json is null");
            return ptr::null_mut();
        }
    };
    let update: VoiceConfigUpdate = match serde_json::from_str(raw) {
        Ok(u) => u,
        Err(e) => {
            set_last_error(&format!("invalid voice update: {e}"));
            return ptr::null_mut();
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.set_voice_config(update, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_doctor(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.voice_doctor() {
        Ok(r) => to_json_ptr(&r),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_engine_installed(
    engine: *mut NaviDartEngine,
    engine_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = unsafe { cstr_to_str(engine_id) };
    match engine.inner.voice_engine_installed(id) {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_init(
    engine: *mut NaviDartEngine,
    engine_id: *const c_char,
    force: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let id = unsafe { cstr_to_str(engine_id) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.voice_init(id.as_deref(), force != 0).await {
            Ok(path) => ctx.success(&path.display().to_string()),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_transcribe_file(
    engine: *mut NaviDartEngine,
    path: *const c_char,
    language: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let path = match unsafe { cstr_to_str(path) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("path is null");
            return;
        }
    };
    let lang = unsafe { cstr_to_str(language) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .voice_transcribe_file_async(&path, lang.as_deref())
            .await
        {
            Ok(r) => ctx.success(&json!({ "text": r.text, "tokenIds": r.token_ids })),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_start_stream(
    engine: *mut NaviDartEngine,
    language: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let lang = unsafe { cstr_to_str(language) };
    match engine.inner.voice_start_stream(lang) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

/// `samples_json` is a JSON array of f32 samples (16 kHz mono).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_push_pcm(
    engine: *mut NaviDartEngine,
    samples_json: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let raw = match unsafe { cstr_to_str(samples_json) } {
        Some(s) => s,
        None => {
            set_last_error("samples_json is null");
            return ptr::null_mut();
        }
    };
    let samples: Vec<f32> = match serde_json::from_str(raw) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(&format!("invalid samples: {e}"));
            return ptr::null_mut();
        }
    };
    match engine.inner.voice_push_pcm(&samples) {
        Ok(delta) => to_json_ptr(&delta),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_end_stream(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.voice_end_stream() {
        Ok(text) => to_json_ptr(&text),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_cancel_stream(engine: *mut NaviDartEngine) -> i32 {
    let engine = unsafe { &*engine };
    match engine.inner.voice_cancel_stream() {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

// ── Memory ─────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_write(
    engine: *mut NaviDartEngine,
    id: *const c_char,
    memory_type: *const c_char,
    name: *const c_char,
    description: *const c_char,
    body: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(id, "id") {
        Some(s) => s,
        None => return -1,
    };
    let mt_raw = match parse_str(memory_type, "memory_type") {
        Some(s) => s,
        None => return -1,
    };
    let mt = match MemoryType::from_str(&mt_raw) {
        Some(t) => t,
        None => {
            set_last_error(&format!("invalid memory_type: {mt_raw}"));
            return -1;
        }
    };
    let name = match parse_str(name, "name") {
        Some(s) => s,
        None => return -1,
    };
    let description = unsafe { cstr_to_str(description) }.unwrap_or("").to_string();
    let body = unsafe { cstr_to_str(body) }.unwrap_or("").to_string();
    match engine
        .inner
        .memory_write(&id, mt, &name, &description, &body)
    {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_read(
    engine: *mut NaviDartEngine,
    id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let id = match parse_str(id, "id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.memory_read(&id) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_list(
    engine: *mut NaviDartEngine,
    status: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let filter = unsafe { cstr_to_str(status) }.and_then(MemoryStatus::from_str);
    match engine.inner.memory_list(filter) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_search(
    engine: *mut NaviDartEngine,
    query: *const c_char,
    limit: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let query = match parse_str(query, "query") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let lim = if limit > 0 { limit as usize } else { 20 };
    match engine.inner.memory_search(&query, lim) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_delete(
    engine: *mut NaviDartEngine,
    id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(id, "id") {
        Some(s) => s,
        None => return -1,
    };
    match engine.inner.memory_delete(&id) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_count(engine: *mut NaviDartEngine) -> i64 {
    let engine = unsafe { &*engine };
    engine.inner.memory_count().unwrap_or(0) as i64
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_index(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.memory_index())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_status(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.memory_status() {
        Ok(s) => to_json_ptr(&s),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_doctor(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.memory_doctor() {
        Ok(s) => to_json_ptr(&s),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_init(
    engine: *mut NaviDartEngine,
    embeddings: i32,
    force: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.memory_init(embeddings != 0, force != 0).await {
            Ok(r) => ctx.success(&r),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_history_search(
    engine: *mut NaviDartEngine,
    query: *const c_char,
    session_id: *const c_char,
    limit: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let query = match parse_str(query, "query") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let sid = unsafe { cstr_to_str(session_id) };
    let limit = if limit > 0 { Some(limit as i64) } else { None };
    match engine.inner.memory_history_search(&query, sid, limit) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_dream(
    engine: *mut NaviDartEngine,
    apply: i32,
    sessions: i32,
    instructions: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let instructions = unsafe { cstr_to_str(instructions) }.map(|s| s.to_string());
    let sessions = if sessions > 0 {
        sessions as usize
    } else {
        10
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .memory_dream(apply != 0, sessions, instructions)
            .await
        {
            Ok(r) => ctx.success(&r),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_distill(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.memory_distill().await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_checkpoint(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.memory_checkpoint().await {
            Ok(id) => ctx.success(&id),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_rebuild_preview(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.memory_rebuild_preview() {
        Ok(s) => to_json_ptr(&s),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

// ── Plugins marketplace ────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_list(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.plugin_list() {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_info(
    engine: *mut NaviDartEngine,
    plugin_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let id = match parse_str(plugin_id, "plugin_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.plugin_info(&id) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_search(
    engine: *mut NaviDartEngine,
    query: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let query = unsafe { cstr_to_str(query) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.plugin_search(query.as_deref()).await {
            Ok(v) => ctx.success(&v),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_install_path(
    engine: *mut NaviDartEngine,
    path: *const c_char,
    confirm: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let path = match parse_str(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine
        .inner
        .plugin_install_path(std::path::Path::new(&path), confirm != 0)
    {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_install_marketplace(
    engine: *mut NaviDartEngine,
    plugin_id: *const c_char,
    confirm: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let id = match unsafe { cstr_to_str(plugin_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("plugin_id is null");
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.plugin_install_marketplace(&id, confirm != 0).await {
            Ok(v) => ctx.success(&v),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_remove(
    engine: *mut NaviDartEngine,
    plugin_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(plugin_id, "plugin_id") {
        Some(s) => s,
        None => return -1,
    };
    match engine.inner.plugin_remove(&id) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

// ── MCP config ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_mcp_config(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.list_mcp_config())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_mcp_enabled(
    engine: *mut NaviDartEngine,
    enabled: i32,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.set_mcp_enabled(enabled != 0, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_upsert_mcp_server(
    engine: *mut NaviDartEngine,
    server_json: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let raw = match unsafe { cstr_to_str(server_json) } {
        Some(s) => s,
        None => {
            set_last_error("server_json is null");
            return ptr::null_mut();
        }
    };
    let server: McpServerConfig = match serde_json::from_str(raw) {
        Ok(s) => s,
        Err(e) => {
            set_last_error(&format!("invalid server: {e}"));
            return ptr::null_mut();
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.upsert_mcp_server(server, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_remove_mcp_server(
    engine: *mut NaviDartEngine,
    server_id: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let id = match parse_str(server_id, "server_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.remove_mcp_server(&id, target) {
        Ok((removed, path)) => to_json_ptr(&json!({
            "removed": removed,
            "savedTo": path.map(|p| p.display().to_string()),
        })),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_mcp_config(
    engine: *mut NaviDartEngine,
    mcp_json: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let raw = match unsafe { cstr_to_str(mcp_json) } {
        Some(s) => s,
        None => {
            set_last_error("mcp_json is null");
            return ptr::null_mut();
        }
    };
    let mcp: McpConfig = match serde_json::from_str(raw) {
        Ok(m) => m,
        Err(e) => {
            set_last_error(&format!("invalid mcp config: {e}"));
            return ptr::null_mut();
        }
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.set_mcp_config(mcp, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

// ── Registry list / rename session / permissions / oauth / notify ──

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_registry(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.list_registry() {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_rename_saved_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    title: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let title = match unsafe { cstr_to_str(title) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("title is null");
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.rename_saved_session_async(&sid, &title).await {
            Ok(ok) => ctx.success(&ok),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_get_permission_mode(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let mode = match engine.inner.get_permission_mode() {
        PermissionMode::Restricted => "restricted",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
    };
    to_json_ptr(&mode)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_permission_mode(
    engine: *mut NaviDartEngine,
    mode: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let mode = match unsafe { cstr_to_str(mode) } {
        Some(s) => s,
        None => {
            set_last_error("mode is null");
            return -1;
        }
    };
    let pm = match mode {
        "restricted" => PermissionMode::Restricted,
        "accept-edits" => PermissionMode::AcceptEdits,
        "auto" => PermissionMode::Auto,
        "yolo" => PermissionMode::Yolo,
        other => {
            set_last_error(&format!("invalid permission mode: {other}"));
            return -1;
        }
    };
    engine.runtime.block_on(async {
        if let Err(e) = engine.inner.set_permission_mode(pm).await {
            set_last_error(&e.to_string());
            return -1;
        }
        0
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_provider_supports_device_oauth(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(provider_id, "provider_id") {
        Some(s) => s,
        None => return 0,
    };
    if engine.inner.provider_supports_device_oauth(&id) {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_start_device_oauth_simple(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let id = match unsafe { cstr_to_str(provider_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("provider_id is null");
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.start_device_oauth_simple(&id).await {
            Ok(token) => ctx.success(&token),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_notify(
    engine: *mut NaviDartEngine,
    title: *const c_char,
    body: *const c_char,
    desktop: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let title = match parse_str(title, "title") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let body = unsafe { cstr_to_str(body) }.unwrap_or("").to_string();
    let req = NotifyRequest {
        title,
        body,
        urgency: NotificationUrgency::Normal,
        category: None,
    };
    match engine.inner.notify(req, desktop != 0) {
        Ok(r) => to_json_ptr(&r),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_open_url(
    engine: *mut NaviDartEngine,
    url: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let url = match parse_str(url, "url") {
        Some(s) => s,
        None => return -1,
    };
    match engine.inner.open_url(&url) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_app_version(engine: *mut NaviDartEngine) -> *mut c_char {
    let engine = unsafe { &*engine };
    to_json_ptr(&engine.inner.app_version())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_check_for_update(
    engine: *mut NaviDartEngine,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.check_for_update().await {
            Ok(info) => ctx.success(&info),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_auto_update_enabled(engine: *mut NaviDartEngine) -> i32 {
    let engine = unsafe { &*engine };
    if engine.inner.auto_update_enabled() {
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_auto_update(
    engine: *mut NaviDartEngine,
    enabled: i32,
) -> i32 {
    let engine = unsafe { &*engine };
    match engine.inner.set_auto_update(enabled != 0) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

// Avoid unused-import warnings if NotifyRequest fields differ across versions.
#[allow(dead_code)]
fn _save_target_default() -> NaviConfigSaveTarget {
    NaviConfigSaveTarget::Auto
}


// ── Surface gap-fill (SDK parity) ──────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_memory_update(
    engine: *mut NaviDartEngine,
    id: *const c_char,
    name: *const c_char,
    description: *const c_char,
    body: *const c_char,
    status: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let id = match parse_str(id, "id") {
        Some(s) => s,
        None => return -1,
    };
    let name = unsafe { cstr_to_str(name) }.map(|s| s.to_string());
    let description = unsafe { cstr_to_str(description) }.map(|s| s.to_string());
    let body = unsafe { cstr_to_str(body) }.map(|s| s.to_string());
    let st = unsafe { cstr_to_str(status) }.and_then(MemoryStatus::from_str);
    match engine.inner.memory_update(
        &id,
        name.as_deref(),
        description.as_deref(),
        body.as_deref(),
        st,
    ) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_notify_simple(
    engine: *mut NaviDartEngine,
    title: *const c_char,
    body: *const c_char,
    desktop: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let title = match parse_str(title, "title") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let body = unsafe { cstr_to_str(body) }.unwrap_or("").to_string();
    match engine
        .inner
        .notify_simple(title, body, desktop != 0)
    {
        Ok(r) => to_json_ptr(&r),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_credential_accounts(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let pid = match parse_str(provider_id, "provider_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.list_credential_accounts(&pid) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_add_provider_account(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    api_key: *const c_char,
    label: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let pid = match parse_str(provider_id, "provider_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let key = match parse_str(api_key, "api_key") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let label = unsafe { cstr_to_str(label) };
    match engine.inner.add_provider_account(&pid, &key, label) {
        Ok(id) => to_json_ptr(&id),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_select_provider_account(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    account_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let pid = match parse_str(provider_id, "provider_id") {
        Some(s) => s,
        None => return -1,
    };
    let aid = match parse_str(account_id, "account_id") {
        Some(s) => s,
        None => return -1,
    };
    match engine.inner.select_provider_account(&pid, &aid) {
        Ok(()) => 0,
        Err(e) => {
            set_last_error(&e.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_delete_provider_account(
    engine: *mut NaviDartEngine,
    provider_id: *const c_char,
    account_id: *const c_char,
) -> i32 {
    let engine = unsafe { &*engine };
    let pid = match parse_str(provider_id, "provider_id") {
        Some(s) => s,
        None => return -1,
    };
    let aid = match parse_str(account_id, "account_id") {
        Some(s) => s,
        None => return -1,
    };
    match engine.inner.delete_provider_account(&pid, &aid) {
        Ok(removed) => {
            if removed {
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

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_attachment_model(
    engine: *mut NaviDartEngine,
    modality: *const c_char,
    provider: *const c_char,
    model: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let modality = match parse_str(modality, "modality") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let provider = match parse_str(provider, "provider") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let model = match parse_str(model, "model") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine
        .inner
        .set_attachment_model(&modality, &provider, &model, target)
    {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_clear_attachment_model(
    engine: *mut NaviDartEngine,
    modality: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let modality = match parse_str(modality, "modality") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.clear_attachment_model(&modality, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_set_background_model(
    engine: *mut NaviDartEngine,
    task: *const c_char,
    provider: *const c_char,
    model: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let task = match parse_str(task, "task") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let provider = match parse_str(provider, "provider") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let model = match parse_str(model, "model") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine
        .inner
        .set_background_model(&task, &provider, &model, target)
    {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_clear_background_model(
    engine: *mut NaviDartEngine,
    task: *const c_char,
    save_target: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let task = match parse_str(task, "task") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let target = parse_save_target(unsafe { cstr_to_str(save_target) });
    match engine.inner.clear_background_model(&task, target) {
        Ok(path) => to_json_ptr(&path_json(path)),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_install_path_with_meta(
    engine: *mut NaviDartEngine,
    path: *const c_char,
    confirm: i32,
    trust: *const c_char,
    kind: *const c_char,
) -> *mut c_char {
    use navi_plugin_manifest::{PluginCatalogKind, TrustLevel};
    let engine = unsafe { &*engine };
    let path = match parse_str(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let trust = match unsafe { cstr_to_str(trust) }.unwrap_or("local-dev") {
        "local-dev" | "local_dev" | "localdev" => TrustLevel::LocalDev,
        "community" => TrustLevel::Community,
        "signed" => TrustLevel::Signed,
        "core" => TrustLevel::Core,
        other => {
            set_last_error(&format!(
                "invalid trust level '{other}' (expected local-dev|community|signed|core)"
            ));
            return ptr::null_mut();
        }
    };
    let kind = match unsafe { cstr_to_str(kind) }.unwrap_or("plugin") {
        "plugin" => PluginCatalogKind::Plugin,
        "skill" => PluginCatalogKind::Skill,
        "mcp" => PluginCatalogKind::Mcp,
        "integration" => PluginCatalogKind::Integration,
        other => {
            set_last_error(&format!(
                "invalid package kind '{other}' (expected plugin|skill|mcp|integration)"
            ));
            return ptr::null_mut();
        }
    };
    match engine.inner.plugin_install_path_with_meta(
        std::path::Path::new(&path),
        confirm != 0,
        trust,
        kind,
    ) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_update_path(
    engine: *mut NaviDartEngine,
    path: *const c_char,
    force: i32,
    confirm: i32,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let path = match parse_str(path, "path") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.plugin_update_path(
        std::path::Path::new(&path),
        force != 0,
        confirm != 0,
    ) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_plugin_update_marketplace(
    engine: *mut NaviDartEngine,
    plugin_id: *const c_char,
    force: i32,
    confirm: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let id = match unsafe { cstr_to_str(plugin_id) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("plugin_id is null");
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .plugin_update_marketplace(&id, force != 0, confirm != 0)
            .await
        {
            Ok(v) => ctx.success(&v),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_tui_extensions(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.list_tui_extensions() {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_tui_extension_commands(
    engine: *mut NaviDartEngine,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    match engine.inner.list_tui_extension_commands() {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_list_tui_components(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let sid = match parse_str(session_id, "session_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.list_tui_components(&sid) {
        Ok(v) => to_json_ptr(&v),
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_take_tui_panels(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*engine };
    let sid = match parse_str(session_id, "session_id") {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    match engine.inner.take_tui_panels(&sid) {
        Ok(panels) => {
            let ids: Vec<String> = panels.iter().map(|p| p.id().to_string()).collect();
            to_json_ptr(&ids)
        }
        Err(e) => {
            set_last_error(&e.to_string());
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_rewind_session(
    engine: *mut NaviDartEngine,
    session_id: *const c_char,
    keep_user_turns: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let sid = match parse_sid(session_id, callback, user_data) {
        Some(s) => s,
        None => return,
    };
    let ctx = CallbackCtx::new(callback, user_data);
    let keep = keep_user_turns.max(0) as usize;
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.rewind_session(&sid, keep).await {
            Ok(n) => ctx.success(&n),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_apply_update(
    engine: *mut NaviDartEngine,
    info_json: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let raw = match unsafe { cstr_to_str(info_json) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("info_json is null");
            return;
        }
    };
    let info: navi_core::UpdateInfo = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            ctx.error(&format!("invalid UpdateInfo JSON: {e}"));
            return;
        }
    };
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner.apply_update(&info).await {
            Ok(()) => ctx.success_str("null"),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_check_for_update_with(
    engine: *mut NaviDartEngine,
    current: *const c_char,
    repo: *const c_char,
    include_prerelease: i32,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let current = match unsafe { cstr_to_str(current) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("current is null");
            return;
        }
    };
    let repo = unsafe { cstr_to_str(repo) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .check_for_update_with(&current, repo.as_deref(), include_prerelease != 0)
            .await
        {
            Ok(info) => ctx.success(&info),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn navi_engine_voice_transcribe_file_async(
    engine: *mut NaviDartEngine,
    path: *const c_char,
    language: *const c_char,
    callback: NaviAsyncCallback,
    user_data: *mut c_void,
) {
    let engine = unsafe { &*engine };
    let ctx = CallbackCtx::new(callback, user_data);
    let path = match unsafe { cstr_to_str(path) } {
        Some(s) => s.to_string(),
        None => {
            ctx.error("path is null");
            return;
        }
    };
    let language = unsafe { cstr_to_str(language) }.map(|s| s.to_string());
    let inner = engine.inner.clone();
    engine.runtime.spawn(async move {
        match inner
            .voice_transcribe_file_async(&path, language.as_deref())
            .await
        {
            Ok(result) => ctx.success(&json!({
                "text": result.text,
                "tokenIds": result.token_ids,
            })),
            Err(e) => ctx.error(&e.to_string()),
        }
    });
}
