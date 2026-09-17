//! Voice / transcription HTTP routes.
//!
//! Engine-scoped (not per-session). Mirrors the remote transcription APIs of
//! `NaviEngine`: status, doctor, provider catalog, and WAV transcription.

use crate::state::{SharedState, err_resp, with_auth, with_state};
use serde::Deserialize;
use std::convert::Infallible;
use warp::Filter;
use warp::filters::BoxedFilter;
use warp::http::StatusCode;
use warp::reply::Reply;

// ── Request bodies ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TranscribeBody {
    path: String,
    #[serde(default)]
    language: Option<String>,
}

// ── Routes ───────────────────────────────────────────────────────────────

/// Remote transcription route tree.
///
/// | Method | Path | Engine |
/// |--------|------|--------|
/// | GET | /voice/status | voice_status |
/// | GET | /voice/doctor | voice_doctor |
/// | GET | /voice/providers | voice_transcription_providers |
/// | POST | /voice/transcribe | voice_transcribe_file_async |
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

    // POST /voice/transcribe  { path, language? }
    let transcribe = warp::path!("voice" / "transcribe")
        .and(warp::post())
        .and(warp::body::json())
        .and(sf)
        .and(af)
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

    status.or(doctor).or(providers).or(transcribe).boxed()
}
