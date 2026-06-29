use crate::goal::runtime::GoalRuntimeHandle;
use crate::goal::service::GoalService;
use crate::goal::types::SessionGoal;
use crate::session::SessionStore;
use std::path::Path;
use std::sync::Arc;

/// Integrates the goal system with session and turn lifecycle.
///
/// Provides explicit hooks that callers should invoke at key points.
pub struct GoalExtension {
    /// Shared goal service.
    service: Arc<GoalService>,
    /// Runtime handle for the current session.
    runtime: Arc<GoalRuntimeHandle>,
}

impl GoalExtension {
    /// Creates a new goal extension backed by the given service and runtime.
    pub fn new(service: Arc<GoalService>, runtime: Arc<GoalRuntimeHandle>) -> Self {
        Self { service, runtime }
    }

    /// Returns the shared goal service.
    pub fn service(&self) -> &Arc<GoalService> {
        &self.service
    }

    /// Returns the runtime handle.
    pub fn runtime(&self) -> &Arc<GoalRuntimeHandle> {
        &self.runtime
    }

    // ── Session lifecycle ──────────────────────────────────────

    /// Called when a session starts. Registers the runtime with the service.
    pub fn on_session_start(&self, session_id: &str) {
        self.service
            .register_runtime(session_id.to_string(), Arc::clone(&self.runtime));
    }

    /// Called when a session is resumed from persistence.
    /// Loads the persisted goal for this session into the runtime.
    pub fn on_session_resume(
        &self,
        session_id: &str,
        session_store: &SessionStore,
    ) -> Option<SessionGoal> {
        if let Some(goal) = self.service.load_goal(session_id, session_store) {
            // Restore the loaded goal into the runtime.
            if goal.status.should_auto_continue() {
                self.runtime.set_auto_continue(true);
            }
            self.runtime.update_goal(goal.clone());
            return Some(goal);
        }
        None
    }

    /// Called when a session ends. Unregisters the runtime and clears state.
    pub fn on_session_end(&self, session_id: &str) {
        self.service.unregister_runtime(session_id);
        self.runtime.clear_goal();
    }

    /// Called when the thread/session becomes idle. Returns a continuation
    /// prompt if the goal should auto-continue.
    pub fn on_idle(&self) -> Option<String> {
        self.runtime.continue_if_idle()
    }

    // ── Turn lifecycle ─────────────────────────────────────────

    /// Called at the start of a turn. Initializes turn-level accounting.
    pub fn on_turn_start(&self, _session_id: &str, _task: &str) {
        self.runtime.start_turn();
    }

    /// Called at the end of a turn. Finalizes turn accounting.
    pub fn on_turn_end(&self, _session_id: &str) {
        self.runtime.finish_turn();
    }

    /// Called when a turn is aborted. Performs cleanup.
    pub fn on_turn_abort(&self, _session_id: &str) {
        self.runtime.finish_turn();
    }

    /// Called when a turn encounters an error.
    pub fn on_turn_error(&self, error_message: &str) {
        let lower = error_message.to_lowercase();
        if lower.contains("usage limit") || lower.contains("rate limit") {
            self.runtime.mark_usage_limited();
        } else if lower.contains("fatal") || lower.contains("blocked") {
            self.runtime.record_blocked_turn(error_message);
        }
    }

    // ── Tool lifecycle ─────────────────────────────────────────

    /// Called after a tool call completes. Checks for budget limits.
    /// Returns `Some(prompt)` if a budget-limit steering prompt should be injected.
    pub fn on_tool_complete(&self) -> Option<String> {
        self.runtime.budget_limit_prompt()
    }

    /// Called when token usage is reported. Records the delta.
    /// Returns `true` if the budget was exceeded by this update.
    pub fn on_token_usage(&self, input_tokens: u64, output_tokens: u64) -> bool {
        let delta = (input_tokens + output_tokens) as i64;
        self.runtime.record_tokens(delta)
    }

    // ── Persistence ────────────────────────────────────────────

    /// Persists the current goal alongside the session snapshot.
    pub fn persist_goal(
        &self,
        session_id: &str,
        project_dir: &Path,
        session_store: &SessionStore,
    ) -> anyhow::Result<()> {
        self.service
            .persist_goal(session_id, project_dir, session_store)
    }
}
