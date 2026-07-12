//! The `EngineDriver` trait abstracts the surface of [`NaviEngine`] that the
//! TUI and other local clients depend on, so a mock can be substituted in
//! integration tests.
//!
//! The blanket impl for [`NaviEngine`] preserves the existing public SDK API:
//! any code that already holds a [`NaviEngine`] continues to compile, and the
//! coercion `Arc<NaviEngine>` → `Arc<dyn EngineDriver>` is automatic.
//!
//! Add a method to this trait only when at least one real client needs it.
//! Methods are the minimum set the TUI drives; SDK-only methods (provider
//! sync, model selection, etc.) live on the inherent impls.

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::types::NaviError;
use crate::{
    ApprovalDecision, BackgroundCommandSnapshot, LoadedConfig, NaviConfigSaveTarget,
    NaviModelSelectionRequest, NaviModelSelectionResult, NaviProviderCredentialStatus,
    NaviProviderSyncReport, NaviSessionInfo, NaviSessionRequest, NaviSkillInfo, NaviTurnRequest,
    NaviTurnResponse, NaviUsageReport, QuestionResponse, RuntimeEvent, SessionSnapshot,
};
use navi_mcp::McpServerInfo;

/// Convenience alias matching the one used inside `engine.rs`.
pub type Result<T> = std::result::Result<T, NaviError>;

/// The TUI-facing engine surface. Implemented by [`NaviEngine`] (blanket impl)
/// and by test mocks in `navi_tui::testing`.
///
/// All methods are `Send + Sync` so the engine can be shared across the
/// runtime tasks the TUI spawns.
#[async_trait]
pub trait EngineDriver: Send + Sync {
    // ── Session lifecycle ──────────────────────────────────────────────

    /// Start a new agent session.
    async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo>;

    /// Subscribe to the runtime event stream for an active session. Each call
    /// returns an independent receiver; receivers are cheap to clone but
    /// cannot be shared with another `EngineDriver` instance.
    fn subscribe_events(&self, session_id: &str) -> Result<broadcast::Receiver<RuntimeEvent>>;

    /// Send a user turn and wait for the assistant to finish streaming.
    async fn send_turn(&self, request: NaviTurnRequest) -> Result<NaviTurnResponse>;

    /// Cancel the currently active turn for the given session, if any.
    async fn cancel_turn(&self, session_id: &str) -> Result<()>;

    /// Rewind live history to the first `keep_user_turns` user turns (edit-message).
    async fn rewind_session(&self, session_id: &str, keep_user_turns: usize) -> Result<usize>;

    /// Returns the current agent mode (Default or Plan).
    fn agent_mode(&self, session_id: &str) -> Result<navi_core::plan_mode::AgentMode>;

    /// Enters Plan mode for the given session.
    async fn enter_plan_mode(&self, session_id: &str) -> Result<()>;

    /// Exits Plan mode and returns to normal execution.
    async fn exit_plan_mode(&self, session_id: &str) -> Result<()>;

    /// Resolve a pending tool approval.
    async fn resolve_approval(&self, session_id: &str, decision: ApprovalDecision) -> Result<bool>;

    /// Resolve a pending interactive question.
    async fn resolve_question(&self, session_id: &str, response: QuestionResponse) -> Result<bool>;

    /// Resolve a pending plan review (unblocks `plan` create).
    async fn resolve_plan_review(
        &self,
        session_id: &str,
        response: crate::PlanReviewResponse,
    ) -> Result<bool>;

    /// Resolve a sudo password prompt (password never logged to chat).
    async fn resolve_sudo_password(
        &self,
        session_id: &str,
        response: crate::SudoPasswordResponse,
    ) -> Result<bool>;

    /// Take a persistence snapshot of the session.
    async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot>;

    /// Close an active in-memory session. Returns `true` when a session was removed.
    async fn close_session(&self, session_id: &str) -> Result<bool>;

    /// Clears the active goal for a session.
    async fn clear_goal(&self, session_id: &str) -> Result<()>;

    /// Reload WASM plugin tools across all active sessions.
    async fn reload_wasm_plugins(&self) -> Result<Vec<String>>;

    // ── Provider / model management ────────────────────────────────────

    /// Sync the remote provider registry into the local SQLite cache.
    async fn sync_registry(&self, force: bool) -> Result<bool>;

    /// Sync the model list for all configured providers.
    async fn sync_models(&self, target: NaviConfigSaveTarget) -> Result<NaviProviderSyncReport>;

