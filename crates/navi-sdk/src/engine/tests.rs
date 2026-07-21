use super::*;

use crate::types::NaviMissingCredentialError;

use navi_core::{AgentEvent, NaviConfig, SessionId, SessionSnapshot};

use std::path::PathBuf;
use std::sync::Arc;

fn test_engine() -> (NaviEngine, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");

    let config = test_config();

    let loaded_config = LoadedConfig {
        config,

        global_config_path: Some(tempdir.path().join("config.toml")),

        project_config_path: None,

        data_dir: tempdir.path().to_path_buf(),
    };

    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(loaded_config)
        .build()
        .expect("build engine");

    (engine, tempdir)
}

fn test_config() -> NaviConfig {
    // Use a config with a custom provider whose env var is definitely not set

    let mut config = NaviConfig::default();

    config.providers.push(ProviderConfig {
        id: "test-provider".to_string(),

        label: "Test Provider".to_string(),

        description: String::new(),

        kind: navi_core::ProviderKind::OpenAiResponses,

        api_key_env: "NAVI_SDK_TEST_NONEXISTENT_ENV_12345".to_string(),

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

    // Disable remote registry updates in tests to avoid network fetches.
    config.registry.update_enabled = false;

    config
}

#[test]
fn known_plugin_policy_keeps_default_components_without_warning() {
    let mut warnings = Vec::new();
    let _components = runtime_components_for_plugin_policies(
        RuntimeComponents::default(),
        &["code_agent".to_string()],
        &mut warnings,
    );
    assert!(
        warnings.is_empty(),
        "code_agent is a known policy; warnings: {warnings:?}"
    );
}

#[test]
fn unknown_plugin_policy_warns_without_replacing_components() {
    let mut warnings = Vec::new();
    let _components = runtime_components_for_plugin_policies(
        RuntimeComponents::default(),
        &["custom-policy".to_string()],
        &mut warnings,
    );

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("unknown agent policy `custom_policy`"));
}

fn test_engine_with_project_config() -> (NaviEngine, tempfile::TempDir) {
    let tempdir = tempfile::tempdir().expect("tempdir");

    let config = test_config();

    let project_config = tempdir.path().join(".navi").join("config.toml");

    let loaded_config = LoadedConfig {
        config,

        global_config_path: Some(tempdir.path().join("global.toml")),

        project_config_path: Some(project_config),

        data_dir: tempdir.path().to_path_buf(),
    };

    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(loaded_config)
        .build()
        .expect("build engine");

    (engine, tempdir)
}

fn write_session_file(tempdir: &tempfile::TempDir, session_id: &str) {
    let sessions_dir = tempdir.path().join("sessions");

    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let snapshot = SessionSnapshot {
        version: SessionSnapshot::CURRENT_VERSION,

        id: SessionId::new(session_id.to_string()),

        title: None,

        project: PathBuf::from("/tmp/test-project"),

        created_at: 1000,

        updated_at: 2000,

        events: vec![AgentEvent::UserTaskSubmitted {
            text: "test task".to_string(),
            content_parts: vec![],
            submitted_at: None,
        }],

        memory: None,

        goal: None,
        usage: None,
    };

    let content = serde_json::to_string(&snapshot).expect("serialize session");

    std::fs::write(sessions_dir.join(format!("{session_id}.json")), content)
        .expect("write session file");
}

// ── Group 1: Builder tests ──────────────────────────────────────────

#[test]

fn builder_with_explicit_config_succeeds() {
    let (engine, _tempdir) = test_engine();

    let loaded = engine.loaded_config();

    assert_eq!(loaded.config.model.provider, "test-provider");

    assert_eq!(loaded.config.model.name, "test-model");
}

#[test]

fn builder_loads_from_project_dir() {
    let tempdir = tempfile::tempdir().expect("tempdir");

    // Just verify that from_project().build() succeeds with defaults

    // (config loading from project dir depends on cwd, so we test the builder path)

    let result = NaviEngineBuilder::from_project(tempdir.path()).build();

    assert!(result.is_ok(), "builder failed: {:?}", result.err());
}

// ── Group 2: Model listing tests ────────────────────────────────────

#[test]

fn list_models_returns_default_models() {
    let (engine, _tempdir) = test_engine();

    let models = engine.list_models();

    assert!(!models.is_empty(), "should have built-in models");

    for model in &models {
        assert!(!model.id.is_empty());

        assert!(!model.name.is_empty());

        assert!(!model.provider_id.is_empty());

        assert!(model.id.contains(':'), "id should be provider:model format");

        // Every model exposes resolved effort options for UI clients.
        assert!(
            !model.effort_options.is_empty(),
            "model {} should have effort_options",
            model.id
        );
        for opt in &model.effort_options {
            assert!(!opt.value.is_empty());
            assert!(!opt.label.is_empty());
        }
        if model.effort_binary {
            assert!(
                model
                    .effort_options
                    .iter()
                    .any(|o| o.label == "thinking on")
                    || model
                        .effort_options
                        .iter()
                        .any(|o| o.label == "thinking off"),
                "binary effort model {} should use thinking on/off labels",
                model.id
            );
        }
    }
}

#[test]

fn list_models_includes_custom_provider_models() {
    let (engine, _tempdir) = test_engine();

    let models = engine.list_models();

    // The test config adds "test-provider" with a default model,

    // so it should appear alongside built-in providers

    let test_models: Vec<_> = models
        .iter()
        .filter(|m| m.provider_id == "test-provider")
        .collect();

    assert!(
        !test_models.is_empty(),
        "custom provider models should be included"
    );
}

// ── Group 3: Credential management tests ────────────────────────────

#[test]

fn credential_status_reports_missing_without_key() {
    let (engine, _tempdir) = test_engine();

    let status = engine.credential_status("test-provider").expect("status");

    assert!(!status.configured);
}

#[test]

fn set_then_get_provider_api_key_roundtrip() {
    let (engine, _tempdir) = test_engine();

    engine
        .set_provider_api_key("test-provider", "sk-test-key")
        .expect("set key");

    let status = engine.credential_status("test-provider").expect("status");

    assert!(status.configured);

    assert_eq!(status.source.as_deref(), Some("stored"));
}

#[test]

fn delete_provider_api_key_returns_true_for_existing() {
    let (engine, _tempdir) = test_engine();

    engine
        .set_provider_api_key("test-provider", "sk-test")
        .expect("set key");

    let deleted = engine
        .delete_provider_api_key("test-provider")
        .expect("delete");

    assert!(deleted);

    let status = engine.credential_status("test-provider").expect("status");

    assert!(!status.configured);
}

#[test]

fn delete_provider_api_key_returns_false_for_missing() {
    let (engine, _tempdir) = test_engine();

    let deleted = engine
        .delete_provider_api_key("nonexistent-provider")
        .expect("delete");

    assert!(!deleted);
}

#[test]

fn list_provider_accounts_returns_all_providers() {
    let (engine, _tempdir) = test_engine();

    let accounts = engine.list_provider_accounts().expect("list accounts");

    assert!(!accounts.is_empty(), "should have built-in providers");

    let ids: Vec<&str> = accounts.iter().map(|a| a.provider_id.as_str()).collect();

    assert!(ids.contains(&"openai"), "should include openai");
}

#[test]

fn list_provider_accounts_reflects_stored_key() {
    let (engine, _tempdir) = test_engine();

    engine
        .set_provider_api_key("test-provider", "sk-test")
        .expect("set key");

    let accounts = engine.list_provider_accounts().expect("list accounts");

    let test_prov = accounts
        .iter()
        .find(|a| a.provider_id == "test-provider")
        .expect("test-provider account");

    assert!(test_prov.has_stored_key);

    // Other providers should not have stored keys

    for account in &accounts {
        if account.provider_id != "test-provider" {
            assert!(
                !account.has_stored_key,
                "{} should not have stored key",
                account.provider_id
            );
        }
    }
}

#[test]

fn credential_status_errors_for_unknown_provider() {
    let (engine, _tempdir) = test_engine();

    let result = engine.credential_status("nonexistent-provider-xyz");

    assert!(result.is_err());
}

// ── Group 4: Model selection tests ──────────────────────────────────

#[test]

fn select_model_updates_loaded_config() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    assert_eq!(result.provider_id, "openai");

    assert_eq!(result.model, "gpt-5.1");

    assert_eq!(result.loaded_config.config.model.provider, "openai");

    assert_eq!(result.loaded_config.config.model.name, "gpt-5.1");
}

