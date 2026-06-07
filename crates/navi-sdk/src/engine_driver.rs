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
    ApprovalDecision, LoadedConfig, NaviConfigSaveTarget, NaviModelSelectionRequest,
    NaviModelSelectionResult, NaviProviderCredentialStatus, NaviProviderSyncReport,
    NaviSessionInfo, NaviSessionRequest, NaviSkillInfo, NaviTurnRequest, NaviTurnResponse,
    RuntimeEvent, SessionSnapshot,
};

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

    /// Resolve a pending tool approval.
    async fn resolve_approval(&self, session_id: &str, decision: ApprovalDecision) -> Result<bool>;

    /// Take a persistence snapshot of the session.
    async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot>;

    /// Reload WASM plugin tools across all active sessions.
    async fn reload_wasm_plugins(&self) -> Result<Vec<String>>;

    // ── Provider / model management ────────────────────────────────────

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

    /// Current in-memory configuration snapshot.
    fn loaded_config(&self) -> LoadedConfig;

    // ── Credentials ────────────────────────────────────────────────────

    /// Status of a single provider's credential.
    fn credential_status(&self, provider_id: &str) -> Result<NaviProviderCredentialStatus>;

    /// Store an API key for a provider in the credential store.
    fn set_provider_api_key(&self, provider_id: &str, api_key: &str) -> Result<()>;

    // ── Skills ─────────────────────────────────────────────────────────

    /// Discover and list configured skills.
    fn list_skills(&self) -> Result<Vec<NaviSkillInfo>>;

    // ── Saved sessions ─────────────────────────────────────────────────

    /// Delete a saved session. Returns `true` if a session was removed.
    fn delete_saved_session(&self, session_id: &str) -> Result<bool>;
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

    async fn resolve_approval(&self, session_id: &str, decision: ApprovalDecision) -> Result<bool> {
        crate::NaviEngine::resolve_approval(self, session_id, decision).await
    }

    async fn snapshot_session(&self, session_id: &str) -> Result<SessionSnapshot> {
        crate::NaviEngine::snapshot_session(self, session_id).await
    }

    async fn reload_wasm_plugins(&self) -> Result<Vec<String>> {
        crate::NaviEngine::reload_wasm_plugins(self).await
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

    fn loaded_config(&self) -> LoadedConfig {
        crate::NaviEngine::loaded_config(self)
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
}
