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
    "send_turn",
    "cancel_turn",
    "close_session",
    "snapshot_session",
    "session_ids",
    // Goal management
    "get_goal",
    "set_goal",
    "clear_goal",
    "update_goal_status",
    "update_goal_checklist",
    "update_goal_task_status",
    // Approval / question
    "resolve_approval",
    "resolve_question",
    "add_context_packet",
    // Model / provider
    "list_models",
    "select_model",
    "list_provider_accounts",
    "credential_status",
    "set_provider_api_key",
    "delete_provider_api_key",
    "usage_report",
    // Skills
    "list_skills",
    "set_session_skills",
    // MCP
    "list_mcp_servers",
    "list_mcp_tools",
    // Events
    "subscribe_events",
    // TUI panels
    "list_tui_components",
    "take_tui_panels",
    // Background commands
    "list_background_commands",
    "poll_background_command",
    "cancel_background_command",
    // Memory
    "memory_write",
    "memory_read",
    "memory_list",
    "memory_search",
    "memory_update",
    "memory_delete",
    "memory_count",
    "memory_index",
    // Registry sync
    "sync_registry",
    "sync_provider_models",
    "sync_models",
    // Saved sessions
    "list_saved_sessions",
    "list_saved_sessions_async",
    "load_saved_session",
    "load_saved_session_async",
    "delete_saved_session",
    "delete_saved_session_async",
    // Permissions
    "get_permission_mode",
    "set_permission_mode",
    // WASM plugins
    "reload_wasm_plugins",
    // Config
    "loaded_config",
];

/// Exhaustive list of methods that the N-API binding exposes via `#[napi]`.
/// This must be kept in sync with `navi-napi/src/lib.rs`.
/// When a method is added to `NAVI_ENGINE_API_METHODS`, add it here too
/// and implement the binding.
pub const NAVI_NAPI_BOUND_METHODS: &[&str] = &[
    // Session lifecycle
    "start_session",
    "send_turn",
    "cancel_turn",
    "close_session",
    "snapshot_session",
    "session_ids",
    // Goal management
    "get_goal",
    "set_goal",
    "clear_goal",
    "update_goal_status",
    "update_goal_checklist",
    "update_goal_task_status",
    // Approval / question
    "resolve_approval",
    "resolve_question",
    "add_context_packet",
    // Model / provider
    "list_models",
    "select_model",
    "list_provider_accounts",
    "credential_status",
    "set_provider_api_key",
    "delete_provider_api_key",
    "usage_report",
    // Skills
    "list_skills",
    "set_session_skills",
    // MCP
    "list_mcp_servers",
    "list_mcp_tools",
    // Events
    "subscribe_events",
    // TUI panels
    "list_tui_components",
    "take_tui_panels",
    // Background commands
    "list_background_commands",
    "poll_background_command",
    "cancel_background_command",
    // Memory
    "memory_write",
    "memory_read",
    "memory_list",
    "memory_search",
    "memory_update",
    "memory_delete",
    "memory_count",
    "memory_index",
    // Registry sync
    "sync_registry",
    "sync_provider_models",
    "sync_models",
    // Saved sessions
    "list_saved_sessions",
    "list_saved_sessions_async",
    "load_saved_session",
    "load_saved_session_async",
    "delete_saved_session",
    "delete_saved_session_async",
    // Permissions
    "get_permission_mode",
    "set_permission_mode",
    // WASM plugins
    "reload_wasm_plugins",
    // Config
    "loaded_config",
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
    fn napi_binding_covers_all_sdk_methods() {
        let sdk: std::collections::HashSet<&str> =
            NAVI_ENGINE_API_METHODS.iter().copied().collect();
        let napi: std::collections::HashSet<&str> =
            NAVI_NAPI_BOUND_METHODS.iter().copied().collect();

        let missing: Vec<&&str> = sdk.difference(&napi).collect();
        assert!(
            missing.is_empty(),
            "N-API binding is missing methods that exist in the SDK API: {missing:?}\n\
             Add them to NAVI_NAPI_BOUND_METHODS in engine_api.rs and implement #[napi] bindings."
        );

        let extra: Vec<&&str> = napi.difference(&sdk).collect();
        assert!(
            extra.is_empty(),
            "N-API binding has methods not in the SDK API: {extra:?}\n\
             Remove them from NAVI_NAPI_BOUND_METHODS or add them to NAVI_ENGINE_API_METHODS."
        );
    }
}