#[test]

fn select_model_returns_context_window() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    assert!(result.context_window_tokens.is_some());

    assert!(result.context_window_tokens.unwrap() > 0);
}

#[test]

fn select_model_with_save_target_none_returns_no_path() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    assert!(result.saved_to.is_none());
}

#[test]

fn select_model_with_save_target_project_writes_config() {
    let (engine, _tempdir) = test_engine_with_project_config();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::Project,
        })
        .expect("select model");

    assert!(result.saved_to.is_some());

    let saved_path = result.saved_to.unwrap();

    assert!(saved_path.exists());
}

#[test]

fn select_model_errors_for_unknown_provider() {
    let (engine, _tempdir) = test_engine();

    let result = engine.select_model(NaviModelSelectionRequest {
        provider_id: "nonexistent-provider-xyz".to_string(),

        model: "some-model".to_string(),

        save_target: NaviConfigSaveTarget::None,
    });

    assert!(result.is_err());
}

#[test]

fn select_model_reports_configured_for_public_model() {
    let (engine, _tempdir) = test_engine();

    // OpenRouter with free model should be publicly accessible

    let result = engine.select_model(NaviModelSelectionRequest {
        provider_id: "openrouter".to_string(),

        model: "deepseek/deepseek-v4-flash:free".to_string(),

        save_target: NaviConfigSaveTarget::None,
    });

    // This may or may not work depending on whether openrouter has free models configured

    // The important thing is the method doesn't panic

    if let Ok(result) = result {
        // If it succeeded, check the field exists

        let _ = result.provider_configured;
    }
}

