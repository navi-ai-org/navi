//! Voice / dictation HTTP + WebSocket routes.
//!
//! Engine-scoped (not per-session). Mirrors `NaviEngine` voice APIs.

use crate::state::{SharedState, err_resp, ok_json, optional_json_body, with_auth, with_state};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::HashMap;
use std::convert::Infallible;
use tracing::{info, warn};
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;
use warp::ws::{Message, WebSocket};

// ── Request bodies / query ───────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct EngineQuery {
    #[serde(default)]
    engine: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct InitBody {
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    force: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TranscribeBody {
    path: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamStartBody {
    #[serde(default)]
    language: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PcmBody {
    /// 16 kHz mono f32 samples.
    samples: Vec<f32>,
}

// ── Routes ───────────────────────────────────────────────────────────────

/// Full voice route tree.
///
/// | Method | Path | Engine |
/// |--------|------|--------|
/// | GET | /voice/status | voice_status |
/// | GET | /voice/doctor | voice_doctor |
/// | GET | /voice/providers | voice_transcription_providers |
/// | GET | /voice/installed?engine= | voice_engine_installed |
/// | POST | /voice/init | voice_init |
/// | POST | /voice/transcribe | voice_transcribe_file_async |
/// | POST | /voice/stream/start | voice_start_stream |
/// | POST | /voice/stream/pcm | voice_push_pcm |
/// | POST | /voice/stream/end | voice_end_stream |
/// | POST | /voice/stream/cancel | voice_cancel_stream |
/// | GET | /voice/events?secret= | WebSocket subscribe_voice_events |
pub fn routes(state: SharedState, secret: &'static str) -> BoxedFilter<(impl Reply,)> {
    let sf = with_state(state);
    let af = with_auth(secret);

    // GET /voice/status
    let status = warp::path!("voice" / "status")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_status() {
                Ok(v) => Ok::<_, Infallible>(warp::reply::json(&v).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /voice/doctor
    let doctor = warp::path!("voice" / "doctor")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_doctor() {
                Ok(report) => Ok::<_, Infallible>(warp::reply::json(&report).into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
            }
        });

    // GET /voice/providers
    let providers = warp::path!("voice" / "providers")
        .and(warp::get())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            let list = engine.voice_transcription_providers();
            Ok::<_, Infallible>(warp::reply::json(&list).into_response())
        });

    // GET /voice/installed?engine=
    let installed = warp::path!("voice" / "installed")
        .and(warp::get())
        .and(warp::query::<EngineQuery>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|query: EngineQuery, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_engine_installed(query.engine.as_deref()) {
                Ok(installed) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "installed": installed }))
                        .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/init  { engine?, force? } — body optional
    let init = warp::path!("voice" / "init")
        .and(warp::post())
        .and(optional_json_body::<InitBody>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: InitBody, s: SharedState| async move {
            let engine = s.engine.read().await.clone();
            match engine
                .voice_init(body.engine.as_deref(), body.force.unwrap_or(false))
                .await
            {
                Ok(path) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({
                        "path": path.display().to_string(),
                    }))
                    .into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/transcribe  { path, language? }
    let transcribe = warp::path!("voice" / "transcribe")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: TranscribeBody, s: SharedState| async move {
            if body.path.trim().is_empty() {
                return Ok::<_, Infallible>(err_resp(
                    "path is required".to_string(),
                    StatusCode::BAD_REQUEST,
                ));
            }
            let engine = s.engine.read().await;
            match engine
                .voice_transcribe_file_async(&body.path, body.language.as_deref())
                .await
            {
                Ok(result) => Ok(warp::reply::json(&serde_json::json!({
                    "text": result.text,
                    "tokenIds": result.token_ids,
                }))
                .into_response()),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/stream/start  { language? } — body optional
    let stream_start = warp::path!("voice" / "stream" / "start")
        .and(warp::post())
        .and(optional_json_body::<StreamStartBody>())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: StreamStartBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_start_stream(body.language.as_deref()) {
                Ok(()) => Ok::<_, Infallible>(ok_json("started")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/stream/pcm  { samples: f32[] }
    let stream_pcm = warp::path!("voice" / "stream" / "pcm")
        .and(warp::post())
        .and(warp::body::content_length_limit(16 * 1024 * 1024))
        .and(warp::body::json())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|body: PcmBody, s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_push_pcm(&body.samples) {
                Ok(delta) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "delta": delta })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/stream/end
    let stream_end = warp::path!("voice" / "stream" / "end")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_end_stream() {
                Ok(text) => Ok::<_, Infallible>(
                    warp::reply::json(&serde_json::json!({ "text": text })).into_response(),
                ),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // POST /voice/stream/cancel
    let stream_cancel = warp::path!("voice" / "stream" / "cancel")
        .and(warp::post())
        .and(sf.clone())
        .and(af.clone())
        .and_then(|s: SharedState| async move {
            let engine = s.engine.read().await;
            match engine.voice_cancel_stream() {
                Ok(()) => Ok::<_, Infallible>(ok_json("cancelled")),
                Err(e) => Ok(err_resp(e.to_string(), StatusCode::BAD_REQUEST)),
            }
        });

    // GET /voice/events?secret=... — WebSocket stream of VoiceEvent
    // Auth via query secret (browsers cannot set custom WS headers).
    let events_ws = warp::path!("voice" / "events")
        .and(warp::ws())
        .and(warp::query::<HashMap<String, String>>())
        .and(sf)
        .map(
            |ws: warp::ws::Ws, params: HashMap<String, String>, state: SharedState| {
                ws.on_upgrade(move |socket| handle_voice_ws(socket, state, params))
            },
        );

    status
        .or(doctor)
        .or(providers)
        .or(installed)
        .or(init)
        .or(transcribe)
        .or(stream_start)
        .or(stream_pcm)
        .or(stream_end)
        .or(stream_cancel)
        .or(events_ws)
        .boxed()
}

// ── WebSocket handler ────────────────────────────────────────────────────

async fn handle_voice_ws(ws: WebSocket, state: SharedState, params: HashMap<String, String>) {
    let provided = params.get("secret").map(|s| s.as_str()).unwrap_or("");
    if provided != state.secret {
        warn!("WS auth failed for /voice/events");
        return;
    }

    let mut rx = {
        let engine = state.engine.read().await;
        engine.subscribe_voice_events()
    };

    let (mut ws_tx, mut ws_rx) = ws.split();

    let forward = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event)
                        && ws_tx.send(Message::text(json)).await.is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    while let Some(result) = ws_rx.next().await {
        if result.map(|m| m.is_close()).unwrap_or(true) {
            break;
        }
    }

    forward.abort();
    info!("WS disconnected for /voice/events");
}
