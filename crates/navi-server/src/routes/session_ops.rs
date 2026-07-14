//! Session ops HTTP routes: plan mode, sudo, permission mode, rewind,
//! context packets, goal extensions, background poll/cancel, rename.

use crate::state::{SharedState, err_resp, ok_json, with_auth, with_state};
use navi_core::event::{PlanReviewDecision, PlanReviewResponse, SudoPasswordResponse};
use navi_core::goal::types::{GoalStatus, GoalTask, TaskStatus};
use navi_core::{ContextPacket, PermissionMode};
use serde::Deserialize;
use std::convert::Infallible;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct PlanReviewBody {
    id: String,
    #[serde(alias = "planId")]
    plan_id: String,
    decision: String,
    #[serde(default)]
    comments: Vec<navi_core::plan_store::PlanLineComment>,
    #[serde(default)]
    freeform: String,
}

/// Sudo resolve body. Password must never be logged.
#[derive(Deserialize)]
struct SudoBody {
    id: String,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Deserialize)]
struct RewindBody {
    #[serde(alias = "keepUserTurns")]
    keep_user_turns: usize,
}

#[derive(Deserialize)]
struct GoalStatusBody {
    status: String,
}

#[derive(Deserialize)]
struct GoalChecklistBody {
    tasks: Vec<GoalTask>,
}

#[derive(Deserialize)]
struct GoalTaskStatusBody {
    status: String,
}

#[derive(Deserialize)]
struct PermissionModeBody {
    mode: String,
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

// ── Enum parsers (case-insensitive) ──────────────────────────────────────

pub(crate) fn parse_plan_review_decision(raw: &str) -> Result<PlanReviewDecision, String> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "approve" => Ok(PlanReviewDecision::Approve),
        "request_changes" => Ok(PlanReviewDecision::RequestChanges),
        "quit" => Ok(PlanReviewDecision::Quit),
        other => Err(format!("invalid plan review decision: {other}")),
    }
}

pub(crate) fn parse_goal_status(raw: &str) -> Result<GoalStatus, String> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "active" => Ok(GoalStatus::Active),
        "paused" => Ok(GoalStatus::Paused),
        "blocked" => Ok(GoalStatus::Blocked),
        "usage_limited" => Ok(GoalStatus::UsageLimited),
        "budget_limited" => Ok(GoalStatus::BudgetLimited),
        "complete" | "completed" | "done" => Ok(GoalStatus::Complete),
        other => Err(format!("invalid goal status: {other}")),
    }
}

pub(crate) fn parse_task_status(raw: &str) -> Result<TaskStatus, String> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "pending" => Ok(TaskStatus::Pending),
        "in_progress" => Ok(TaskStatus::InProgress),
        "done" => Ok(TaskStatus::Done),
        "verified" => Ok(TaskStatus::Verified),
        "skipped" => Ok(TaskStatus::Skipped),
        other => Err(format!("invalid task status: {other}")),
    }
}

pub(crate) fn parse_permission_mode(raw: &str) -> Result<PermissionMode, String> {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "restricted" => Ok(PermissionMode::Restricted),
        "accept_edits" => Ok(PermissionMode::AcceptEdits),
        "auto" => Ok(PermissionMode::Auto),
        "yolo" => Ok(PermissionMode::Yolo),
        other => Err(format!("invalid permission mode: {other}")),
    }
}

pub(crate) fn permission_mode_str(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Restricted => "restricted",
        PermissionMode::AcceptEdits => "accept_edits",
        PermissionMode::Auto => "auto",
        PermissionMode::Yolo => "yolo",
    }
}

// ── Routes ───────────────────────────────────────────────────────────────

pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // GET /sessions/:id/mode
    let get_mode = warp::path!("sessions" / String / "mode")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.agent_mode(&sid) {
                Ok(mode) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({"mode": mode.as_str()})).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /sessions/:id/plan/enter
    let plan_enter = warp::path!("sessions" / String / "plan" / "enter")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.enter_plan_mode(&sid).await {
                Ok(()) => Ok::<_, Infallible>(ok_json("plan mode entered")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /sessions/:id/plan/exit
    let plan_exit = warp::path!("sessions" / String / "plan" / "exit")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.exit_plan_mode(&sid).await {
                Ok(()) => Ok::<_, Infallible>(ok_json("plan mode exited")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /sessions/:id/plan/review
    let plan_review = warp::path!("sessions" / String / "plan" / "review")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |sid: String, body: PlanReviewBody, s: SharedState| async move {
                let decision = match parse_plan_review_decision(&body.decision) {
                    Ok(d) => d,
                    Err(msg) => {
                        return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                    }
                };
                let response = PlanReviewResponse {
                    id: body.id,
                    plan_id: body.plan_id,
                    decision,
                    comments: body.comments,
                    freeform: body.freeform,
                };
                let engine = s.engine.read().await;
                match engine.resolve_plan_review(&sid, response).await {
                    Ok(consumed) => Ok(warp::reply::json(&serde_json::json!({
                        "consumed": consumed
                    }))
                    .into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /sessions/:id/sudo — password never logged
    let sudo = warp::path!("sessions" / String / "sudo")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, body: SudoBody, s: SharedState| async move {
            // Empty / whitespace password is treated as cancel (not submit empty secret).
            let response = match body.password {
                Some(password) if !password.trim().is_empty() => {
                    SudoPasswordResponse::Submitted {
                        id: body.id,
                        password,
                    }
                }
                _ => SudoPasswordResponse::Cancelled { id: body.id },
            };
            let engine = s.engine.read().await;
            match engine.resolve_sudo_password(&sid, response).await {
                Ok(consumed) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({
                        "consumed": consumed
                    }))
                    .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /sessions/:id/context
    let add_context = warp::path!("sessions" / String / "context")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |sid: String, packet: ContextPacket, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.add_context_packet(&sid, packet).await {
                    Ok(()) => Ok::<_, Infallible>(ok_json("context added")),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /sessions/:id/rewind
    let rewind = warp::path!("sessions" / String / "rewind")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, body: RewindBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.rewind_session(&sid, body.keep_user_turns).await {
                Ok(n) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({
                        "remainingMessages": n
                    }))
                    .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /sessions/:id/goal/status
    let goal_status = warp::path!("sessions" / String / "goal" / "status")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |sid: String, body: GoalStatusBody, s: SharedState| async move {
                let status = match parse_goal_status(&body.status) {
                    Ok(st) => st,
                    Err(msg) => {
                        return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                    }
                };
                let engine = s.engine.read().await;
                match engine.update_goal_status(&sid, status).await {
                    Ok(goal) => Ok(warp::reply::json(&goal).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /sessions/:id/goal/checklist
    let goal_checklist = warp::path!("sessions" / String / "goal" / "checklist")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |sid: String, body: GoalChecklistBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.update_goal_checklist(&sid, body.tasks).await {
                    Ok(goal) => Ok::<_, Infallible>(warp::reply::json(&goal).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /sessions/:id/goal/tasks/:taskId
    let goal_task = warp::path!("sessions" / String / "goal" / "tasks" / usize)
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |sid: String, task_id: usize, body: GoalTaskStatusBody, s: SharedState| async move {
                let status = match parse_task_status(&body.status) {
                    Ok(st) => st,
                    Err(msg) => {
                        return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                    }
                };
                let engine = s.engine.read().await;
                match engine
                    .update_goal_task_status(&sid, task_id, status)
                    .await
                {
                    Ok(goal) => Ok(warp::reply::json(&goal).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // GET /permission-mode
    let permission_get = warp::path("permission-mode")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            let mode = engine.get_permission_mode();
            Ok::<_, Infallible>(
                warp::reply::json(&serde_json::json!({
                    "mode": permission_mode_str(mode)
                }))
                .into_response(),
            )
        });

    // POST /permission-mode
    let permission_set = warp::path("permission-mode")
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: PermissionModeBody, s: SharedState| async move {
            let mode = match parse_permission_mode(&body.mode) {
                Ok(m) => m,
                Err(msg) => {
                    return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                }
            };
            let engine = s.engine.read().await;
            if let Err(e) = engine.set_permission_mode(mode).await {
                return Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR));
            }
            Ok(warp::reply::json(&serde_json::json!({
                "mode": permission_mode_str(mode)
            }))
            .into_response())
        });

    // POST /sessions/:id/background/:taskId/cancel — register before the shorter poll path.
    let bg_cancel = warp::path!("sessions" / String / "background" / String / "cancel")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, task_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.cancel_background_command(&sid, &task_id).await {
                Ok(snap) => Ok::<_, Infallible>(warp::reply::json(&snap).into_response()),
                Err(e) => {
                    let msg = e.to_string();
                    let code = if msg.contains("not found") {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::BAD_REQUEST
                    };
                    Ok(err_resp(msg, code))
                }
            }
        });

    // GET /sessions/:id/background/:taskId
    let bg_poll = warp::path!("sessions" / String / "background" / String)
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, task_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.poll_background_command(&sid, &task_id).await {
                Ok(snap) => Ok::<_, Infallible>(warp::reply::json(&snap).into_response()),
                Err(e) => {
                    let msg = e.to_string();
                    let code = if msg.contains("not found") {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::BAD_REQUEST
                    };
                    Ok(err_resp(msg, code))
                }
            }
        });

    // POST /sessions/:id/rename
    let rename = warp::path!("sessions" / String / "rename")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, body: RenameBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.rename_saved_session(&sid, &body.title) {
                Ok(renamed) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({
                        "renamed": renamed
                    }))
                    .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    get_mode
        .or(plan_enter)
        .or(plan_exit)
        .or(plan_review)
        .or(sudo)
        .or(add_context)
        .or(rewind)
        .or(goal_status)
        .or(goal_checklist)
        .or(goal_task)
        .or(permission_get)
        .or(permission_set)
        .or(bg_cancel)
        .or(bg_poll)
        .or(rename)
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_decision_aliases() {
        assert_eq!(
            parse_plan_review_decision("Request-Changes").unwrap(),
            PlanReviewDecision::RequestChanges
        );
        assert!(parse_plan_review_decision("maybe").is_err());
    }

    #[test]
    fn goal_status_complete_aliases() {
        assert_eq!(parse_goal_status("done").unwrap(), GoalStatus::Complete);
        assert_eq!(
            parse_goal_status("usage-limited").unwrap(),
            GoalStatus::UsageLimited
        );
    }

    #[test]
    fn permission_mode_roundtrip_strings() {
        for mode in [
            PermissionMode::Restricted,
            PermissionMode::AcceptEdits,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            let s = permission_mode_str(mode);
            assert_eq!(parse_permission_mode(s).unwrap(), mode);
        }
        assert_eq!(
            parse_permission_mode("ACCEPT-EDITS").unwrap(),
            PermissionMode::AcceptEdits
        );
    }
}