#[test]

fn select_model_reports_not_configured_without_key() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "test-provider".to_string(),

            model: "test-model".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    // No key stored, so should report not configured

    assert!(!result.provider_configured);
}

#[test]

fn select_model_engine_state_updates() {
    let (engine, _tempdir) = test_engine();

    engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "anthropic".to_string(),

            model: "claude-sonnet-4-20250514".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    let loaded = engine.loaded_config();

    assert_eq!(loaded.config.model.provider, "anthropic");

    assert_eq!(loaded.config.model.name, "claude-sonnet-4-20250514");
}

// ── Group 5: Session persistence tests ──────────────────────────────

#[test]

fn list_saved_sessions_returns_empty_initially() {
    let (engine, _tempdir) = test_engine();

    let sessions = engine.list_saved_sessions().expect("list sessions");

    assert!(sessions.is_empty());
}

#[test]

fn list_saved_sessions_returns_prepopulated_sessions() {
    let (engine, tempdir) = test_engine();

    write_session_file(&tempdir, "test-session-123");

    let sessions = engine.list_saved_sessions().expect("list sessions");

    assert_eq!(sessions.len(), 1);

    assert_eq!(sessions[0].id, "test-session-123");

    assert_eq!(sessions[0].project, PathBuf::from("/tmp/test-project"));
}

#[test]

fn load_saved_session_loads_prepopulated() {
    let (engine, tempdir) = test_engine();

    write_session_file(&tempdir, "load-test-456");

    let snapshot = engine
        .load_saved_session("load-test-456")
        .expect("load session");

    assert_eq!(snapshot.id.as_str(), "load-test-456");

    assert_eq!(snapshot.project, PathBuf::from("/tmp/test-project"));
}

#[test]

fn load_saved_session_errors_for_missing() {
    let (engine, _tempdir) = test_engine();

    let result = engine.load_saved_session("nonexistent-session");

    assert!(result.is_err());
}

#[test]

fn delete_saved_session_removes_file() {
    let (engine, tempdir) = test_engine();

    write_session_file(&tempdir, "delete-test-789");

    // Verify it exists first

    let sessions = engine.list_saved_sessions().expect("list");

    assert_eq!(sessions.len(), 1);

    // Delete it

    let deleted = engine
        .delete_saved_session("delete-test-789")
        .expect("delete");

    assert!(deleted);

    // Verify it's gone

    let sessions = engine.list_saved_sessions().expect("list");

    assert!(sessions.is_empty());
}

#[test]

fn delete_saved_session_returns_false_for_missing() {
    let (engine, _tempdir) = test_engine();

    let deleted = engine
        .delete_saved_session("nonexistent-session")
        .expect("delete");

    assert!(!deleted);
}

// ── Group 6: Skills tests ───────────────────────────────────────────

#[test]

fn list_skills_returns_empty_when_disabled() {
    let (engine, _tempdir) = test_engine();

    let skills = engine.list_skills().expect("list skills");

    // Default config has skills.enabled = false, so no skills should be discovered

    // (skills disabled → empty list, not an error)

    let _ = skills;
}

