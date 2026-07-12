//! Registry list/sync HTTP routes.
//!
//! Exposes the engine provider-registry catalog (`list_registry`) and remote/local
//! cache sync (`sync_registry`). Model listing (`GET /models`) stays in `server.rs`.

use crate::state::{SharedState, err_resp, ok_json, with_auth, with_state};
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct SyncRegistryBody {
    /// When true, force a remote/local registry refresh even if the cache looks fresh.
    #[serde(default)]
    force: Option<bool>,
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn parse_force_query(params: &HashMap<String, String>) -> Option<bool> {
    params.get("force").map(|raw| {
        let v = raw.trim();
        v.eq_ignore_ascii_case("true")
            || v == "1"
            || v.eq_ignore_ascii_case("yes")
            || v.eq_ignore_ascii_case("on")
    })
}

/// Optional JSON body. Missing / empty / non-JSON bodies fall back to defaults so
/// clients can pass `force` only via query string.
fn optional_sync_body() -> impl Filter<Extract = (SyncRegistryBody,), Error = Infallible> + Clone {
    warp::body::content_length_limit(16 * 1024)
        .and(warp::body::json())
        .or(warp::any().map(SyncRegistryBody::default))
        .unify()
}

// ── Routes ───────────────────────────────────────────────────────────────

/// Registry HTTP routes.
///
/// | Method | Path | Engine |
/// |--------|------|--------|
/// | GET | /registry | `list_registry()` |
/// | POST | /registry/sync | `sync_registry(force?)` — body or query `{force?: bool}` → `{"updated": bool}` |
///
/// `GET /models` is intentionally not registered here (lives in `server.rs`).
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // GET /registry
    let list = warp::path("registry")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.list_registry() {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /registry/sync — `force` from JSON body and/or `?force=` query (body wins).
    let sync = warp::path!("registry" / "sync")
        .and(warp::post())
        .and(warp::query::<HashMap<String, String>>())
        .and(optional_sync_body())
        .and(sf)
        .and(af)
        .and_then(
            |query: HashMap<String, String>, body: SyncRegistryBody, s: SharedState| async move {
                let force = body
                    .force
                    .or_else(|| parse_force_query(&query))
                    .unwrap_or(false);

                let engine = s.engine.read().await;
                match engine.sync_registry(force).await {
                    Ok(updated) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "updated": updated,
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            },
        );

    // Keep helper symbols referenced for consistent route-module surface.
    let _ = ok_json;

    list.or(sync).boxed()
}
