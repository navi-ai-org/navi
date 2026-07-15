//! Skills CRUD, MCP config CRUD, session MCP tools, attachment/background routing.

use crate::state::{SharedState, err_resp, parse_save_target, with_auth, with_state};
use navi_core::{McpConfig, McpServerConfig, SkillWriteRequest};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::PathBuf;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies / query ───────────────────────────────────────────────
// Accept both snake_case and camelCase (JS/Dart clients).

#[derive(Debug, Deserialize, Default)]
struct SaveTargetQuery {
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Debug, Deserialize)]
struct McpEnabledBody {
    enabled: bool,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Debug, Deserialize)]
struct McpConfigBody {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    servers: Vec<McpServerConfig>,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Debug, Deserialize)]
struct AttachmentModelBody {
    modality: String,
    provider: String,
    model: String,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

#[derive(Debug, Deserialize)]
struct BackgroundModelBody {
    task: String,
    provider: String,
    model: String,
    #[serde(default, alias = "saveTarget")]
    save_target: String,
}

fn saved_to_json(path: Option<PathBuf>) -> serde_json::Value {
    serde_json::json!({
        "savedTo": path.map(|p| p.display().to_string()),
    })
}

fn extract_save_target(value: &serde_json::Value) -> String {
    value
        .get("saveTarget")
        .or_else(|| value.get("save_target"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Skills, MCP config, session MCP tools, and model-routing routes.
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // ── Skills ───────────────────────────────────────────────────────────

    // GET /skills
    let list_skills = warp::path("skills")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            // Surface a clear error when discovery is disabled (store writes still work,
            // so an empty list is easy to misread as "no skills").
            if !engine.loaded_config().config.skills.enabled {
                return Ok::<_, Infallible>(err_resp(
                    "skills discovery is disabled; set [skills].enabled = true in config"
                        .to_string(),
                    StatusCode::BAD_REQUEST,
                ));
            }
            match engine.list_skills() {
                Ok(skills) => Ok(warp::reply::json(&skills).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /skills/:id
    let get_skill = warp::path!("skills" / String)
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            if !engine.loaded_config().config.skills.enabled {
                return Ok::<_, Infallible>(err_resp(
                    "skills discovery is disabled; set [skills].enabled = true in config"
                        .to_string(),
                    StatusCode::BAD_REQUEST,
                ));
            }
            match engine.get_skill(&id) {
                Ok(skill) => Ok(warp::reply::json(&skill).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::NOT_FOUND)),
            }
        });

    // POST /skills  body = SkillWriteRequest
    let save_skill = warp::path("skills")
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: SkillWriteRequest, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.save_skill(body) {
                Ok(result) => {
                    // Prefer a UI-friendly payload with full skill info when reload works.
                    match engine.get_skill(&result.skill.id) {
                        Ok(loaded) => Ok::<_, Infallible>(
                            warp::reply::json(&serde_json::json!({
                                "created": result.created,
                                "path": result.path.display().to_string(),
                                "skill": loaded,
                            }))
                            .into_response(),
                        ),
                        Err(_) => Ok(warp::reply::json(&result).into_response()),
                    }
                }
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // DELETE /skills/:id → {"deleted": bool}
    let delete_skill = warp::path!("skills" / String)
        .and(warp::delete())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.delete_skill(&id) {
                Ok(deleted) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "deleted": deleted })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // ── MCP config (global, not session) ─────────────────────────────────

    // GET /mcp
    let list_mcp = warp::path("mcp")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            let snap = engine.list_mcp_config();
            Ok::<_, Infallible>(warp::reply::json(&snap).into_response())
        });