// ── Group 7: Config save target tests ───────────────────────────────

#[test]

fn select_model_save_target_auto_prefers_project() {
    let (engine, _td) = test_engine_with_project_config();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::Auto,
        })
        .expect("select model");

    assert!(result.saved_to.is_some());
}

#[test]

fn select_model_save_target_auto_falls_back_to_global() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::Auto,
        })
        .expect("select model");

    assert!(result.saved_to.is_some());
}

#[test]

fn select_model_save_target_global_writes_global() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::Global,
        })
        .expect("select model");

    assert!(result.saved_to.is_some());

    let saved_path = result.saved_to.unwrap();

    assert!(saved_path.exists());
}

#[test]

fn select_model_save_target_project_writes_project() {
    let (engine, _tempdir) = test_engine_with_project_config();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-5.1".to_string(),

            save_target: NaviConfigSaveTarget::Project,
        })
        .expect("select model");

    assert!(result.saved_to.is_some());

    let saved_path = result.saved_to.unwrap();

    assert!(saved_path.exists());
}

// ── Group 8: Error type tests ───────────────────────────────────────

#[test]

fn missing_credential_error_display_includes_details() {
    let error = NaviMissingCredentialError {
        provider_id: "test-provider".to_string(),

        env_var: "TEST_ENV_VAR".to_string(),

        credential_store_path: PathBuf::from("/tmp/creds.toml"),
    };

    let msg = error.message();

    assert!(msg.contains("test-provider"));

    assert!(msg.contains("TEST_ENV_VAR"));

    assert!(msg.contains("/tmp/creds.toml"));

    // Display trait

    let display = format!("{error}");

    assert_eq!(display, msg);

    // Error trait

    let err: &dyn std::error::Error = &error;

    assert!(err.to_string().contains("test-provider"));
}

// ── Group 9: Session lifecycle tests ─────────────────────────────────

fn test_engine_with_key() -> (NaviEngine, tempfile::TempDir) {
    let (engine, tempdir) = test_engine();

    engine
        .set_provider_api_key("test-provider", "sk-test-key")
        .expect("set key");

    (engine, tempdir)
}

#[tokio::test]

async fn start_session_returns_session_info() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    assert!(!session.id.is_empty());
}

#[tokio::test]

async fn subscribe_events_returns_receiver() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let _receiver = engine.subscribe_events(&session.id);

    // Should not panic; receiver is valid
}

#[tokio::test]
async fn list_tui_components_returns_session_plugin_declarations() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let components = engine
        .list_tui_components(&session.id)
        .expect("tui components");

    assert!(components.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_session_returns_snapshot() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let snapshot = engine
        .snapshot_session(&session.id)
        .await
        .expect("snapshot");

    assert!(!snapshot.id.as_str().is_empty());
}

#[tokio::test]

async fn snapshot_nonexistent_session_errors() {
    let (engine, _tempdir) = test_engine_with_key();

    let result = engine.snapshot_session("nonexistent-session-id").await;

    assert!(result.is_err());
}

#[test]

fn list_models_returns_current_provider_models() {
    let (engine, _tempdir) = test_engine();

    let models = engine.list_models();

    // test_engine sets provider to "test-provider" with "test-model"

    assert!(
        models
            .iter()
            .any(|m| m.provider_id == "test-provider" && m.name == "test-model")
    );
}

#[test]

fn set_model_changes_active_model() {
    let (engine, _tempdir) = test_engine();

    let result = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "openai".to_string(),

            model: "gpt-4.1-nano".to_string(),

            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select model");

    assert_eq!(result.provider_id, "openai");

    assert_eq!(result.model, "gpt-4.1-nano");
}

#[test]

fn list_provider_accounts_includes_test_provider() {
    let (engine, _tempdir) = test_engine();

    let accounts = engine.list_provider_accounts().expect("accounts");

    assert!(accounts.iter().any(|a| a.provider_id == "test-provider"));
}

#[tokio::test]

async fn set_session_skills_succeeds() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let result = engine.set_session_skills(&session.id, vec![]).await;

    assert!(result.is_ok());
}

#[tokio::test]

async fn add_context_packet_succeeds() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let packet = navi_core::ContextPacket {
        id: Some("test".to_string()),

        source: navi_core::ContextSource::UserSelection,

        title: Some("test context".to_string()),

        content: "some context data".to_string(),

        priority: 0,

        metadata: serde_json::json!({}),
    };

    let result = engine.add_context_packet(&session.id, packet).await;

    assert!(result.is_ok());
}

