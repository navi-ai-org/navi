//! Memory CRUD + maintenance HTTP routes.

use crate::state::{SharedState, err_resp, ok_json, optional_json_body, with_auth, with_state};
use navi_core::memory::{MemoryStatus, MemoryType};
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Parse helpers ────────────────────────────────────────────────────────

pub(crate) fn parse_memory_type(s: &str) -> Result<MemoryType, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "user" => Ok(MemoryType::User),
        "feedback" => Ok(MemoryType::Feedback),
        "project" => Ok(MemoryType::Project),
        "reference" => Ok(MemoryType::Reference),
        other => Err(format!("invalid memory type: {other}")),
    }
}

pub(crate) fn parse_memory_status(s: &str) -> Result<MemoryStatus, String> {
    let normalized = s.trim().to_ascii_lowercase().replace('-', "_");
    MemoryStatus::from_str(&normalized)
        .or(match normalized.as_str() {
            "active" => Some(MemoryStatus::Active),
            "needs_review" => Some(MemoryStatus::NeedsReview),
            "obsolete" => Some(MemoryStatus::Obsolete),
            _ => None,
        })
        .ok_or_else(|| {
            format!("invalid memory status: {s} (expected active|needs_review|obsolete)")
        })
}

// ── Request bodies / query DTOs ──────────────────────────────────────────
// Accept both snake_case (Rust default) and camelCase (JS/Dart clients).

#[derive(Debug, Default, Deserialize)]
struct InitBody {
    #[serde(default)]
    embeddings: Option<bool>,
    #[serde(default)]
    force: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WriteBody {
    id: String,
    /// Memory type: user | feedback | project | reference
    #[serde(alias = "memoryType", alias = "memory_type")]
    #[serde(rename = "type")]
    memory_type: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Deserialize)]
struct UpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct DreamBody {
    #[serde(default)]
    apply: Option<bool>,
    #[serde(default)]
    sessions: Option<u32>,
    #[serde(default)]
    instructions: Option<String>,
}

// ── Routes ───────────────────────────────────────────────────────────────

