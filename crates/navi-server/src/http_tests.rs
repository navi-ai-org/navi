//! HTTP integration tests for navi-server routes (warp::test).
//!
//! These exercise real filter trees + engine wiring without binding a TCP port.

use crate::routes;
use crate::state::{AppState, SharedState, handle_rejection};
use navi_core::{LoadedConfig, NaviConfig, ProviderConfig, ProviderKind};
use navi_sdk::{NaviEngineBuilder, NaviSessionRequest};
use std::sync::Arc;
use warp::Filter;
use warp::http::StatusCode;
use warp::test::RequestBuilder;

const SECRET: &str = "test-secret-integration";

fn test_config() -> NaviConfig {
    let mut config = NaviConfig::default();
    config.providers.push(ProviderConfig {
        id: "test-provider".to_string(),
        label: "Test Provider".to_string(),
        description: String::new(),
        kind: ProviderKind::OpenAiResponses,
        api_key_env: "NAVI_SERVER_TEST_NONEXISTENT_ENV_999".to_string(),
        base_url: Some("https://example.test/v1".to_string()),
        models: vec![navi_core::config::types::ProviderModelConfig {
            name: "test-model".to_string(),
            task_size: Some(navi_core::config::types::ModelTaskSize::Small),
            context_window_tokens: Some(8192),
            max_output_tokens: None,
            recommended_temperature: None,
            supports_thinking: None,
            supports_images: None,
            supports_audio: None,
            supports_video: None,
            supports_documents: None,
            tool_prompt_manifest: None,
            pricing_input_per_1m: None,
            pricing_output_per_1m: None,
            reasoning_levels: Vec::new(),
            default_reasoning_effort: None,
        }],
        ..Default::default()
    });
    config.model.provider = "test-provider".to_string();
    config.model.name = "test-model".to_string();
    config.registry.update_enabled = false;
    // Skills discovery is off under `SkillsConfig::default()` (enabled: false).
    // Without this, write succeeds but list/get appear empty — a real footgun.
    config.skills.enabled = true;
    config
}

fn test_state() -> (SharedState, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let loaded_config = LoadedConfig {
        config: test_config(),
        global_config_path: Some(tempdir.path().join("config.toml")),
        project_config_path: None,
        data_dir: tempdir.path().to_path_buf(),
    };
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(loaded_config)
        .build()
        .expect("build engine");
    // Sessions need a resolvable credential; seed a dummy key for tests.
    engine
        .set_provider_api_key("test-provider", "sk-test-not-real")
        .expect("seed test credential");
    let state = Arc::new(AppState::new(
        engine,
        SECRET.to_string(),
        tempdir.path().to_path_buf(),
    ));
    (state, tempdir)
}

fn domain_filter(
    state: SharedState,
) -> impl Filter<Extract = (impl warp::Reply,), Error = Infallible> + Clone {
    // Leak secret once per process for filter lifetime (same pattern as server).
    let secret: &'static str = Box::leak(SECRET.to_string().into_boxed_str());
    routes::all_routes(state, secret).recover(handle_rejection)
}

use std::convert::Infallible;

fn authed(req: RequestBuilder) -> RequestBuilder {
    req.header("x-navi-secret", SECRET)
}

async fn body_json(res: &warp::http::Response<bytes::Bytes>) -> serde_json::Value {
    serde_json::from_slice(res.body()).unwrap_or_else(|_| {
        panic!(
            "expected JSON body, got: {}",
            String::from_utf8_lossy(res.body())
        )
    })
}

// ── Auth ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn missing_secret_returns_401() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);
    let res = warp::test::request()
        .method("GET")
        .path("/plugins")
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let v = body_json(&res).await;
    assert!(
        v["error"].as_str().unwrap_or("").contains("missing"),
        "error={v}"
    );
}

#[tokio::test]
async fn wrong_secret_returns_401_not_500() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);
    let res = warp::test::request()
        .method("GET")
        .path("/plugins")
        .header("x-navi-secret", "wrong-secret")
        .reply(&api)
        .await;
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "wrong secret must not become 500: body={}",
        String::from_utf8_lossy(res.body())
    );
    let v = body_json(&res).await;
    assert!(
        v["error"].as_str().unwrap_or("").contains("invalid"),
        "error={v}"
    );
}