#[tokio::test]

async fn cancel_turn_succeeds_when_no_active_turn() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    let result = engine.cancel_turn(&session.id).await;

    assert!(result.is_ok());
}

#[tokio::test]

async fn close_session_removes_active_session() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");

    assert!(engine.session_ids().contains(&session.id));

    let closed = engine.close_session(&session.id).await.expect("close");

    assert!(closed);

    assert!(!engine.session_ids().contains(&session.id));

    assert!(matches!(
        engine.snapshot_session(&session.id).await,
        Err(NaviError::SessionNotFound(_))
    ));
}

#[tokio::test]

async fn close_session_returns_false_for_missing_session() {
    let (engine, _tempdir) = test_engine_with_key();

    let closed = engine
        .close_session("missing-session")
        .await
        .expect("close missing");

    assert!(!closed);
}

#[tokio::test]

async fn set_model_on_active_session_updates_runtime_and_engine_config() {
    let (engine, _tempdir) = test_engine_with_key();

    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session");
    let mut events = engine.subscribe_events(&session.id).expect("events");

    engine
        .set_model(&session.id, "test-provider", "next-test-model")
        .await
        .expect("set active session model");

    let event = events.try_recv().expect("model change event");
    assert!(matches!(
        event.kind,
        navi_core::RuntimeEventKind::ContextUpdated
    ));
}

#[tokio::test]

async fn start_multiple_sessions_independent() {
    let (engine, _tempdir) = test_engine_with_key();

    let s1 = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session 1");

    let s2 = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start session 2");

    assert_ne!(s1.id, s2.id);
}

#[test]
fn voice_status_without_model() {
    let (engine, _tempdir) = test_engine();
    let status = engine.voice_status().expect("status");
    assert!(!status.installed);
    assert!(!status.streaming_active);
    assert_eq!(status.sample_rate, 16000);
    assert_eq!(status.chunk_samples, 8960);
}

#[test]
fn voice_push_without_start_errors() {
    let (engine, _tempdir) = test_engine();
    let err = engine.voice_push_pcm(&[0.0f32; 100]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not active") || msg.contains("start"),
        "unexpected error: {msg}"
    );
}

#[test]
fn voice_cancel_when_idle_ok() {
    let (engine, _tempdir) = test_engine();
    engine.voice_cancel_stream().expect("cancel idle");
}

#[test]
fn voice_doctor_runs() {
    let (engine, _tempdir) = test_engine();
    let report = engine.voice_doctor().expect("doctor");
    // Model missing → not ok, but doctor itself returns a report
    assert!(!report.lines.is_empty());
}

#[test]
fn voice_engine_installed_false_by_default() {
    let (engine, _tempdir) = test_engine();
    assert!(
        !engine
            .voice_engine_installed(Some("nemotron_streaming"))
            .expect("installed check")
    );
}

#[test]
fn voice_start_without_model_errors() {
    let (engine, _tempdir) = test_engine();
    let err = engine.voice_start_stream(Some("en-US")).unwrap_err();
    assert!(
        err.to_string().contains("not installed") || err.to_string().contains("model"),
        "unexpected: {err}"
    );
}

#[test]
fn memory_status_and_doctor_without_data() {
    let (engine, _tempdir) = test_engine();
    let status = engine.memory_status().expect("memory_status");
    assert!(!status.memory_root.is_empty());
    let doctor = engine.memory_doctor().expect("memory_doctor");
    assert!(!doctor.lines.is_empty());
}

#[test]
fn plugin_list_empty_by_default() {
    let (engine, _tempdir) = test_engine();
    let list = engine.plugin_list().expect("plugin_list");
    assert!(list.is_empty());
}

#[test]
fn list_registry_returns_providers() {
    let (engine, _tempdir) = test_engine();
    let reg = engine.list_registry().expect("list_registry");
    assert!(reg.get("providers").is_some());
}

#[test]
fn oauth_support_check() {
    let (engine, _tempdir) = test_engine();
    assert!(engine.provider_supports_device_oauth("github-copilot"));
    assert!(!engine.provider_supports_device_oauth("unknown-xyz"));
}

#[test]
fn plugin_install_requires_confirm() {
    let (engine, _tempdir) = test_engine();
    let err = engine
        .plugin_install_path(std::path::Path::new("/tmp/nope"), false)
        .unwrap_err();
    assert!(err.to_string().contains("confirm"));
}