    /// Sync the model list for a single provider.
    async fn sync_provider_models(
        &self,
        provider_id: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport>;

    /// Select a model and optionally persist the change.
    fn select_model(&self, request: NaviModelSelectionRequest) -> Result<NaviModelSelectionResult>;

    /// Persist a background-task model route. Active sessions retain their
    /// route; the change is used when a subsequent session starts.
    fn set_background_model(
        &self,
        task: &str,
        provider: &str,
        model: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<()>;

    /// Clear a background-task model route.
    fn clear_background_model(&self, task: &str, target: NaviConfigSaveTarget) -> Result<()>;

    /// Persist an attachment fallback model for a modality (`image`/`audio`/`video`/`document`).
    fn set_attachment_model(
        &self,
        modality: &str,
        provider: &str,
        model: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<()>;

    /// Clear an attachment fallback model for a modality.
    fn clear_attachment_model(&self, modality: &str, target: NaviConfigSaveTarget) -> Result<()>;

    /// Current in-memory configuration snapshot.
    fn loaded_config(&self) -> LoadedConfig;

    /// One-line memory system status for settings/TUI (best-effort).
    fn memory_quick_status(&self) -> Result<String>;

    /// Fetch current provider usage and rate-limit windows.
    async fn usage_report(&self) -> Result<NaviUsageReport>;

    // ── Credentials ────────────────────────────────────────────────────

    /// Status of a single provider's credential.
    fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus>;

    /// Store an API key for a provider in the credential store.
    fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()>;

    // ── Skills & MCP ───────────────────────────────────────────────────

    /// Discover and list configured skills.
    fn list_skills(&self) -> Result<Vec<NaviSkillInfo>>;

    /// List connected MCP servers.
    fn list_mcp_servers(&self, session_id: &str) -> Result<Vec<McpServerInfo>>;

    /// List active background bash commands for a session.
    async fn list_background_commands(
        &self,
        session_id: &str,
    ) -> Result<Vec<BackgroundCommandSnapshot>>;

    /// Poll a specific background bash command for a session.
    async fn poll_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<BackgroundCommandSnapshot>;

    /// Cancel a specific background bash command for a session.
    async fn cancel_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<BackgroundCommandSnapshot>;

    // ── Saved sessions ─────────────────────────────────────────────────

    /// Delete a saved session. Returns `true` if a session was removed.
    fn delete_saved_session(&self, session_id: &str) -> Result<bool>;

    /// Take ownership of TUI component panels registered by native plugins.
    ///
    /// Returns `Box<dyn TuiComponent>` instances that the TUI can register
    /// with its `PanelManager`. Each component is only available once.
    fn take_tui_panels(
        &self,
        session_id: &str,
    ) -> Result<Vec<Box<dyn navi_plugin_api::TuiComponent>>>;
}

// Blanket impl delegating to the inherent methods on `NaviEngine`.
//
// We use UFCS (`NaviEngine::method(self, ...)`) inside the trait method bodies
// to disambiguate from the trait methods themselves — Rust otherwise sees
// `self.method(...)` as ambiguous between the inherent and trait impls.
#[async_trait]
impl EngineDriver for crate::NaviEngine {
    async fn start_session(&self, request: NaviSessionRequest) -> Result<NaviSessionInfo> {
        crate::NaviEngine::start_session(self, request).await
    }

    fn subscribe_events(&self, session_id: &str) -> Result<broadcast::Receiver<RuntimeEvent>> {
        crate::NaviEngine::subscribe_events(self, session_id)
    }

    async fn send_turn(&self, request: NaviTurnRequest) -> Result<NaviTurnResponse> {
        crate::NaviEngine::send_turn(self, request).await
    }

    async fn cancel_turn(&self, session_id: &str) -> Result<()> {
        crate::NaviEngine::cancel_turn(self, session_id).await
    }

    async fn rewind_session(&self, session_id: &str, keep_user_turns: usize) -> Result<usize> {
        crate::NaviEngine::rewind_session(self, session_id, keep_user_turns).await
    }

    fn agent_mode(&self, session_id: &str) -> Result<navi_core::plan_mode::AgentMode> {
        crate::NaviEngine::agent_mode(self, session_id)
    }

    async fn enter_plan_mode(&self, session_id: &str) -> Result<()> {
        crate::NaviEngine::enter_plan_mode(self, session_id).await
    }

    async fn exit_plan_mode(&self, session_id: &str) -> Result<()> {
        crate::NaviEngine::exit_plan_mode(self, session_id).await
    }

    async fn resolve_approval(&self, session_id: &str, decision: ApprovalDecision) -> Result<bool> {
        crate::NaviEngine::resolve_approval(self, session_id, decision).await
    }

    async fn resolve_question(&self, session_id: &str, response: QuestionResponse) -> Result<bool> {
        crate::NaviEngine::resolve_question(self, session_id, response).await
    }

    async fn resolve_plan_review(
        &self,
        session_id: &str,
        response: crate::PlanReviewResponse,
    ) -> Result<bool> {
        crate::NaviEngine::resolve_plan_review(self, session_id, response).await
    }

    async fn resolve_sudo_password(
        &self,
        session_id: &str,
        response: crate::SudoPasswordResponse,
    ) -> Result<bool> {
        crate::NaviEngine::resolve_sudo_password(self, session_id, response).await
    }

    async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        crate::NaviEngine::snapshot_session(self, session_id).await
    }

    async fn close_session(&self, session_id: &str) -> Result<bool> {
        crate::NaviEngine::close_session(self, session_id).await
    }

    async fn clear_goal(&self, session_id: &str) -> Result<()> {
        crate::NaviEngine::clear_goal(self, session_id).await
    }

    async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        crate::NaviEngine::reload_wasm_plugins(self).await
    }

    async fn sync_registry(&self, force: bool) -> Result<bool> {
        crate::NaviEngine::sync_registry(self, force).await
    }

    async fn sync_models(&self, target: NaviConfigSaveTarget) -> Result<NaviProviderSyncReport> {
        crate::NaviEngine::sync_models(self, target).await
    }

    async fn sync_provider_models(
        &self,
        provider_id: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<NaviProviderSyncReport> {
        crate::NaviEngine::sync_provider_models(self, provider_id, target).await
    }

    fn select_model(&self, request: NaviModelSelectionRequest) -> Result<NaviModelSelectionResult> {
        crate::NaviEngine::select_model(self, request)
    }

    fn set_background_model(
        &self,
        task: &str,
        provider: &str,
        model: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<()> {
        crate::NaviEngine::set_background_model(self, task, provider, model, target).map(|_| ())
    }

    fn clear_background_model(&self, task: &str, target: NaviConfigSaveTarget) -> Result<()> {
        crate::NaviEngine::clear_background_model(self, task, target).map(|_| ())
    }

    fn set_attachment_model(
        &self,
        modality: &str,
        provider: &str,
        model: &str,
        target: NaviConfigSaveTarget,
    ) -> Result<()> {
        crate::NaviEngine::set_attachment_model(self, modality, provider, model, target).map(|_| ())
    }

    fn clear_attachment_model(&self, modality: &str, target: NaviConfigSaveTarget) -> Result<()> {
        crate::NaviEngine::clear_attachment_model(self, modality, target).map(|_| ())
    }

    fn loaded_config(&self) -> LoadedConfig {
        crate::NaviEngine::loaded_config(self)
    }

    fn memory_quick_status(&self) -> Result<String> {
        match crate::NaviEngine::memory_status(self) {
            Ok(s) => Ok(format!(
                "{} · {} active · embeddings {}",
                if s.enabled { "on" } else { "off" },
                s.active_memories,
                if s.embeddings_available { "ready" } else { "missing" }
            )),
            Err(err) => Err(err),
        }
    }

    async fn usage_report(&self) -> Result<NaviUsageReport> {
        crate::NaviEngine::usage_report(self).await
    }

    fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus> {
        crate::NaviEngine::credential_status(self, provider_id)
    }

    fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        crate::NaviEngine::set_provider_api_key(self, provider_id, api_key)
    }

    fn list_skills(&self) -> Result<Vec<NaviSkillInfo>> {
        crate::NaviEngine::list_skills(self)
    }

    fn delete_saved_session(&self, session_id: &str) -> Result<bool> {
        crate::NaviEngine::delete_saved_session(self, session_id)
    }

    fn take_tui_panels(
        &self,
        session_id: &str,
    ) -> Result<Vec<Box<dyn navi_plugin_api::TuiComponent>>> {
        crate::NaviEngine::take_tui_panels(self, session_id)
    }

    fn list_mcp_servers(&self, session_id: &str) -> Result<Vec<McpServerInfo>> {
        crate::NaviEngine::list_mcp_servers(self, session_id)
    }

    async fn list_background_commands(
        &self,
        session_id: &str,
    ) -> Result<Vec<BackgroundCommandSnapshot>> {
        crate::NaviEngine::list_background_commands(self, session_id).await
    }

    async fn poll_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<BackgroundCommandSnapshot> {
        crate::NaviEngine::poll_background_command(self, session_id, task_id).await
    }

    async fn cancel_background_command(
        &self,
        session_id: &str,
        task_id: &str,
    ) -> Result<BackgroundCommandSnapshot> {
        crate::NaviEngine::cancel_background_command(self, session_id, task_id).await
    }
}
