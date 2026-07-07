use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use navi_sdk::{ApprovalDecision, NaviEngine, NaviEngineBuilder, NaviTurnRequest, RuntimeEvent};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use warp::Filter;
use warp::Reply;
use warp::http::StatusCode;
use warp::ws::{Message, WebSocket};

// ── Server config ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NaviServerConfig {
    pub bind: String,
    pub port: u16,
    pub shared_secret: String,
    pub project_dir: String,
}

// ── Shared state ─────────────────────────────────────────────────────────

type Engine = Arc<RwLock<NaviEngine>>;

struct AppState {
    engine: Engine,
    secret: String,
    /// Per-session broadcast senders for event streaming.
    event_senders: RwLock<std::collections::HashMap<String, broadcast::Sender<RuntimeEvent>>>,
}

impl AppState {
    fn new(engine: NaviEngine, secret: String) -> Self {
        Self {
            engine: Arc::new(RwLock::new(engine)),
            secret,
            event_senders: RwLock::new(std::collections::HashMap::new()),
        }
    }

    async fn register_sender(&self, session_id: &str) -> broadcast::Sender<RuntimeEvent> {
        let (tx, _) = broadcast::channel(2048);
        self.event_senders
            .write()
            .await
            .insert(session_id.to_string(), tx.clone());
        tx
    }

    async fn get_sender(&self, session_id: &str) -> Option<broadcast::Sender<RuntimeEvent>> {
        self.event_senders.read().await.get(session_id).cloned()
    }

    async fn remove_sender(&self, session_id: &str) {
        self.event_senders.write().await.remove(session_id);
    }
}

type SharedState = Arc<AppState>;

// ── Request/Response DTOs ────────────────────────────────────────────────

#[derive(Deserialize)]
struct StartSessionBody {
    #[serde(default)]
    project_dir: Option<PathBuf>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    active_skills: Vec<String>,
}

#[derive(Deserialize)]
struct TurnBody {
    message: String,
    #[serde(default)]
    content_parts: Vec<navi_core::ContentPart>,
    #[serde(default)]
    thinking: Option<navi_core::ThinkingConfig>,
}

