//! Credentials and OAuth HTTP routes.

use crate::state::{SharedState, err_resp, ok_json, with_auth, with_state};
use serde::Deserialize;
use std::convert::Infallible;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ApiKeyBody {
    #[serde(alias = "apiKey")]
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct AddAccountBody {
    #[serde(alias = "apiKey")]
    api_key: String,
    #[serde(default)]
    label: Option<String>,
}

// ── Routes ───────────────────────────────────────────────────────────────

/// Full credentials / OAuth route tree.
///
/// | Method | Path | Engine |
/// |--------|------|--------|
/// | GET | /credentials | list_provider_accounts |
/// | GET | /credentials/:providerId | credential_status + list_credential_accounts |
/// | PUT | /credentials/:providerId | set_provider_api_key |
/// | DELETE | /credentials/:providerId | delete_provider_api_key |
/// | GET | /credentials/:providerId/accounts | list_credential_accounts |
/// | POST | /credentials/:providerId/accounts | add_provider_account |
/// | POST | /credentials/:providerId/accounts/:accountId/select | select_provider_account |
/// | DELETE | /credentials/:providerId/accounts/:accountId | delete_provider_account |
/// | GET | /oauth/:providerId/supports | provider_supports_device_oauth |
/// | POST | /oauth/:providerId | start_device_oauth_simple |
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // More specific paths first so they are not swallowed by `/:providerId`.

    // GET /credentials
    let list_credentials = warp::path("credentials")
        .and(warp::path::end())
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.list_provider_accounts() {
                Ok(accounts) => Ok::<_, Infallible>(warp::reply::json(&accounts).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /credentials/:providerId/accounts
    let list_accounts = warp::path!("credentials" / String / "accounts")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|provider_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.list_credential_accounts(&provider_id) {
                Ok(accounts) => Ok::<_, Infallible>(warp::reply::json(&accounts).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // POST /credentials/:providerId/accounts
    let add_account = warp::path!("credentials" / String / "accounts")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |provider_id: String, body: AddAccountBody, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.add_provider_account(
                    &provider_id,
                    &body.api_key,
                    body.label.as_deref(),
                ) {
                    Ok(account_id) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({ "accountId": account_id }))
                            .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // POST /credentials/:providerId/accounts/:accountId/select
    let select_account = warp::path!("credentials" / String / "accounts" / String / "select")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |provider_id: String, account_id: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.select_provider_account(&provider_id, &account_id) {
                    Ok(()) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({
                            "selected": true,
                            "accountId": account_id,
                        }))
                        .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // DELETE /credentials/:providerId/accounts/:accountId
    let delete_account = warp::path!("credentials" / String / "accounts" / String)
        .and(warp::delete())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |provider_id: String, account_id: String, s: SharedState| async move {
                let engine = s.engine.read().await;
                match engine.delete_provider_account(&provider_id, &account_id) {
                    Ok(deleted) => Ok::<_, Infallible>(
                        warp::reply::json(&serde_json::json!({ "deleted": deleted }))
                            .into_response(),
                    ),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // GET /credentials/:providerId — status + accounts
    let get_credential = warp::path!("credentials" / String)
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|provider_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            let status = match engine.credential_status(&provider_id) {
                Ok(status) => status,
                Err(e) => {
                    return Ok::<_, Infallible>(err_resp(
                        e.to_string(),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    ));
                }
            };
            match engine.list_credential_accounts(&provider_id) {
                Ok(accounts) => Ok(warp::reply::json(&serde_json::json!({
                    "status": status,
                    "accounts": accounts,
                }))
                .into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // PUT /credentials/:providerId
    let set_credential = warp::path!("credentials" / String)
        .and(warp::put())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(
            |provider_id: String, body: ApiKeyBody, s: SharedState| async move {
                if body.api_key.trim().is_empty() {
                    return Ok::<_, Infallible>(err_resp(
                        "apiKey is required".to_string(),
                        StatusCode::BAD_REQUEST,
                    ));
                }
                let engine = s.engine.read().await;
                match engine.set_provider_api_key(&provider_id, &body.api_key) {
                    Ok(()) => Ok(ok_json("updated")),
                    Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
                }
            },
        );

    // DELETE /credentials/:providerId
    let delete_credential = warp::path!("credentials" / String)
        .and(warp::delete())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|provider_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.delete_provider_api_key(&provider_id) {
                Ok(deleted) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "deleted": deleted })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // GET /oauth/:providerId/supports
    let oauth_supports = warp::path!("oauth" / String / "supports")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|provider_id: String, s: SharedState| async move {
            let engine = s.engine.read().await;
            let supports = engine.provider_supports_device_oauth(&provider_id);
            Ok::<_, Infallible>(
                warp::reply::json(&serde_json::json!({ "supports": supports })).into_response(),
            )
        });

    // POST /oauth/:providerId — long-running device OAuth
    // Clone engine and drop the RwLock so other requests are not blocked for minutes.
    let oauth_start = warp::path!("oauth" / String)
        .and(warp::post())
        .and(sf)
        .and(af)
        .and_then(|provider_id: String, s: SharedState| async move {
            let engine = s.engine.read().await.clone();
            match engine.start_device_oauth_simple(&provider_id).await {
                Ok(secondary) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "secondary": secondary }))
                        .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    list_credentials
        .or(list_accounts)
        .or(add_account)
        .or(select_account)
        .or(delete_account)
        .or(get_credential)
        .or(set_credential)
        .or(delete_credential)
        .or(oauth_supports)
        .or(oauth_start)
        .boxed()
}
