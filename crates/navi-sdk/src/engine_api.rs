//! Compile-time parity enforcement between NaviEngine (SDK) and N-API bindings.
//!
//! This module defines `NAVI_ENGINE_API_METHODS` — an exhaustive list of every
//! public method on `NaviEngine` that external clients (TUI, Tutor, ACP, N-API)
//! should be able to call.
//!
//! Downstream bindings (e.g. `navi-napi`) use this list in a parity test to
//! verify that every SDK method has a corresponding `#[napi]` binding. If a
//! method is added to the SDK but not to the binding, the test fails with a
//! clear message listing the missing methods.
//!
//! **When adding a new public method to `NaviEngine`, add it to this list too.**
//! The parity test in `navi-napi` will fail until a binding is added.

/// Exhaustive list of every public method on `NaviEngine` that must be
/// exposed through all client surfaces (N-API, ACP, etc).
pub const NAVI_ENGINE_API_METHODS: &[&str] = &[
    // Session lifecycle
    "start_session",
    "start_session_from_snapshot",
    "send_turn",
    "cancel_turn",
    "rewind_session",
    "compact_session",
    "close_session",
    "snapshot_session",
    "session_ids",
    "list_session_tools",
    // Goal management (host API; model tools are get_goal/create_goal/update_goal)
    "get_goal",
    "set_goal",
    "set_goal_with_short_description",
    "clear_goal",
    "update_goal_status",
    "update_goal_checklist",
    "update_goal_task_status",
    // Plan mode
    "agent_mode",
    "enter_plan_mode",
    "exit_plan_mode",
    // Approval / question / plan / sudo
    "resolve_approval",
    "resolve_question",
    "resolve_plan_review",
    "resolve_sudo_password",
    "add_context_packet",
    // Model / provider
    "list_models",
    "select_model",
    "set_model",
    "set_attachment_model",
    "clear_attachment_model",
    "set_background_model",
    "clear_background_model",
    "list_provider_accounts",
    "credential_status",
    "set_provider_api_key",
    "delete_provider_api_key",
    "list_credential_accounts",
    "add_provider_account",
    "select_provider_account",
    "delete_provider_account",
    "provider_supports_device_oauth",
    "start_device_oauth",
    "start_device_oauth_simple",
    "upsert_provider",
    "usage_report",
    // Skills
    "list_skills",
    "get_skill",
    "save_skill",
    "delete_skill",
    "set_session_skills",
    // MCP (live session)
    "list_mcp_servers",
    "list_mcp_tools",
    // MCP (config)
    "list_mcp_config",
    "set_mcp_enabled",
    "upsert_mcp_server",
    "remove_mcp_server",
    "set_mcp_config",
    // ACP peers
    "list_acp_agents",
    "delegate_acp_turn",
    "delegate_acp_turn_simple",
    // Events
    "subscribe_events",
    // TUI panels
    "list_tui_components",
    "take_tui_panels",
    "list_tui_extensions",
    "list_tui_extension_commands",
    // Background commands
    "list_background_commands",
    "poll_background_command",
    "cancel_background_command",
    // Memory (CRUD)
    "memory_write",
    "memory_read",
    "memory_list",
    "memory_search",
    "memory_update",
    "memory_delete",
    "memory_count",
    "memory_index",
    // Memory (ops / maintenance)
    "memory_status",
    "memory_doctor",
    "memory_init",
    "memory_history_search",
    "memory_dream",
    "memory_distill",
    "memory_checkpoint",
    "memory_rebuild_preview",
    // Voice / dictation
    "voice_status",
    "voice_transcription_providers",
    "set_voice_config",
    "voice_doctor",
    "voice_engine_installed",
    "voice_init",
    "voice_transcribe_file",
    "voice_transcribe_file_async",
    "voice_start_stream",
    "voice_push_pcm",
    "voice_end_stream",
    "voice_cancel_stream",
    "subscribe_voice_events",
    // Registry
    "sync_registry",
    "list_registry",
    "sync_provider_models",
    "sync_models",
    // Plugins
    "plugin_list",
    "plugin_info",
    "plugin_search",
    "plugin_install_path",
    "plugin_install_path_with_meta",
    "plugin_install_marketplace",
    "plugin_update_path",
    "plugin_update_marketplace",
    "plugin_remove",
    // Saved sessions
    "list_saved_sessions",
    "list_saved_sessions_async",
    "load_saved_session",
    "load_saved_session_async",
    "delete_saved_session",
    "delete_saved_session_async",
    "rename_saved_session",
    "rename_saved_session_async",
    // Permissions / host profiles
    "get_permission_mode",
    "set_permission_mode",
    "tool_profile",
    "prompt_profile",
    "security_profile",
    // WASM plugins
    "reload_wasm_plugins",
    // Config
    "loaded_config",
    // Notifications / self-update
    "notify",
    "notify_simple",
    "open_url",
    "app_version",
    "check_for_update",
    "check_for_update_with",
    "apply_update",
    "auto_update_enabled",
    "set_auto_update",
];