#[derive(Deserialize)]
struct ApprovalBody {
    request_id: String,
    approved: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct QuestionBody {
    question_id: String,
    answer: String,
    #[serde(default)]
    custom: Option<String>,
}

#[derive(Deserialize)]
struct SelectModelBody {
    provider_id: String,
    model: String,
    #[serde(default)]
    save_target: String, // "auto" | "project" | "global" | "none"
}

#[derive(Deserialize)]
struct SyncModelsBody {
    provider_id: String,
    #[serde(default)]
    save_target: String,
}

#[derive(Deserialize)]
struct SetGoalBody {
    objective: String,
    #[serde(default)]
    token_budget: Option<i64>,
}

#[derive(Deserialize)]
struct SetSkillsBody {
    skills: Vec<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct AddContextBody {
    source: String,
    payload: serde_json::Value,
}

#[derive(Deserialize)]
struct SetModelBody {
    provider: String,
    model: String,
}

#[derive(Serialize)]
struct ErrorResp {
    error: String,
}

// ── Auth filter ──────────────────────────────────────────────────────────

fn with_auth(secret: &'static str) -> impl Filter<Extract = (), Error = warp::Rejection> + Clone {
    warp::header::exact("x-navi-secret", secret)
}

fn with_state(
    state: SharedState,
) -> impl Filter<Extract = (SharedState,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
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
        let state = Arc::new(AppState::new(engine, self.config.shared_secret.clone()));

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
                        "project_dir": lc.project_config_path,
                        "data_dir": lc.data_dir,
                    }))
                    .into_response(),
                )
            });

        // ── Skills ───────────────────────────────────────────────────────
        let skills = warp::path("skills")
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

        // ── Credentials ──��───────────────────────────────────────────────
        let credentials = warp::path("credentials")
            .and(warp::get())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.list_provider_accounts() {
                    Ok(accounts) => {
                        Ok::<_, Infallible>(warp::reply::json(&accounts).into_response())
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        // ── Sessions list ────────────────────────────────────────────────
        let list_sessions = warp::path("sessions")
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
                        // Forward events from engine to our broadcast channel
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
        let list_saved = warp::path("sessions")
            .and(warp::path("saved"))
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

        // ── Load saved session ───────────────────────────────────────────
        let load_saved = warp::path!("sessions" / "load" / String)
            .and(warp::post())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|session_id: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.load_saved_session(&session_id) {
                    Ok(snapshot) => {
                        Ok::<_, Infallible>(warp::reply::json(&snapshot).into_response())
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::NOT_FOUND)),
                }
            });

        // ── Session info ─────────────────────────────────────────────────
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

        // ── Send turn ────────────────────────────────────────────────────
        let send_turn = warp::path!("sessions" / String / "turns")
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|sid: String, body: TurnBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let req = NaviTurnRequest {
                    session_id: sid,
                    message: body.message,
                    content_parts: body.content_parts,
                    context_packets: Vec::new(),
                    thinking: body.thinking,
                };
                match engine.send_turn(req).await {
                    Ok(resp) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "sessionId": resp.session_id,
                            "text": resp.text,
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            });

        // ── Close session ────────────────────────────────────────────────
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

        // ── Cancel turn ──────────────────────────────────────────────────
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

        // ── Approve ──────────────────────────────────────────────────────
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
                    ApprovalDecision::Approved { id: body.request_id.clone() }
                } else {
                    if let Some(ref msg) = body.message {
                        tracing::info!(session = %sid, request = %body.request_id, "approval denied: {msg}");
                    }
                    ApprovalDecision::Denied { id: body.request_id }
                };
                match engine.resolve_approval(&sid, decision).await {
                    Ok(consumed) => Ok::<_, Infallible>(warp::reply::json(&serde_json::json!({
                        "consumed": consumed
                    })).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            });

        // ── Deny ─────────────────────────────────────────────────────────
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

        // ── Question ─────────────────────────────────────────────────────
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
                        if let Some(custom) = body.custom {
                            if !custom.is_empty() {
                                answers.push(custom);
                            }
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

        // ── Set goal ─────────────────────────────────────────────────────
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

        // ── Get goal ─────────────────────────────────────────────────────
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

        // ── Clear goal ───────────────────────────────────────────────────
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

        // ── Snapshot session ─────────────────────────────────────────────
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

        // ── Set session model ────────────────────────────────────────────
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

        // ── Set session skills ─────────────────────────────────────────���─
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

        // ── MCP servers ──────────────────────────────────────────────────
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

        // ── Background commands ──────────────────────────────────────────
        let bg_commands = warp::path!("sessions" / String / "background")
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

        // ── Delete saved session ─────────────────────────────────────────
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

        // ── Global model select ──────────────────────────────────────────
        let select_model = warp::path("model")
            .and(warp::path("select"))
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|body: SelectModelBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let save = match body.save_target.as_str() {
                    "project" => navi_sdk::NaviConfigSaveTarget::Project,
                    "global" => navi_sdk::NaviConfigSaveTarget::Global,
                    "none" => navi_sdk::NaviConfigSaveTarget::None,
                    _ => navi_sdk::NaviConfigSaveTarget::Auto,
                };
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

        // ── Sync provider models ─────────────────────────────────────────
        let sync_models = warp::path("providers")
            .and(warp::path("sync"))
            .and(warp::post())
            .and(warp::body::json())
            .and(sf.clone())
            .and(af.clone())
            .and_then(|body: SyncModelsBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                let save = match body.save_target.as_str() {
                    "project" => navi_sdk::NaviConfigSaveTarget::Project,
                    "global" => navi_sdk::NaviConfigSaveTarget::Global,
                    "none" => navi_sdk::NaviConfigSaveTarget::None,
                    _ => navi_sdk::NaviConfigSaveTarget::Auto,
                };
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

        // ── Sync all models ──────────────────────────────────────────────
        let sync_all = warp::path("providers")
            .and(warp::path("sync-all"))
            .and(warp::post())
            .and(warp::query::<std::collections::HashMap<String, String>>())
            .and(sf.clone())
            .and(af.clone())
            .and_then(
                |params: std::collections::HashMap<String, String>, s: SharedState| async move {
                    let engine = s.engine.read().await;
                    let save = match params.get("save").map(|s| s.as_str()) {
                        Some("project") => navi_sdk::NaviConfigSaveTarget::Project,
                        Some("global") => navi_sdk::NaviConfigSaveTarget::Global,
                        Some("none") => navi_sdk::NaviConfigSaveTarget::None,
                        _ => navi_sdk::NaviConfigSaveTarget::Auto,
                    };
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

        // ── Usage report ─────────────────────────────────────────────────
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

        // ── WebSocket events ─────────────────────────────────────────────
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

        // ── Combine all routes ───────────────────────────────────────────
        let routes = health
            .or(models)
            .or(get_config)
            .or(skills)
            .or(credentials)
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

    // Get or create broadcast channel for this session
    let tx = match state.get_sender(&session_id).await {
        Some(tx) => tx,
        None => state.register_sender(&session_id).await,
    };

    let mut rx = tx.subscribe();

    let forward = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                if ws_tx.send(Message::text(json)).await.is_err() {
                    break;
                }
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

// ── Helpers ──────────────────────────────────────────────────────────────

fn ok_json(message: &str) -> warp::reply::Response {
    warp::reply::json(&serde_json::json!({"status": message})).into_response()
}

fn err_resp(message: String, code: StatusCode) -> warp::reply::Response {
    warp::reply::with_status(warp::reply::json(&ErrorResp { error: message }), code).into_response()
}

async fn handle_rejection(err: warp::Rejection) -> Result<impl warp::Reply, Infallible> {
    let (code, msg) = if err.is_not_found() {
        (StatusCode::NOT_FOUND, "not found".to_string())
    } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed".to_string(),
        )
    } else if err.find::<warp::reject::MissingHeader>().is_some() {
        (
            StatusCode::UNAUTHORIZED,
            "unauthorized: missing X-Navi-Secret".to_string(),
        )
    } else if err
        .find::<warp::filters::body::BodyDeserializeError>()
        .is_some()
    {
        (StatusCode::BAD_REQUEST, "invalid request body".to_string())
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error".to_string(),
        )
    };
    Ok(warp::reply::with_status(
        warp::reply::json(&ErrorResp { error: msg }),
        code,
    ))
}