/// Full memory route tree.
///
/// | Method | Path | Engine |
/// |--------|------|--------|
/// | GET | /memory/status | memory_status |
/// | GET | /memory/doctor | memory_doctor |
/// | POST | /memory/init | memory_init |
/// | GET | /memory | memory_list |
/// | POST | /memory | memory_write |
/// | GET | /memory/count | memory_count |
/// | GET | /memory/index | memory_index |
/// | GET | /memory/search | memory_search |
/// | GET | /memory/:id | memory_read |
/// | PATCH | /memory/:id | memory_update |
/// | DELETE | /memory/:id | memory_delete |
/// | GET | /memory/history | memory_history_search |
/// | POST | /memory/dream | memory_dream |
/// | POST | /memory/distill | memory_distill |
/// | POST | /memory/checkpoint | memory_checkpoint |
/// | GET | /memory/rebuild-preview | memory_rebuild_preview |
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // More specific paths first so they are not swallowed by `/:id`.

    // GET /memory/status
    let status = warp::path!("memory" / "status")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_status() {
                Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /memory/doctor
    let doctor = warp::path!("memory" / "doctor")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_doctor() {
                Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /memory/init  (body optional — empty POST is valid)
    let init = warp::path!("memory" / "init")
        .and(warp::post())
        .and(optional_json_body::<InitBody>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: InitBody, s: SharedState| async move {
            // Long-running: clone engine so other handlers are not blocked on the lock.
            let engine = s.engine.read().await.clone();
            match engine
                .memory_init(
                    body.embeddings.unwrap_or(false),
                    body.force.unwrap_or(false),
                )
                .await
            {
                Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /memory/count
    let count = warp::path!("memory" / "count")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_count() {
                Ok(n) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "count": n })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /memory/index
    let index = warp::path!("memory" / "index")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            let text = engine.memory_index();
            Ok::<_, Infallible>(
                warp::reply::json(&serde_json::json!({ "index": text })).into_response(),
            )
        });

    // GET /memory/search?q=&limit=
    let search = warp::path!("memory" / "search")
        .and(warp::get())
        .and(warp::query::<HashMap<String, String>>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |params: HashMap<String, String>, s: SharedState| async move {
                let q = match params.get("q").filter(|v| !v.is_empty()) {
                    Some(q) => q.clone(),
                    None => {
                        return Ok::<_, Infallible>(err_resp(
                            "query parameter 'q' is required".into(),
                            StatusCode::BAD_REQUEST,
                        ));
                    }
                };
                let limit = params
                    .get("limit")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(20);
                let engine = s.engine.read().await;
                match engine.memory_search(&q, limit) {
                    Ok(results) => Ok(warp::reply::json(&results).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            },
        );

    // GET /memory/history?q=&limit=&sessionId=
    let history = warp::path!("memory" / "history")
        .and(warp::get())
        .and(warp::query::<HashMap<String, String>>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |params: HashMap<String, String>, s: SharedState| async move {
                let q = match params.get("q").filter(|v| !v.is_empty()) {
                    Some(q) => q.clone(),
                    None => {
                        return Ok::<_, Infallible>(err_resp(
                            "query parameter 'q' is required".into(),
                            StatusCode::BAD_REQUEST,
                        ));
                    }
                };
                let limit = params.get("limit").and_then(|v| v.parse::<i64>().ok());
                let session_id = params
                    .get("sessionId")
                    .or_else(|| params.get("session_id"))
                    .cloned();
                let engine = s.engine.read().await;
                match engine.memory_history_search(&q, session_id.as_deref(), limit) {
                    Ok(events) => Ok(warp::reply::json(&events).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            },
        );

    // GET /memory/rebuild-preview
    let rebuild_preview = warp::path!("memory" / "rebuild-preview")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_rebuild_preview() {
                Ok(preview) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "preview": preview })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /memory/dream  (body optional)
    let dream = warp::path!("memory" / "dream")
        .and(warp::post())
        .and(optional_json_body::<DreamBody>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: DreamBody, s: SharedState| async move {
            let engine = s.engine.read().await.clone();
            match engine
                .memory_dream(
                    body.apply.unwrap_or(false),
                    body.sessions.unwrap_or(10) as usize,
                    body.instructions,
                )
                .await
            {
                Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /memory/distill
    let distill = warp::path!("memory" / "distill")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await.clone();
            match engine.memory_distill().await {
                Ok(()) => Ok::<_, Infallible>(ok_json("ok")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /memory/checkpoint
    let checkpoint = warp::path!("memory" / "checkpoint")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await.clone();
            match engine.memory_checkpoint().await {
                Ok(session_id) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "sessionId": session_id }))
                        .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /memory?status=
    let list = warp::path("memory")
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::query::<HashMap<String, String>>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |params: HashMap<String, String>, s: SharedState| async move {
                let status = match params.get("status") {
                    Some(raw) if !raw.is_empty() => match parse_memory_status(raw) {
                        Ok(st) => Some(st),
                        Err(msg) => {
                            return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                        }
                    },
                    _ => None,
                };
                let engine = s.engine.read().await;
                match engine.memory_list(status) {
                    Ok(items) => Ok(warp::reply::json(&items).into_response()),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            },
        );

    // POST /memory
    let write = warp::path("memory")
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: WriteBody, s: SharedState| async move {
            if body.id.trim().is_empty() {
                return Ok::<_, Infallible>(err_resp(
                    "id is required".into(),
                    StatusCode::BAD_REQUEST,
                ));
            }
            let mt = match parse_memory_type(&body.memory_type) {
                Ok(t) => t,
                Err(msg) => {
                    return Ok(err_resp(msg, StatusCode::BAD_REQUEST));
                }
            };
            let engine = s.engine.read().await;
            match engine.memory_write(
                body.id.trim(),
                mt,
                &body.name,
                &body.description,
                &body.body,
            ) {
                Ok(()) => Ok(ok_json("created")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /memory/:id
    let read = warp::path!("memory" / String)
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_read(&id) {
                Ok(Some(entry)) => Ok::<_, Infallible>(warp::reply::json(&entry).into_response()),
                Ok(None) => Ok(err_resp(
                    format!("memory not found: {id}"),
                    StatusCode::NOT_FOUND,
                )),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // PATCH /memory/:id
    let update = warp::path!("memory" / String)
        .and(warp::patch())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|id: String, body: UpdateBody, s: SharedState| async move {
            let status = match body.status.as_deref() {
                Some(raw) if !raw.is_empty() => match parse_memory_status(raw) {
                    Ok(st) => Some(st),
                    Err(msg) => {
                        return Ok::<_, Infallible>(err_resp(msg, StatusCode::BAD_REQUEST));
                    }
                },
                _ => None,
            };
            let engine = s.engine.read().await;
            match engine.memory_update(
                &id,
                body.name.as_deref(),
                body.description.as_deref(),
                body.body.as_deref(),
                status,
            ) {
                Ok(()) => Ok(ok_json("updated")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // DELETE /memory/:id
    let delete = warp::path!("memory" / String)
        .and(warp::delete())
        .and(sf)
        .and(af)
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.memory_delete(&id) {
                Ok(()) => Ok::<_, Infallible>(ok_json("deleted")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    status
        .or(doctor)
        .or(init)
        .or(count)
        .or(index)
        .or(search)
        .or(history)
        .or(rebuild_preview)
        .or(dream)
        .or(distill)
        .or(checkpoint)
        .or(list)
        .or(write)
        .or(read)
        .or(update)
        .or(delete)
        .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_parses_trim_and_case() {
        assert_eq!(parse_memory_type(" User ").unwrap(), MemoryType::User);
        assert_eq!(parse_memory_type("FEEDBACK").unwrap(), MemoryType::Feedback);
        assert!(parse_memory_type("nope").is_err());
    }

    #[test]
    fn memory_status_parses_hyphen_and_underscore() {
        assert_eq!(
            parse_memory_status("needs-review").unwrap(),
            MemoryStatus::NeedsReview
        );
        assert_eq!(
            parse_memory_status(" needs_review ").unwrap(),
            MemoryStatus::NeedsReview
        );
        assert_eq!(parse_memory_status("ACTIVE").unwrap(), MemoryStatus::Active);
        assert!(parse_memory_status("stale").is_err());
    }
}
