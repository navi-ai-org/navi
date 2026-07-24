//! HTTP/WebSocket server core: session lifecycle, turns, and route wiring.

use crate::routes;
use crate::state::{
    AppState, NaviServerConfig, SharedState, err_resp, handle_rejection, ok_json, with_auth,
    with_state,
};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use navi_sdk::{ApprovalDecision, NaviEngineBuilder, NaviTurnRequest};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};
use warp::Filter;
use warp::Reply;
use warp::http::StatusCode;
use warp::ws::{Message, WebSocket};

// ── Request DTOs (core session surface) ──────────────────────────────────

#[derive(Deserialize)]
struct StartSessionBody {
    #[serde(default, alias = "projectDir")]
    project_dir: Option<PathBuf>,
    #[serde(default, alias = "sessionId")]
    session_id: Option<String>,
    #[serde(default, alias = "activeSkills")]
    active_skills: Vec<String>,
}

#[derive(Deserialize)]
struct TurnBody {
    message: String,
    #[serde(default, alias = "contentParts")]
    content_parts: Vec<navi_core::ContentPart>,
    #[serde(default)]
    thinking: Option<navi_core::ThinkingConfig>,
}

#[derive(Deserialize)]
struct ApprovalBody {
    #[serde(alias = "requestId")]
    request_id: String,
    approved: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct QuestionBody {
    #[serde(alias = "questionId")]
    question_id: String,
    answer: String,
    #[serde(default)]
    custom: Option<String>,
}

#[derive(Deserialize)]
struct SelectModelBody {
    #[serde(alias = "providerId")]
    provider_id: String,
    model: String,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Deserialize)]
struct SyncModelsBody {
    #[serde(alias = "providerId")]
    provider_id: String,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Deserialize)]
struct SetGoalBody {
    objective: String,
    #[serde(default, alias = "tokenBudget")]
    token_budget: Option<i64>,
}

#[derive(Deserialize)]
struct SetSkillsBody {
    skills: Vec<String>,
}

#[derive(Deserialize)]
struct SetModelBody {
    /// Provider id (accept camelCase aliases from mobile clients).
    #[serde(alias = "providerId", alias = "provider_id")]
    provider: String,
    model: String,
}

// ── Server ───────────────────────────────────────────────────────────────

pub struct NaviServer {
    config: NaviServerConfig,
}

impl NaviServer {
    pub async fn new(config: NaviServerConfig) -> Result<Self> {
        Ok(Self { config })
    }

