//! Shared server state and HTTP reply helpers.

use navi_sdk::{NaviConfigSaveTarget, NaviEngine, RuntimeEvent};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use warp::Filter;
use warp::Reply;
use warp::http::StatusCode;

// ── Server config ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NaviServerConfig {
    pub bind: String,
    pub port: u16,
    pub shared_secret: String,
    pub project_dir: String,
}

// ── Shared state ─────────────────────────────────────────────────────────

pub type Engine = Arc<RwLock<NaviEngine>>;

pub struct AppState {
    pub engine: Engine,
    pub secret: String,
    /// Engine home / default project directory (from `--project`).
    pub home_project: PathBuf,
    /// Per-session broadcast senders for event streaming.
    pub event_senders: RwLock<HashMap<String, broadcast::Sender<RuntimeEvent>>>,
}

impl AppState {
    pub fn new(engine: NaviEngine, secret: String, home_project: PathBuf) -> Self {
        Self {
            engine: Arc::new(RwLock::new(engine)),
            secret,
            home_project,
            event_senders: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register_sender(&self, session_id: &str) -> broadcast::Sender<RuntimeEvent> {
        let (tx, _) = broadcast::channel(2048);
        self.event_senders
            .write()
            .await
            .insert(session_id.to_string(), tx.clone());
        tx
    }

    pub async fn get_sender(&self, session_id: &str) -> Option<broadcast::Sender<RuntimeEvent>> {
        self.event_senders.read().await.get(session_id).cloned()
    }

    pub async fn remove_sender(&self, session_id: &str) {
        self.event_senders.write().await.remove(session_id);
    }
}

pub type SharedState = Arc<AppState>;

// ── DTOs ─────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ErrorResp {
    pub error: String,
}

// ── Auth / state filters ─────────────────────────────────────────────────

pub fn with_auth(secret: &'static str) -> impl Filter<Extract = (), Error = warp::Rejection> + Clone {
    warp::header::exact("x-navi-secret", secret)
}

pub fn with_state(
    state: SharedState,
) -> impl Filter<Extract = (SharedState,), Error = Infallible> + Clone {
    warp::any().map(move || state.clone())
}

// ── Reply helpers ────────────────────────────────────────────────────────

pub fn ok_json(message: &str) -> warp::reply::Response {
    warp::reply::json(&serde_json::json!({"status": message})).into_response()
}

pub fn err_resp(message: String, code: StatusCode) -> warp::reply::Response {
    warp::reply::with_status(warp::reply::json(&ErrorResp { error: message }), code).into_response()
}

/// Parse a config save target from client input.
///
/// Accepts case-insensitive values with `-` or `_` separators. Unknown values
/// (including empty string) map to [`NaviConfigSaveTarget::Auto`].
pub fn parse_save_target(raw: &str) -> NaviConfigSaveTarget {
    match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "project" => NaviConfigSaveTarget::Project,
        "global" => NaviConfigSaveTarget::Global,
        "none" => NaviConfigSaveTarget::None,
        _ => NaviConfigSaveTarget::Auto,
    }
}

/// Optional JSON body: missing / empty / non-JSON falls back to `T::default()`.
///
/// Use for POST endpoints that accept `{}` or no body (e.g. `/memory/init`).
pub fn optional_json_body<T>() -> impl Filter<Extract = (T,), Error = Infallible> + Clone
where
    T: Default + Send + DeserializeOwned + 'static,
{
    warp::body::content_length_limit(1024 * 1024)
        .and(warp::body::json())
        .or(warp::any().map(T::default))
        .unify()
}

pub async fn handle_rejection(err: warp::Rejection) -> Result<impl warp::Reply, Infallible> {
    // Order matters: warp `or()` combines rejections (e.g. MethodNotAllowed from a
    // GET-only sibling + BodyDeserializeError from the POST handler). Prefer the
    // more specific client errors over MethodNotAllowed / generic 500.
    let (code, msg) = if err
        .find::<warp::filters::body::BodyDeserializeError>()
        .is_some()
    {
        (StatusCode::BAD_REQUEST, "invalid request body".to_string())
    } else if err.find::<warp::reject::MissingHeader>().is_some() {
        (
            StatusCode::UNAUTHORIZED,
            "unauthorized: missing X-Navi-Secret".to_string(),
        )
    } else if err.find::<warp::reject::InvalidHeader>().is_some() {
        // `header::exact` rejects wrong secret values as InvalidHeader (not MissingHeader).
        // Map to 401 so clients never see a 500 for bad credentials.
        (
            StatusCode::UNAUTHORIZED,
            "unauthorized: invalid X-Navi-Secret".to_string(),
        )
    } else if err.is_not_found() {
        (StatusCode::NOT_FOUND, "not found".to_string())
    } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            "method not allowed".to_string(),
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use navi_sdk::NaviConfigSaveTarget;

    #[test]
    fn parse_save_target_accepts_case_and_separators() {
        assert!(matches!(
            parse_save_target("project"),
            NaviConfigSaveTarget::Project
        ));
        assert!(matches!(
            parse_save_target("PROJECT"),
            NaviConfigSaveTarget::Project
        ));
        assert!(matches!(
            parse_save_target(" Global "),
            NaviConfigSaveTarget::Global
        ));
        assert!(matches!(
            parse_save_target("none"),
            NaviConfigSaveTarget::None
        ));
        assert!(matches!(
            parse_save_target(""),
            NaviConfigSaveTarget::Auto
        ));
        assert!(matches!(
            parse_save_target("auto"),
            NaviConfigSaveTarget::Auto
        ));
        assert!(matches!(
            parse_save_target("garbage"),
            NaviConfigSaveTarget::Auto
        ));
    }
}