    // PUT /mcp  body = McpConfig + saveTarget?
    let set_mcp = warp::path("mcp")
        .and(warp::path::end())
        .and(warp::put())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: McpConfigBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            let mcp = McpConfig {
                enabled: body.enabled,
                servers: body.servers,
            };
            let target = parse_save_target(&body.save_target);
            match engine.set_mcp_config(mcp, target) {
                Ok(path) => {
                    Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                }
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /mcp/enabled  {enabled, saveTarget?}
    let set_mcp_enabled = warp::path!("mcp" / "enabled")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: McpEnabledBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            let target = parse_save_target(&body.save_target);
            match engine.set_mcp_enabled(body.enabled, target) {
                Ok(path) => {
                    Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                }
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /mcp/servers  {server, saveTarget?} | flat McpServerConfig + saveTarget?
    let upsert_mcp_server = warp::path!("mcp" / "servers")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: serde_json::Value, s: SharedState| async move {
            let save_raw = extract_save_target(&body);
            let target = parse_save_target(&save_raw);

            let server_val = match body.get("server") {
                Some(server) => server.clone(),
                None => {
                    let mut flat = body;
                    if let Some(obj) = flat.as_object_mut() {
                        obj.remove("saveTarget");
                        obj.remove("save_target");
                    }
                    flat
                }
            };

            let server: McpServerConfig = match serde_json::from_value(server_val) {
                Ok(server) => server,
                Err(e) => {
                    return Ok::<_, Infallible>(err_resp(
                        format!("invalid MCP server body: {e}"),
                        StatusCode::BAD_REQUEST,
                    ));
                }
            };

            let engine = s.engine.read().await;
            match engine.upsert_mcp_server(server, target) {
                Ok(path) => Ok(warp::reply::json(&saved_to_json(path)).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // DELETE /mcp/servers/:id ?saveTarget=
    let remove_mcp_server = warp::path!("mcp" / "servers" / String)
        .and(warp::delete())
        .and(warp::query::<SaveTargetQuery>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |id: String, query: SaveTargetQuery, s: SharedState| async move {
                let engine = s.engine.read().await;
                let target = parse_save_target(&query.save_target);
                match engine.remove_mcp_server(&id, target) {
                    Ok((removed, path)) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "removed": removed,
                            "savedTo": path.map(|p| p.display().to_string()),
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // ── Session MCP tools ────────────────────────────────────────────────

    // GET /sessions/:id/mcp/tools
    let list_mcp_tools = warp::path!("sessions" / String / "mcp" / "tools")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|sid: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.list_mcp_tools(&sid) {
                Ok(tools) => Ok::<_, Infallible>(warp::reply::json(&tools).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // ── Model routing (attachment + background) ──────────────────────────

    // GET /routing
    let get_routing = warp::path("routing")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            let loaded = engine.loaded_config();
            Ok::<_, Infallible>(
                warp::reply::json(&serde_json::json!({
                    "attachmentModels": loaded.config.attachment_models,
                    "attachment_models": loaded.config.attachment_models,
                    "backgroundModels": loaded.config.background_models,
                    "background_models": loaded.config.background_models,
                }))
                .into_response(),
            )
        });

    // POST /routing/attachment  {modality, provider, model, saveTarget?}
    let set_attachment = warp::path!("routing" / "attachment")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: AttachmentModelBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            let target = parse_save_target(&body.save_target);
            match engine.set_attachment_model(&body.modality, &body.provider, &body.model, target) {
                Ok(path) => {
                    Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                }
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // DELETE /routing/attachment/:modality ?saveTarget=
    let clear_attachment = warp::path!("routing" / "attachment" / String)
        .and(warp::delete())
        .and(warp::query::<SaveTargetQuery>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |modality: String, query: SaveTargetQuery, s: SharedState| async move {
                let engine = s.engine.read().await;
                let target = parse_save_target(&query.save_target);
                match engine.clear_attachment_model(&modality, target) {
                    Ok(path) => {
                        Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /routing/background  {task, provider, model, saveTarget?}
    let set_background = warp::path!("routing" / "background")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: BackgroundModelBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            let target = parse_save_target(&body.save_target);
            match engine.set_background_model(&body.task, &body.provider, &body.model, target) {
                Ok(path) => {
                    Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                }
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // DELETE /routing/background/:task ?saveTarget=
    let clear_background = warp::path!("routing" / "background" / String)
        .and(warp::delete())
        .and(warp::query::<SaveTargetQuery>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |task: String, query: SaveTargetQuery, s: SharedState| async move {
                let engine = s.engine.read().await;
                let target = parse_save_target(&query.save_target);
                match engine.clear_background_model(&task, target) {
                    Ok(path) => {
                        Ok::<_, Infallible>(warp::reply::json(&saved_to_json(path)).into_response())
                    }
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    list_skills
        .or(get_skill)
        .or(save_skill)
        .or(delete_skill)
        .or(list_mcp)
        .or(set_mcp)
        .or(set_mcp_enabled)
        .or(upsert_mcp_server)
        .or(remove_mcp_server)
        .or(list_mcp_tools)
        .or(get_routing)
        .or(set_attachment)
        .or(clear_attachment)
        .or(set_background)
        .or(clear_background)
        .boxed()
}
