//! Plugin lifecycle HTTP routes (list / search / install / update / remove / reload).

use crate::state::{SharedState, err_resp, ok_json, with_auth, with_state};
use serde::Deserialize;
use std::convert::Infallible;
use std::path::Path;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies ───────────────────────────────────────────────────────
// Accept both snake_case and camelCase (JS/Dart clients).

#[derive(Debug, Deserialize)]
struct InstallPathBody {
    path: String,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct InstallMarketplaceBody {
    #[serde(alias = "pluginId")]
    plugin_id: String,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct UpdatePathBody {
    path: String,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Deserialize)]
struct UpdateMarketplaceBody {
    #[serde(alias = "pluginId")]
    plugin_id: String,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
}

#[derive(Debug, Deserialize, Default)]
struct SearchQuery {
    #[serde(default)]
    q: Option<String>,
}

/// Plugin lifecycle routes under `/plugins`.
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // GET /plugins
    let list = warp::path("plugins")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_list() {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /plugins/search?q=
    let search = warp::path("plugins")
        .and(warp::path("search"))
        .and(warp::path::end())
        .and(warp::get())
        .and(warp::query::<SearchQuery>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|query: SearchQuery, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_search(query.q.as_deref()).await {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /plugins/install/path  { path, confirm }
    let install_path = warp::path("plugins")
        .and(warp::path("install"))
        .and(warp::path("path"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: InstallPathBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_install_path(Path::new(&body.path), body.confirm) {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /plugins/install/marketplace  { pluginId|plugin_id, confirm }
    let install_marketplace = warp::path("plugins")
        .and(warp::path("install"))
        .and(warp::path("marketplace"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: InstallMarketplaceBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine
                .plugin_install_marketplace(&body.plugin_id, body.confirm)
                .await
            {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /plugins/update/path  { path, force?, confirm }
    let update_path = warp::path("plugins")
        .and(warp::path("update"))
        .and(warp::path("path"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: UpdatePathBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_update_path(Path::new(&body.path), body.force, body.confirm) {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /plugins/update/marketplace  { pluginId|plugin_id, force?, confirm }
    let update_marketplace = warp::path("plugins")
        .and(warp::path("update"))
        .and(warp::path("marketplace"))
        .and(warp::path::end())
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: UpdateMarketplaceBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine
                .plugin_update_marketplace(&body.plugin_id, body.force, body.confirm)
                .await
            {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /plugins/reload-wasm
    let reload_wasm = warp::path("plugins")
        .and(warp::path("reload-wasm"))
        .and(warp::path::end())
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.reload_wasm_plugins().await {
                Ok(names) => Ok::<_, Infallible>(warp::reply::json(&names).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /plugins/:id  (after static segments so "search" etc. are not captured)
    let info = warp::path("plugins")
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_info(&id) {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
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

    // DELETE /plugins/:id
    let remove = warp::path("plugins")
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(warp::delete())
        .and(sf)
        .and(af)
        .and_then(|id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.plugin_remove(&id) {
                Ok(()) => Ok::<_, Infallible>(ok_json("removed")),
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

    list.or(search)
        .or(install_path)
        .or(install_marketplace)
        .or(update_path)
        .or(update_marketplace)
        .or(reload_wasm)
        .or(info)
        .or(remove)
        .boxed()
}