/// Exhaustive list of methods that the N-API binding exposes via `#[napi]`.
/// This must be kept in sync with `navi-napi/src/lib.rs`.
/// When a method is added to `NAVI_ENGINE_API_METHODS`, add it here too
/// and implement the binding.
pub const NAVI_NAPI_BOUND_METHODS: &[&str] = &[
    // Session lifecycle
    "start_session",
    "start_session_from_snapshot",
    "send_turn",
    "cancel_turn",
    "rewind_session",
    "compact_session",
    "close_session",
    "snapshot_session",
    "session_ids",
    "list_session_tools",
    // Goal management (host API; model tools are get_goal/create_goal/update_goal)
    "get_goal",
    "set_goal",
    "set_goal_with_short_description",
    "clear_goal",
    "update_goal_status",
    "update_goal_checklist",
    "update_goal_task_status",
    // Plan mode
    "agent_mode",
    "enter_plan_mode",
    "exit_plan_mode",
    // Approval / question / plan / sudo
    "resolve_approval",
    "resolve_question",
    "resolve_plan_review",
    "resolve_sudo_password",
    "add_context_packet",
    // Model / provider
    "list_models",
    "select_model",
    "set_model",
    "set_attachment_model",
    "clear_attachment_model",
    "set_background_model",
    "clear_background_model",
    "list_provider_accounts",
    "credential_status",
    "set_provider_api_key",
    "delete_provider_api_key",
    "list_credential_accounts",
    "add_provider_account",
    "select_provider_account",
    "delete_provider_account",
    "provider_supports_device_oauth",
    "start_device_oauth",
    "start_device_oauth_simple",
    "upsert_provider",
    "usage_report",
    // Skills
    "list_skills",
    "get_skill",
    "save_skill",
    "delete_skill",
    "set_session_skills",
    // MCP (live session)
    "list_mcp_servers",
    "list_mcp_tools",
    // MCP (config)
    "list_mcp_config",
    "set_mcp_enabled",
    "upsert_mcp_server",
    "remove_mcp_server",
    "set_mcp_config",
    // ACP peers
    "list_acp_agents",
    "delegate_acp_turn",
    "delegate_acp_turn_simple",
    // Events
    "subscribe_events",
    // TUI panels
    "list_tui_components",
    "take_tui_panels",
    "list_tui_extensions",
    "list_tui_extension_commands",
    // Background commands
    "list_background_commands",
    "poll_background_command",
    "cancel_background_command",
    // Memory (CRUD)
    "memory_write",
    "memory_read",
    "memory_list",
    "memory_search",
    "memory_update",
    "memory_delete",
    "memory_count",
    "memory_index",
    // Memory (ops / maintenance)
    "memory_status",
    "memory_doctor",
    "memory_init",
    "memory_history_search",
    "memory_dream",
    "memory_distill",
    "memory_checkpoint",
    "memory_rebuild_preview",
    // Voice / dictation
    "voice_status",
    "voice_transcription_providers",
    "set_voice_config",
    "voice_doctor",
    "voice_engine_installed",
    "voice_init",
    "voice_transcribe_file",
    "voice_transcribe_file_async",
    "voice_start_stream",
    "voice_push_pcm",
    "voice_end_stream",
    "voice_cancel_stream",
    "subscribe_voice_events",
    // Registry
    "sync_registry",
    "list_registry",
    "sync_provider_models",
    "sync_models",
    // Plugins
    "plugin_list",
    "plugin_info",
    "plugin_search",
    "plugin_install_path",
    "plugin_install_path_with_meta",
    "plugin_install_marketplace",
    "plugin_update_path",
    "plugin_update_marketplace",
    "plugin_remove",
    // Saved sessions
    "list_saved_sessions",
    "list_saved_sessions_async",
    "load_saved_session",
    "load_saved_session_async",
    "delete_saved_session",
    "delete_saved_session_async",
    "rename_saved_session",
    "rename_saved_session_async",
    // Permissions / host profiles
    "get_permission_mode",
    "set_permission_mode",
    "tool_profile",
    "prompt_profile",
    "security_profile",
    // WASM plugins
    "reload_wasm_plugins",
    // Config
    "loaded_config",
    // Notifications / self-update
    "notify",
    "notify_simple",
    "open_url",
    "app_version",
    "check_for_update",
    "check_for_update_with",
    "apply_update",
    "auto_update_enabled",
    "set_auto_update",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_list_is_non_empty() {
        assert!(!NAVI_ENGINE_API_METHODS.is_empty());
    }

    #[test]
    fn method_list_has_no_duplicates() {
        let mut sorted: Vec<&str> = NAVI_ENGINE_API_METHODS.to_vec();
        sorted.sort();
        let dups: Vec<&str> = sorted
            .windows(2)
            .filter(|w| w[0] == w[1])
            .map(|w| w[0])
            .collect();
        assert!(
            dups.is_empty(),
            "duplicate methods in NAVI_ENGINE_API_METHODS: {dups:?}"
        );
    }

    #[test]
    fn napi_bound_matches_engine_api() {
        let engine: std::collections::HashSet<_> =
            NAVI_ENGINE_API_METHODS.iter().copied().collect();
        let napi: std::collections::HashSet<_> = NAVI_NAPI_BOUND_METHODS.iter().copied().collect();
        let missing_in_napi: Vec<_> = engine.difference(&napi).copied().collect();
        let extra_in_napi: Vec<_> = napi.difference(&engine).copied().collect();
        assert!(
            missing_in_napi.is_empty(),
            "SDK methods missing from N-API bound list: {missing_in_napi:?}"
        );
        assert!(
            extra_in_napi.is_empty(),
            "N-API bound methods not in SDK list: {extra_in_napi:?}"
        );
    }
}