    pub async fn run(self) -> Result<()> {
        let project_dir = PathBuf::from(&self.config.project_dir)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&self.config.project_dir));

        info!(
            "Starting NAVI server for project: {}",
            project_dir.display()
        );

        let engine = NaviEngineBuilder::from_project(&project_dir).build()?;
        let secret: &'static str = Box::leak(self.config.shared_secret.clone().into_boxed_str());
        let state = Arc::new(AppState::new(
            engine,
            self.config.shared_secret.clone(),
            project_dir.clone(),
        ));

        let sf = with_state(state.clone());
        let af = with_auth(secret);

        // ── Health (no auth) ─────────────────────────────────────────────
        let health = warp::path("health")
            .and(warp::get())
            .map(|| warp::reply::json(&serde_json::json!({"status": "ok"})));

        // ── Models ───────────────────────────────────────────────────────
        let models = warp::path("models")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                let models = engine.list_models();
                Ok::<_, Infallible>(warp::reply::json(&models).into_response())
            });

        // ── Config ───────────────────────────────────────────────────────
        let get_config = warp::path("config")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                let lc = engine.loaded_config();
                Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({
                        "model": {
                            "provider": lc.config.model.provider,
                            "name": lc.config.model.name,
                        },
                        "projectDir": s.home_project,
                        "project_dir": s.home_project,
                        "projectConfigPath": lc.project_config_path,
                        "dataDir": lc.data_dir,
                        "data_dir": lc.data_dir,
                    }))
                    .into_response(),
                )
            });

        // ── Skills list (also expanded in skills_mcp module) ─────────────
        let skills = warp::path("skills")
            .and(warp::path::end())
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.list_skills() {
                    Ok(skills) => Ok::<_, Infallible>(warp::reply::json(&skills).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        // ── Sessions list ────────────────────────────────────────────────
        let list_sessions = warp::path("sessions")
            .and(warp::path::end())
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                let ids = engine.session_ids();
                Ok::<_, Infallible>(warp::reply::json(&ids).into_response())
            });

        // ── Start session ────────────────────────────────────────────────
        let start_session = warp::path("sessions")
            .and(warp::path::end())
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|body: StartSessionBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let req = navi_sdk::NaviSessionRequest {
                    project_dir: body.project_dir,
                    session_id: body.session_id,
                    active_skills: body.active_skills,
                    ..Default::default()
                };
                match engine.start_session(req).await {
                    Ok(info) => {
                        let tx = s.register_sender(&info.id).await;
                        let engine_clone = s.engine.clone();
                        let session_id_clone = info.id.clone();
                        tokio::spawn(async move {
                            let rx = {
                                let eng = engine_clone.read().await;
                                eng.subscribe_events(&session_id_clone)
                            };
                            if let Ok(mut rx) = rx {
                                while let Ok(event) = rx.recv().await {
                                    let _ = tx.send(event);
                                }
                            }
                        });
                        Ok::<_, Infallible>(
                            warp::reply::with_status(
                                warp::reply::json(&serde_json::json!({
                                    "id": info.id,
                                    "projectDir": info.project_dir,
                                    "model": info.model,
                                    "provider": info.provider,
                                })),
                                StatusCode::CREATED,
                            )
                            .into_response(),
                        )
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        // ── Saved sessions ───────────────────────────────────────────────
        let list_saved = warp::path!("sessions" / "saved")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.list_saved_sessions() {
                    Ok(saved) => Ok::<_, Infallible>(warp::reply::json(&saved).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        let load_saved = warp::path!("sessions" / "load" / String)
            .and(warp::post())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|session_id: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                let snapshot = match engine.load_saved_session(&session_id) {
                    Ok(snap) => snap,
                    Err(e) => {
                        return Ok::<_, Infallible>(err_resp(e.to_string(), StatusCode::NOT_FOUND));
                    }
                };

                // Rebuild provider history (path first, then durable attachment store).
                let data_dir = engine.loaded_config().data_dir;
                let req =
                    navi_sdk::session_request_from_snapshot(&snapshot, Some(data_dir.as_path()));

                match engine.start_session(req).await {
                    Ok(info) => {
                        let tx = s.register_sender(&info.id).await;
                        let engine_clone = s.engine.clone();
                        let session_id_clone = info.id.clone();
                        tokio::spawn(async move {
                            let rx = {
                                let eng = engine_clone.read().await;
                                eng.subscribe_events(&session_id_clone)
                            };
                            if let Ok(mut rx) = rx {
                                while let Ok(event) = rx.recv().await {
                                    let _ = tx.send(event);
                                }
                            }
                        });
                        Ok(warp::reply::json(&serde_json::json!({
                            "id": info.id,
                            "projectDir": info.project_dir,
                            "model": info.model,
                            "provider": info.provider,
                            "title": snapshot.title,
                            "snapshot": snapshot,
                        }))
                        .into_response())
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        let session_info = warp::path!("sessions" / String)
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                let ids = engine.session_ids();
                if ids.contains(&sid) {
                    let lc = engine.loaded_config();
                    Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "id": sid,
                            "model": lc.config.model.name,
                            "provider": lc.config.model.provider,
                        }))
                        .into_response(),
                    )
                } else {
                    Ok(err_resp(
                        format!("session not found: {sid}"),
                        StatusCode::NOT_FOUND,
                    ))
                }
            });

        let send_turn = warp::path!("sessions" / String / "turns")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, body: TurnBody, s: SharedState| async move {
                // Clone engine and run the turn on a detached task so dropping the
                // HTTP connection (phone app backgrounded/killed) does NOT cancel
                // the agent mid-turn. Events still flow over the session WebSocket.
                let engine = s.engine.read().await.clone();
                let req = NaviTurnRequest {
                    session_id: sid,
                    message: body.message,
                    content_parts: body.content_parts,
                    context_packets: Vec::new(),
                    thinking: body.thinking,
                };
                let join = tokio::spawn(async move { engine.send_turn(req).await });
                match join.await {
                    Ok(Ok(resp)) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "sessionId": resp.session_id,
                            "text": resp.text,
                        }))
                        .into_response(),
                    ),
                    Ok(Err(e)) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                    Err(e) => Ok(err_resp(
                        format!("turn task failed: {e}"),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )),
                }
            });

        let close_session = warp::path!("sessions" / String / "close")
            .and(warp::post())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.close_session(&sid).await {
                    Ok(_) => {
                        s.remove_sender(&sid).await;
                        Ok::<_, Infallible>(ok_json("closed"))
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let cancel_turn = warp::path!("sessions" / String / "cancel")
            .and(warp::post())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.cancel_turn(&sid).await {
                    Ok(_) => Ok::<_, Infallible>(ok_json("cancelled")),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let approve = warp::path!("sessions" / String / "approve")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, body: ApprovalBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let decision = if body.approved {
                    if let Some(ref msg) = body.message {
                        tracing::info!(session = %sid, request = %body.request_id, "approval approved: {msg}");
                    }
                    ApprovalDecision::Approved {
                        id: body.request_id.clone(),
                    }
                } else {
                    if let Some(ref msg) = body.message {
                        tracing::info!(session = %sid, request = %body.request_id, "approval denied: {msg}");
                    }
                    ApprovalDecision::Denied {
                        id: body.request_id,
                    }
                };
                match engine.resolve_approval(&sid, decision).await {
                    Ok(consumed) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "consumed": consumed
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let deny = warp::path!("sessions" / String / "deny")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |sid: String, body: ApprovalBody, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    let decision = ApprovalDecision::Denied {
                        id: body.request_id,
                    };
                    match engine.resolve_approval(&sid, decision).await {
                        Ok(consumed) => Ok::<_, Infallible>(
                            warp::reply::json(&serde_json::json!({
                                "consumed": consumed
                            }))
                            .into_response(),
                        ),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                    }
                },
            );

        let question = warp::path!("sessions" / String / "question")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |sid: String, body: QuestionBody, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    let response = if body.answer.is_empty() {
                        navi_core::QuestionResponse::Dismissed {
                            id: body.question_id,
                        }
                    } else {
                        let mut answers = vec![body.answer];
                        if let Some(custom) = body.custom
                            && !custom.is_empty()
                        {
                            answers.push(custom);
                        }
                        navi_core::QuestionResponse::Answered {
                            id: body.question_id,
                            answers,
                        }
                    };
                    match engine.resolve_question(&sid, response).await {
                        Ok(consumed) => Ok::<_, Infallible>(
                            warp::reply::json(&serde_json::json!({
                                "consumed": consumed
                            }))
                            .into_response(),
                        ),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                    }
                },
            );

        let set_goal = warp::path!("sessions" / String / "goal")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |sid: String, body: SetGoalBody, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    match engine
                        .set_goal(&sid, body.objective, body.token_budget)
                        .await
                    {
                        Ok(goal) => Ok::<_, Infallible>(warp::reply::json(&goal).into_response()),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                    }
                },
            );

        let get_goal = warp::path!("sessions" / String / "goal")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.get_goal(&sid).await {
                    Ok(goal) => Ok::<_, Infallible>(warp::reply::json(&goal).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let clear_goal = warp::path!("sessions" / String / "goal")
            .and(warp::delete())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.clear_goal(&sid).await {
                    Ok(_) => Ok::<_, Infallible>(ok_json("cleared")),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let snapshot = warp::path!("sessions" / String / "snapshot")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.snapshot_session(&sid).await {
                    Ok(snap) => Ok::<_, Infallible>(warp::reply::json(&snap).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let set_session_model = warp::path!("sessions" / String / "model")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |sid: String, body: SetModelBody, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    match engine.set_model(&sid, &body.provider, &body.model).await {
                        Ok(_) => Ok::<_, Infallible>(ok_json("model updated")),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                    }
                },
            );

        let set_session_skills = warp::path!("sessions" / String / "skills")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |sid: String, body: SetSkillsBody, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    match engine.set_session_skills(&sid, body.skills).await {
                        Ok(_) => Ok::<_, Infallible>(ok_json("skills updated")),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                    }
                },
            );

        let mcp_servers = warp::path!("sessions" / String / "mcp")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.list_mcp_servers(&sid) {
                    Ok(servers) => Ok::<_, Infallible>(warp::reply::json(&servers).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let bg_commands = warp::path!("sessions" / String / "background")
            .and(warp::path::end())
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.list_background_commands(&sid).await {
                    Ok(cmds) => Ok::<_, Infallible>(warp::reply::json(&cmds).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let delete_saved = warp::path!("sessions" / String / "delete")
            .and(warp::post())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.delete_saved_session(&sid) {
                    Ok(deleted) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "deleted": deleted
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let select_model = warp::path!("model" / "select")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|body: SelectModelBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let save = crate::state::parse_save_target(&body.save_target);
                let req = navi_sdk::NaviModelSelectionRequest {
                    provider_id: body.provider_id,
                    model: body.model,
                    save_target: save,
                };
                match engine.select_model(req) {
                    Ok(result) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "providerId": result.provider_id,
                            "model": result.model,
                            "contextWindowTokens": result.context_window_tokens,
                            "providerConfigured": result.provider_configured,
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let sync_models = warp::path!("providers" / "sync")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|body: SyncModelsBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let save = crate::state::parse_save_target(&body.save_target);
                match engine.sync_provider_models(&body.provider_id, save).await {
                    Ok(report) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "updated": report.updated.iter().map(|u| serde_json::json!({
                                "providerId": u.provider_id,
                                "modelCount": u.model_count,
                            })).collect::<Vec<_>>(),
                            "failed": report.failed.iter().map(|f| serde_json::json!({
                                "providerId": f.provider_id,
                                "message": f.message,
                            })).collect::<Vec<_>>(),
                            "skipped": report.skipped.iter().map(|sk| serde_json::json!({
                                "providerId": sk.provider_id,
                                "reason": sk.reason,
                            })).collect::<Vec<_>>(),
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        let sync_all = warp::path!("providers" / "sync-all")
            .and(warp::post())
            .and(warp::query::<std::collections::HashMap<String, String>>())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |params: std::collections::HashMap<String, String>, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    let save = crate::state::parse_save_target(
                        params.get("save").map(|s| s.as_str()).unwrap_or("auto"),
                    );
                    match engine.sync_models(save).await {
                        Ok(report) => Ok::<_, Infallible>(
                            warp::reply::json(&serde_json::json!({
                                "updated": report.updated.len(),
                                "failed": report.failed.len(),
                                "skipped": report.skipped.len(),
                            }))
                            .into_response(),
                        ),
                        Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                    }
                },
            );

        let usage = warp::path("usage")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.usage_report().await {
                    Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        let ws_events = warp::path!("sessions" / String / "events")
            .and(warp::ws())
            .and(warp::query::<std::collections::HashMap<String, String>>())
            .and(sf.clone())
            .map(
                |sid: String,
                 ws: warp::ws::Ws,
                 params: std::collections::HashMap<String, String>,
                 state: SharedState| {
                    ws.on_upgrade(move |socket| handle_ws(socket, sid, state, params))
                },
            );

        // Domain modules (memory, voice, plugins, auth, session_ops, skills_mcp, registry)
        let domain = routes::all_routes(state.clone(), secret);

        let routes = health
            .or(models)
            .or(get_config)
            .or(skills)
            .or(list_sessions)
            .or(start_session)
            .or(list_saved)
            .or(load_saved)
            .or(session_info)
            .or(send_turn)
            .or(close_session)
            .or(cancel_turn)
            .or(approve)
            .or(deny)
            .or(question)
            .or(set_goal)
            .or(get_goal)
            .or(clear_goal)
            .or(snapshot)
            .or(set_session_model)
            .or(set_session_skills)
            .or(mcp_servers)
            .or(bg_commands)
            .or(delete_saved)
            .or(select_model)
            .or(sync_models)
            .or(sync_all)
            .or(usage)
            .or(ws_events)
            .or(domain)
            .recover(handle_rejection);

        let addr: std::net::IpAddr = self.config.bind.parse().unwrap_or([0, 0, 0, 0].into());
        let socket_addr = std::net::SocketAddr::new(addr, self.config.port);

        info!("NAVI server listening on {}", socket_addr);
        warp::serve(routes).run(socket_addr).await;

        Ok(())
    }
}

// ── WebSocket handler ────────────────────────────────────────────────────

async fn handle_ws(
    ws: WebSocket,
    session_id: String,
    state: SharedState,
    params: std::collections::HashMap<String, String>,
) {
    let provided = params.get("secret").map(|s| s.as_str()).unwrap_or("");
    if provided != state.secret {
        warn!("WS auth failed for session {}", session_id);
        return;
    }

    let (mut ws_tx, mut ws_rx) = ws.split();

    let tx = match state.get_sender(&session_id).await {
        Some(tx) => tx,
        None => state.register_sender(&session_id).await,
    };

    let mut rx = tx.subscribe();

    let forward = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event)
                && ws_tx.send(Message::text(json)).await.is_err()
            {
                break;
            }
        }
    });

    while let Some(result) = ws_rx.next().await {
        if result.map(|m| m.is_close()).unwrap_or(true) {
            break;
        }
    }

    forward.abort();
    info!("WS disconnected for session {}", session_id);
}