// ── Memory ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn memory_crud_roundtrip() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    // Empty body init must work (optional JSON body).
    let res = authed(warp::test::request().method("POST").path("/memory/init"))
        .reply(&api)
        .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "init body={}",
        String::from_utf8_lossy(res.body())
    );

    // Write with camelCase type alias
    let res = authed(warp::test::request().method("POST").path("/memory").json(
        &serde_json::json!({
            "id": "mem-1",
            "memoryType": "project",
            "name": "Deadline",
            "description": "Ship gateway",
            "body": "Due Friday"
        }),
    ))
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "write: {}",
        String::from_utf8_lossy(res.body())
    );

    // List
    let res = authed(warp::test::request().method("GET").path("/memory"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let list = body_json(&res).await;
    assert!(
        list.as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "{list}"
    );

    // Read
    let res = authed(warp::test::request().method("GET").path("/memory/mem-1"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let entry = body_json(&res).await;
    assert_eq!(entry["id"], "mem-1");

    // Missing id → 404
    let res = authed(warp::test::request().method("GET").path("/memory/nope"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Invalid type → 400
    let res = authed(warp::test::request().method("POST").path("/memory").json(
        &serde_json::json!({
            "id": "x",
            "type": "not-a-type",
            "name": "x"
        }),
    ))
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Invalid status filter → 400
    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/memory?status=bogus"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Search requires q
    let res = authed(warp::test::request().method("GET").path("/memory/search"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Search with q
    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/memory/search?q=Deadline"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Patch status
    let res = authed(
        warp::test::request()
            .method("PATCH")
            .path("/memory/mem-1")
            .json(&serde_json::json!({ "status": "needs-review" })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Count + index
    let res = authed(warp::test::request().method("GET").path("/memory/count"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(warp::test::request().method("GET").path("/memory/index"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Specific path must not be swallowed by /memory/:id
    let res = authed(warp::test::request().method("GET").path("/memory/status"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let status = body_json(&res).await;
    assert!(
        status.get("active_memories").is_some() || status.get("enabled").is_some(),
        "{status}"
    );

    // Delete
    let res = authed(warp::test::request().method("DELETE").path("/memory/mem-1"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
}

// ── Permission mode ──────────────────────────────────────────────────────

#[tokio::test]
async fn permission_mode_get_set() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/permission-mode"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let before = body_json(&res).await;
    assert!(before["mode"].as_str().is_some());

    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/permission-mode")
            .json(&serde_json::json!({ "mode": "yolo" })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(&res).await["mode"], "yolo");

    let res = authed(warp::test::request().method("GET").path("/permission-mode"))
        .reply(&api)
        .await;
    assert_eq!(body_json(&res).await["mode"], "yolo");

    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/permission-mode")
            .json(&serde_json::json!({ "mode": "not-real" })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Session ops ──────────────────────────────────────────────────────────

// Session start / plan mode use SessionStore I/O via `block_in_place`, which
// requires the multi-threaded Tokio runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_plan_mode_and_goal_extensions() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state.clone());

    // Start a live session via engine directly (core route lives in server.rs).
    let session_id = {
        let engine = state.engine.read().await;
        let info = engine
            .start_session(NaviSessionRequest::default())
            .await
            .expect("start session");
        info.id
    };

    let res = authed(
        warp::test::request()
            .method("GET")
            .path(&format!("/sessions/{session_id}/mode")),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(&res).await["mode"], "default");

    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/plan/enter")),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(
        warp::test::request()
            .method("GET")
            .path(&format!("/sessions/{session_id}/mode")),
    )
    .reply(&api)
    .await;
    assert_eq!(body_json(&res).await["mode"], "plan");

    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/plan/exit")),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Context packet
    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/context"))
            .json(&serde_json::json!({
                "source": "File",
                "content": "fn main() {}",
                "title": "main.rs",
                "priority": 10
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "context: {}",
        String::from_utf8_lossy(res.body())
    );

    // Rewind
    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/rewind"))
            .json(&serde_json::json!({ "keepUserTurns": 0 })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(body_json(&res).await["remainingMessages"].is_number());

    // Unknown session
    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/sessions/does-not-exist/mode"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Invalid plan review decision
    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/plan/review"))
            .json(&serde_json::json!({
                "id": "x",
                "planId": "p",
                "decision": "maybe"
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Sudo cancel (no password / empty)
    let res = authed(
        warp::test::request()
            .method("POST")
            .path(&format!("/sessions/{session_id}/sudo"))
            .json(&serde_json::json!({ "id": "sudo-1", "password": "   " })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    // consumed may be false if nothing pending — still must not 500
    assert!(body_json(&res).await.get("consumed").is_some());
}

// ── Credentials ──────────────────────────────────────────────────────────

#[tokio::test]
async fn credentials_set_list_delete() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/credentials"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(
        warp::test::request()
            .method("PUT")
            .path("/credentials/test-provider")
            .json(&serde_json::json!({ "apiKey": "sk-test-123" })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Empty key rejected
    let res = authed(
        warp::test::request()
            .method("PUT")
            .path("/credentials/test-provider")
            .json(&serde_json::json!({ "api_key": "  " })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/credentials/test-provider"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(&res).await;
    assert!(v.get("status").is_some(), "{v}");
    assert!(v.get("accounts").is_some(), "{v}");

    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/credentials/test-provider/accounts")
            .json(&serde_json::json!({
                "apiKey": "sk-second",
                "label": "work"
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let account_id = body_json(&res).await["accountId"]
        .as_str()
        .unwrap()
        .to_string();

    let res = authed(warp::test::request().method("POST").path(&format!(
        "/credentials/test-provider/accounts/{account_id}/select"
    )))
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/oauth/test-provider/supports"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(body_json(&res).await["supports"].is_boolean());

    let res = authed(
        warp::test::request()
            .method("DELETE")
            .path("/credentials/test-provider"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

// ── Skills ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn skills_crud_and_path_specificity() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/skills"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(warp::test::request().method("POST").path("/skills").json(
        &serde_json::json!({
            "id": "gateway-tester",
            "name": "Gateway Tester",
            "instructions": "Always run the HTTP integration tests.",
            "description": "test skill",
            "tags": ["test"]
        }),
    ))
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "save skill: {}",
        String::from_utf8_lossy(res.body())
    );
    let saved = body_json(&res).await;
    // Accept either nested skill payload or raw SkillWriteResult.
    let skill_id = saved
        .pointer("/skill/id")
        .or_else(|| saved.pointer("/id"))
        .and_then(|v| v.as_str())
        .unwrap_or("gateway-tester")
        .to_string();

    // List should include the new skill (store discovery).
    let res = authed(warp::test::request().method("GET").path("/skills"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let listed = body_json(&res).await;
    let found = listed
        .as_array()
        .map(|arr| {
            arr.iter()
                .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(skill_id.as_str()))
        })
        .unwrap_or(false);
    assert!(
        found,
        "saved skill {skill_id} not in list: {listed}; save response={saved}"
    );

    let res = authed(
        warp::test::request()
            .method("GET")
            .path(&format!("/skills/{skill_id}")),
    )
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "get skill {skill_id}: {}",
        String::from_utf8_lossy(res.body())
    );

    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/skills/definitely-missing-skill-xyz"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let res = authed(
        warp::test::request()
            .method("DELETE")
            .path(&format!("/skills/{skill_id}")),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(&res).await["deleted"], true);
}

// ── MCP config ───────────────────────────────────────────────────────────

#[tokio::test]
async fn mcp_config_upsert_and_list() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/mcp"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Flat server body + saveTarget none (avoid writing unexpected files if possible)
    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/mcp/servers")
            .json(&serde_json::json!({
                "id": "demo-mcp",
                "command": "echo",
                "args": ["hi"],
                "enabled": true,
                "saveTarget": "none"
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "upsert: {}",
        String::from_utf8_lossy(res.body())
    );

    let res = authed(warp::test::request().method("GET").path("/mcp"))
        .reply(&api)
        .await;
    let snap = body_json(&res).await;
    let servers = snap["servers"].as_array().cloned().unwrap_or_default();
    assert!(
        servers.iter().any(|s| s["id"] == "demo-mcp"),
        "servers={servers:?}"
    );

    let res = authed(
        warp::test::request()
            .method("DELETE")
            .path("/mcp/servers/demo-mcp?saveTarget=none"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(&res).await["removed"], true);
}

// ── Routing ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn routing_get_and_invalid_modality() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/routing"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(&res).await;
    assert!(v.get("attachmentModels").is_some() || v.get("attachment_models").is_some());

    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/routing/attachment")
            .json(&serde_json::json!({
                "modality": "not-a-modality",
                "provider": "test-provider",
                "model": "test-model",
                "saveTarget": "none"
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Plugins path specificity ─────────────────────────────────────────────

#[tokio::test]
async fn plugins_list_and_static_paths_not_captured_as_id() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/plugins"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    // /plugins/search must not be treated as plugin id "search"
    let res = authed(warp::test::request().method("GET").path("/plugins/search"))
        .reply(&api)
        .await;
    // May 200 (catalog) or 500 (network) — must NOT be 404 "plugin 'search' not found"
    assert_ne!(
        res.status(),
        StatusCode::NOT_FOUND,
        "search captured as id: {}",
        String::from_utf8_lossy(res.body())
    );

    let res = authed(
        warp::test::request()
            .method("GET")
            .path("/plugins/definitely-not-installed"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // install without confirm → 400
    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/plugins/install/path")
            .json(&serde_json::json!({
                "path": "/tmp/nope",
                "confirm": false
            })),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Registry ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn registry_list_and_sync_query_force() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/registry"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(&res).await;
    assert!(
        v.get("providers").is_some() || v.get("provider_count").is_some(),
        "{v}"
    );

    // No body + query force=false — must not 400 on missing body
    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/registry/sync?force=false"),
    )
    .reply(&api)
    .await;
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "sync: {}",
        String::from_utf8_lossy(res.body())
    );
    assert!(body_json(&res).await.get("updated").is_some());
}

// ── Voice status ─────────────────────────────────────────────────────────

#[tokio::test]
async fn voice_status_and_providers() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(warp::test::request().method("GET").path("/voice/status"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(warp::test::request().method("GET").path("/voice/providers"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = authed(warp::test::request().method("GET").path("/voice/doctor"))
        .reply(&api)
        .await;
    assert_eq!(res.status(), StatusCode::OK);
}

// ── Invalid body ─────────────────────────────────────────────────────────

#[tokio::test]
async fn invalid_json_body_returns_400() {
    let (state, _tmp) = test_state();
    let api = domain_filter(state);

    let res = authed(
        warp::test::request()
            .method("POST")
            .path("/memory")
            .header("content-type", "application/json")
            .body("{not-json"),
    )
    .reply(&api)
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}