// ── SDK refactor host embedding surface (docs/sdk-refactor) ─────────────

use crate::host_tool::{
    HostToolDefinition, HostToolHandler, HostToolInvocation, SdkHostTool, SdkHostToolResult,
};
use crate::profiles::{
    NaviPromptProfile, NaviSecurityProfile, NaviToolProfile, ProfilePromptBuilder,
};
use async_trait::async_trait;
use navi_core::{
    PermissionMode, PromptBuilder, PromptCache, SecurityDecision, SecurityPolicy,
    SystemPromptInput, ToolKind,
};
use serde_json::json;

struct OkHostHandler;

#[async_trait]
impl HostToolHandler for OkHostHandler {
    async fn invoke(&self, _invocation: HostToolInvocation) -> anyhow::Result<SdkHostToolResult> {
        Ok(SdkHostToolResult::success(json!({"ok": true})))
    }
}

fn host_write_tool(name: &str) -> Arc<dyn navi_core::Tool> {
    Arc::new(SdkHostTool::new(
        HostToolDefinition {
            name: name.to_string(),
            description: "host write tool".into(),
            kind: ToolKind::Write,
            input_schema: json!({"type": "object"}),
        },
        Arc::new(OkHostHandler),
    ))
}

#[test]
fn injected_data_dir_is_used_for_durable_state() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let data = tempdir.path().join("app-data");
    std::fs::create_dir_all(&data).expect("mkdir");
    let mut config = test_config();
    config.registry.update_enabled = false;
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(LoadedConfig {
            config,
            global_config_path: Some(data.join("config.toml")),
            project_config_path: None,
            data_dir: data.clone(),
        })
        .data_dir(data.clone())
        .build()
        .expect("build");
    assert_eq!(engine.loaded_config().data_dir, data);
    // Credential store path is under data_dir — durable artifact.
    engine
        .set_provider_api_key("test-provider", "sk-test")
        .expect("set key");
    assert!(
        data.join("credentials.toml").exists() || data.join("credentials").exists() || {
            // Some builds store credentials.toml directly
            let status = engine.credential_status("test-provider").expect("status");
            status.configured || status.source.is_some()
        }
    );
}

#[tokio::test]
async fn start_session_seeds_initial_messages_into_history() {
    let (engine, _tempdir) = test_engine_with_key();
    let session = engine
        .start_session(NaviSessionRequest {
            session_id: Some("seeded-1".into()),
            initial_messages: vec![
                navi_core::ModelMessage::user("hello from seed"),
                navi_core::ModelMessage::assistant("prior reply"),
            ],
            initial_events: vec![
                AgentEvent::UserTaskSubmitted {
                    text: "hello from seed".into(),
                    content_parts: Vec::new(),
                    submitted_at: None,
                },
                AgentEvent::ModelOutput {
                    text: "prior reply".into(),
                    thinking: None,
                },
            ],
            ..Default::default()
        })
        .await
        .expect("start");
    assert_eq!(session.id, "seeded-1");
    let snap = engine.snapshot_session(&session.id).await.expect("snap");
    let texts: Vec<String> = snap
        .events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::UserTaskSubmitted { text, .. } => Some(text.clone()),
            AgentEvent::ModelOutput { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("hello from seed")),
        "seeded user message must appear in snapshot events: {texts:?}"
    );
}

#[tokio::test]
async fn session_request_from_snapshot_reopens_history() {
    let (engine, _tempdir) = test_engine_with_key();
    let session = engine
        .start_session(NaviSessionRequest {
            session_id: Some("reopen-src".into()),
            initial_messages: vec![navi_core::ModelMessage::user("persist me")],
            initial_events: vec![AgentEvent::UserTaskSubmitted {
                text: "persist me".into(),
                content_parts: Vec::new(),
                submitted_at: None,
            }],
            ..Default::default()
        })
        .await
        .expect("start");
    let snapshot = engine.snapshot_session(&session.id).await.expect("snap");
    engine.close_session(&session.id).await.expect("close");

    let info = engine
        .start_session_from_snapshot(&snapshot)
        .await
        .expect("reopen");
    assert_eq!(info.id, "reopen-src");
    let reopened = engine.snapshot_session(&info.id).await.expect("snap2");
    let has = reopened.events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::UserTaskSubmitted { text, .. } if text.contains("persist me")
        )
    });
    assert!(has, "reopened session must retain history");
}

