use crate::goal::types::{GoalStatus, SessionGoal};
use crate::session::SessionStore;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use super::runtime::GoalRuntimeHandle;

/// Public API for managing goals across sessions.
///
/// Maintains a registry of all active goal runtimes keyed by session id.
/// Goal persistence is handled via `SessionSnapshot.goal`.
pub struct GoalService {
    /// Active goal runtimes, keyed by session id.
    runtimes: RwLock<HashMap<String, Arc<GoalRuntimeHandle>>>,
}

impl GoalService {
    /// Creates a new goal service.
    pub fn new() -> Self {
        Self {
            runtimes: RwLock::new(HashMap::new()),
        }
    }

    /// Sets or updates the goal for a session.
    pub fn set_goal(
        &self,
        session_id: String,
        objective: String,
        token_budget: Option<i64>,
    ) -> SessionGoal {
        let runtimes = self.runtimes.read().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = runtimes.get(&session_id) {
            runtime.set_objective(objective, token_budget)
        } else {
            drop(runtimes);
            SessionGoal::new(session_id, objective, token_budget)
        }
    }

    /// Updates the status of an existing goal.
    pub fn update_goal_status(&self, session_id: &str, status: GoalStatus) -> Option<SessionGoal> {
        let runtimes = self.runtimes.read().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = runtimes.get(session_id) {
            let goal = runtime.get_goal();
            if let Some(mut goal) = goal {
                goal.transition_to(status);
                runtime.update_goal(goal.clone());
                return Some(goal);
            }
        }
        None
    }

    /// Gets the current goal for a session.
    pub fn get_goal(&self, session_id: &str) -> Option<SessionGoal> {
        let runtimes = self.runtimes.read().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = runtimes.get(session_id) {
            runtime.get_goal()
        } else {
            None
        }
    }

    /// Clears the goal for a session. Also clears the runtime handle's state.
    pub fn clear_goal(&self, session_id: &str) -> bool {
        let mut runtimes = self.runtimes.write().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = runtimes.get(session_id) {
            runtime.clear_goal();
        }
        runtimes.remove(session_id);
        true
    }

    /// Registers a runtime handle for a session.
    pub fn register_runtime(&self, session_id: String, runtime: Arc<GoalRuntimeHandle>) {
        let mut runtimes = self.runtimes.write().unwrap_or_else(|e| e.into_inner());
        runtimes.insert(session_id, runtime);
    }

    /// Unregisters a runtime handle for a session.
    pub fn unregister_runtime(&self, session_id: &str) {
        let mut runtimes = self.runtimes.write().unwrap_or_else(|e| e.into_inner());
        runtimes.remove(session_id);
    }

    /// Persists the goal for a session.
    ///
    /// The main runtime persists goals in `SessionSnapshot.goal`; this legacy
    /// helper keeps the standalone `goal.json` path available for callers that
    /// still use `GoalService` directly.
    pub fn persist_goal(
        &self,
        session_id: &str,
        _project_dir: &Path,
        session_store: &SessionStore,
    ) -> anyhow::Result<()> {
        if let Some(goal) = self.get_goal(session_id) {
            let json = serde_json::to_string_pretty(&goal)?;
            let dir = session_store.root().join(session_id);
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("goal.json");
            std::fs::write(&path, json)?;
        }
        Ok(())
    }

    /// Loads a persisted goal for a session.
    ///
    /// Prefer the current session snapshot field, then fall back to the older
    /// `<sessions>/<session_id>/goal.json` layout.
    pub fn load_goal(&self, session_id: &str, session_store: &SessionStore) -> Option<SessionGoal> {
        if let Ok(snapshot) = session_store.load(session_id)
            && snapshot.goal.is_some()
        {
            return snapshot.goal;
        }

        let path = session_store.root().join(session_id).join("goal.json");
        if path.exists() {
            let json = std::fs::read_to_string(&path).ok()?;
            serde_json::from_str(&json).ok()
        } else {
            None
        }
    }
}

impl Default for GoalService {
    fn default() -> Self {
        Self::new()
    }
}