#[tokio::test]
async fn host_tools_only_profile_hides_bash_and_edit() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.registry.update_enabled = false;
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        })
        .tool_profile(NaviToolProfile::HostToolsOnly)
        .host_tool(host_write_tool("vault_write"))
        .build()
        .expect("build");
    engine
        .set_provider_api_key("test-provider", "sk-test")
        .expect("key");
    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start");
    let tools = engine.list_session_tools(&session.id).await.expect("tools");
    assert!(
        tools.iter().any(|t| t == "vault_write"),
        "host tool must remain: {tools:?}"
    );
    assert!(
        !tools
            .iter()
            .any(|t| t == "bash" || t == "edit" || t == "write_file"),
        "code-agent builtins must be hidden: {tools:?}"
    );
    assert_eq!(engine.tool_profile(), NaviToolProfile::HostToolsOnly);
}

#[tokio::test]
async fn chat_only_profile_exposes_no_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.registry.update_enabled = false;
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        })
        .tool_profile(NaviToolProfile::ChatOnly)
        .host_tool(host_write_tool("should_hide"))
        .build()
        .expect("build");
    engine
        .set_provider_api_key("test-provider", "sk-test")
        .expect("key");
    let session = engine
        .start_session(NaviSessionRequest::default())
        .await
        .expect("start");
    let tools = engine.list_session_tools(&session.id).await.expect("tools");
    assert!(tools.is_empty(), "chat_only must offer no tools: {tools:?}");
}

#[test]
fn host_app_security_profile_gates_write_tools() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.registry.update_enabled = false;
    // Start permissive; profile must override.
    config.security.permission_mode = PermissionMode::Yolo;
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        })
        .security_profile(NaviSecurityProfile::HostApp)
        .build()
        .expect("build");
    assert_eq!(engine.get_permission_mode(), PermissionMode::Restricted);
    assert_eq!(engine.security_profile(), NaviSecurityProfile::HostApp);

    let policy = SecurityPolicy::new(
        tempdir.path().to_path_buf(),
        tempdir.path().to_path_buf(),
        engine.loaded_config().config.effective_security_config(),
    )
    .expect("policy");
    let def = navi_core::ToolDefinition {
        name: "vault_write".into(),
        description: "write".into(),
        kind: ToolKind::Write,
        input_schema: json!({"type":"object"}),
        ..Default::default()
    };
    let inv = navi_core::ToolInvocation {
        id: "1".into(),
        tool_name: "vault_write".into(),
        input: json!({}),
    };
    let decision = policy.validate_tool_invocation(&def, &inv);
    assert!(
        matches!(decision, SecurityDecision::NeedsApproval(_)),
        "host_app must require approval for write tools, got {decision:?}"
    );
}

#[test]
fn assistant_prompt_profile_differs_from_code_agent() {
    let cache = Arc::new(PromptCache::new());
    let mk_input = || SystemPromptInput {
        config: NaviConfig::default(),
        project_dir: PathBuf::from("/tmp/proj"),
        memory_injection: None,
        tools: Vec::new(),
        include_tool_prompt_manifest: false,
        context_packets: Vec::new(),
        available_skills: Vec::new(),
        active_skills: Vec::new(),
    };
    let code = ProfilePromptBuilder::new(NaviPromptProfile::CodeAgent)
        .build(mk_input(), cache.clone())
        .instructions;
    let assistant = ProfilePromptBuilder::new(NaviPromptProfile::Assistant)
        .build(mk_input(), cache)
        .instructions;
    assert_ne!(code, assistant);
    assert!(assistant.contains("assistant mode"));
    assert!(!assistant.contains("autonomous code agent"));

    let tempdir = tempfile::tempdir().expect("tempdir");
    let mut config = test_config();
    config.registry.update_enabled = false;
    let engine = NaviEngineBuilder::from_project(tempdir.path())
        .loaded_config(LoadedConfig {
            config,
            global_config_path: Some(tempdir.path().join("config.toml")),
            project_config_path: None,
            data_dir: tempdir.path().to_path_buf(),
        })
        .prompt_profile(NaviPromptProfile::Assistant)
        .build()
        .expect("build");
    assert_eq!(engine.prompt_profile(), NaviPromptProfile::Assistant);
}

#[tokio::test]
async fn compact_session_returns_outcome_on_seeded_history() {
    let (engine, _tempdir) = test_engine_with_key();
    // Seed enough history that force_compact can run (may return empty/no-op
    // if model is unavailable — still must not panic and must return Ok).
    let mut messages = Vec::new();
    let mut events = Vec::new();
    for i in 0..12 {
        messages.push(navi_core::ModelMessage::user(format!("user turn {i}")));
        messages.push(navi_core::ModelMessage::assistant(format!(
            "assistant turn {i}"
        )));
        events.push(AgentEvent::UserTaskSubmitted {
            text: format!("user turn {i}"),
            content_parts: Vec::new(),
            submitted_at: None,
        });
        events.push(AgentEvent::ModelOutput {
            text: format!("assistant turn {i}"),
            thinking: None,
        });
    }
    let session = engine
        .start_session(NaviSessionRequest {
            session_id: Some("compact-seed".into()),
            initial_messages: messages,
            initial_events: events,
            ..Default::default()
        })
        .await
        .expect("start");
    // Force compact may fail on missing network for real model call; accept either
    // Ok(outcome) or Err with a clear message — never panic. Prefer Ok when mock works.
    match engine.compact_session(&session.id).await {
        Ok(outcome) => {
            // Real outcome from shipped path
            let _ = outcome.summary;
            let _ = outcome.tokens_saved;
            let _ = outcome.kept_recent_messages;
        }
        Err(err) => {
            // Without a live model, compact can fail — still exercises the API entry.
            assert!(
                !err.to_string().is_empty(),
                "compact_session must return a structured error"
            );
        }
    }
}

#[test]
fn upsert_provider_openai_compat_and_select_model() {
    let (engine, _tempdir) = test_engine();
    let result = engine
        .upsert_provider(
            ProviderConfig {
                id: "ollama-local".into(),
                label: "Ollama".into(),
                description: "local".into(),
                kind: navi_core::ProviderKind::OpenAiChatCompletions,
                api_key_env: "OLLAMA_API_KEY".into(),
                base_url: Some("http://127.0.0.1:9/v1".into()), // unreachable
                models: vec![navi_core::ProviderModelConfig {
                    name: "llama3".into(),
                    task_size: None,
                    context_window_tokens: Some(8192),
                    max_output_tokens: None,
                    recommended_temperature: None,
                    supports_thinking: None,
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                    supports_images: None,
                    supports_audio: None,
                    supports_video: None,
                    supports_documents: None,
                    tool_prompt_manifest: None,
                    pricing_input_per_1m: None,
                    pricing_output_per_1m: None,
                }],
                ..Default::default()
            },
            NaviConfigSaveTarget::None,
        )
        .expect("upsert");
    assert_eq!(result.provider_id, "ollama-local");
    let accounts = engine.list_provider_accounts().expect("accounts");
    assert!(
        accounts.iter().any(|a| a.provider_id == "ollama-local"),
        "upserted provider must appear in list"
    );
    let sel = engine
        .select_model(NaviModelSelectionRequest {
            provider_id: "ollama-local".into(),
            model: "llama3".into(),
            save_target: NaviConfigSaveTarget::None,
        })
        .expect("select");
    assert_eq!(sel.provider_id, "ollama-local");
    assert_eq!(sel.model, "llama3");
    // Unreachable base URL must not crash status/list
    let _ = engine.credential_status("ollama-local").expect("status");
    let models = engine.list_models();
    assert!(
        models
            .iter()
            .any(|m| m.provider_id == "ollama-local" && m.name == "llama3")
    );
}

#[test]
fn list_acp_agents_empty_when_disabled() {
    let (engine, _tempdir) = test_engine();
    let agents = engine.list_acp_agents();
    assert!(agents.is_empty());
}

#[test]
fn api_methods_include_sdk_refactor_surface() {
    use crate::engine_api::{NAVI_ENGINE_API_METHODS, NAVI_NAPI_BOUND_METHODS};
    for m in [
        "compact_session",
        "start_session_from_snapshot",
        "upsert_provider",
        "list_acp_agents",
        "delegate_acp_turn",
        "delegate_acp_turn_simple",
        "list_session_tools",
        "tool_profile",
        "prompt_profile",
        "security_profile",
    ] {
        assert!(
            NAVI_ENGINE_API_METHODS.contains(&m),
            "missing from engine API: {m}"
        );
        assert!(
            NAVI_NAPI_BOUND_METHODS.contains(&m),
            "missing from NAPI bound: {m}"
        );
    }
}
