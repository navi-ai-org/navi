use crate::goal::types::SessionGoal;
use std::sync::RwLock;

use super::accounting::GoalAccountingState;
use super::steering;

/// Per-session goal lifecycle manager.
///
/// Drives auto-continuation, status transitions, and steering prompt injection.
/// All mutations go through `with_goal_state` which serializes access via a
/// dedicated state lock, preventing external `set`/`clear` calls from
/// interfering with `continue_if_idle`.
pub struct GoalRuntimeHandle {
    /// The currently bound session id.
    session_id: RwLock<Option<String>>,
    /// The goal state protected by a mutex.
    goal: RwLock<Option<SessionGoal>>,
    /// Accounting state for the current turn.
    accounting: RwLock<Option<GoalAccountingState>>,
    /// Whether auto-continuation is enabled.
    auto_continue: RwLock<bool>,
    /// Serializes goal state mutations to prevent race conditions between
    /// external set/clear and internal continue_if_idle.
    state_lock: std::sync::Mutex<()>,
}

impl GoalRuntimeHandle {
    /// Creates a new runtime handle with an optional initial goal.
    pub fn new(initial_goal: Option<SessionGoal>) -> Self {
        // Initialize accounting if there's an active goal.
        let accounting = initial_goal
            .as_ref()
            .map(|g| GoalAccountingState::new(g.clone()));
        let session_id = initial_goal
            .as_ref()
            .and_then(|goal| (!goal.session_id.is_empty()).then(|| goal.session_id.clone()));
        Self {
            session_id: RwLock::new(session_id),
            goal: RwLock::new(initial_goal),
            accounting: RwLock::new(accounting),
            auto_continue: RwLock::new(true),
            state_lock: std::sync::Mutex::new(()),
        }
    }

    // ── Goal accessors ──────────────────────────────────────────

    /// Returns the current goal, if any.
    pub fn get_goal(&self) -> Option<SessionGoal> {
        self.goal.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Binds this runtime to a session id and rewrites any loaded goal to match.
    pub fn set_session_id(&self, session_id: impl Into<String>) {
        let session_id = session_id.into();
        *self.session_id.write().unwrap_or_else(|e| e.into_inner()) = Some(session_id.clone());
        if let Some(ref mut goal) = *self.goal.write().unwrap_or_else(|e| e.into_inner()) {
            goal.session_id = session_id;
            goal.updated_at = crate::session::current_unix_timestamp();
        }
    }

    /// Sets or replaces the goal.
    pub fn set_objective(&self, objective: String, token_budget: Option<i64>) -> SessionGoal {
        self.set_objective_with_short_description(objective, None, token_budget)
    }

    /// Sets or replaces the goal with an optional compact UI label.
    ///
    /// When a goal already exists, mutates it in place (same `goal_id`) and
    /// re-activates it — used by host/SDK `set_goal`. Model `create_goal` must
    /// call [`create_new_goal`] so terminal goals get a fresh id.
    pub fn set_objective_with_short_description(
        &self,
        objective: String,
        short_description: Option<String>,
        token_budget: Option<i64>,
    ) -> SessionGoal {
        let mut goal_guard = self.goal.write().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut goal) = *goal_guard {
            goal.objective = objective;
            goal.short_description = short_description;
            goal.token_budget = token_budget;
            goal.status = crate::goal::types::GoalStatus::Active;
            goal.consecutive_blocked_turns = 0;
            goal.block_reason = None;
            goal.updated_at = crate::session::current_unix_timestamp();
            let new_goal = goal.clone();
            drop(goal_guard);
            *self.accounting.write().unwrap_or_else(|e| e.into_inner()) =
                Some(GoalAccountingState::new(new_goal.clone()));
            new_goal
        } else {
            let session_id = self
                .session_id
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .unwrap_or_default();
            let mut new_goal = SessionGoal::new(session_id, objective, token_budget);
            new_goal.short_description = short_description;
            let cloned = new_goal.clone();
            *goal_guard = Some(new_goal);
            drop(goal_guard);
            *self.accounting.write().unwrap_or_else(|e| e.into_inner()) =
                Some(GoalAccountingState::new(cloned.clone()));
            cloned
        }
    }

    /// Creates a brand-new active goal (new `goal_id`), replacing any prior goal.
    ///
    /// Caller must ensure there is no unfinished goal when this is used from the
    /// model-facing `create_goal` tool.
    pub fn create_new_goal(
        &self,
        objective: String,
        short_description: Option<String>,
        token_budget: Option<i64>,
    ) -> SessionGoal {
        let session_id = self
            .session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        let mut new_goal = SessionGoal::new(session_id, objective, token_budget);
        new_goal.short_description = short_description;
        let cloned = new_goal.clone();
        *self.goal.write().unwrap_or_else(|e| e.into_inner()) = Some(new_goal);
        *self.accounting.write().unwrap_or_else(|e| e.into_inner()) =
            Some(GoalAccountingState::new(cloned.clone()));
        cloned
    }

    /// Updates the stored goal (used after status transitions).
    pub fn update_goal(&self, mut goal: SessionGoal) {
        if let Some(session_id) = self
            .session_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            goal.session_id = session_id;
        }
        *self.goal.write().unwrap_or_else(|e| e.into_inner()) = Some(goal.clone());
        if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
            acct.replace_goal(goal);
        }
    }

    /// Clears the current goal.
    pub fn clear_goal(&self) {
        *self.goal.write().unwrap_or_else(|e| e.into_inner()) = None;
        *self.accounting.write().unwrap_or_else(|e| e.into_inner()) = None;
        *self.session_id.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Updates the checklist on the current goal. Returns the updated goal if successful.
    pub fn update_checklist(&self, tasks: Vec<super::types::GoalTask>) -> Option<SessionGoal> {
        let mut goal = self.goal.write().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut g) = *goal {
            g.set_checklist(tasks);
            let cloned = g.clone();
            drop(goal);
            if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
                acct.replace_goal(cloned.clone());
            }
            Some(cloned)
        } else {
            None
        }
    }

    /// Updates a single task's status in the goal checklist.
    pub fn update_task_status(
        &self,
        task_id: usize,
        status: super::types::TaskStatus,
    ) -> Option<SessionGoal> {
        let mut goal = self.goal.write().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut g) = *goal {
            if !g.update_task_status(task_id, status) {
                return None;
            }
            let cloned = g.clone();
            drop(goal);
            if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
                acct.replace_goal(cloned.clone());
            }
            Some(cloned)
        } else {
            None
        }
    }

    // ── Accounting ─────────────────────────────────────────────

    /// Starts turn accounting for the active goal.
    pub fn start_turn(&self) {
        let goal = self.get_goal();
        if let Some(goal) = goal
            && goal.status.should_auto_continue()
        {
            let acct = GoalAccountingState::new(goal);
            acct.start_turn();
            *self.accounting.write().unwrap_or_else(|e| e.into_inner()) = Some(acct);
        }
    }

    /// Records token usage during a turn.
    pub fn record_tokens(&self, delta: i64) -> bool {
        if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
            let exceeded = acct.record_token_usage(delta);
            if exceeded {
                // Sync back immediately so budget_limit_prompt works.
                if let Some(goal) = acct.snapshot() {
                    self.update_goal(goal);
                }
            }
            exceeded
        } else {
            false
        }
    }

    /// Finishes turn accounting and updates the stored goal.
    pub fn finish_turn(&self) {
        if let Some(acct) = self
            .accounting
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            && let Some(goal) = acct.finish_turn()
        {
            self.update_goal(goal);
        }
        *self.accounting.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    // ── Status transitions ─────────────────────────────────────

    /// Transitions the goal to Blocked after consecutive blocked turns.
    pub fn record_blocked_turn(&self, reason: &str) -> bool {
        let mut goal_guard = self.goal.write().unwrap_or_else(|e| e.into_inner());
        if let Some(ref mut goal) = *goal_guard {
            let became_blocked = goal.record_blocked_turn(reason);
            let updated = goal.clone();
            drop(goal_guard);
            if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
                acct.replace_goal(updated);
            }
            became_blocked
        } else {
            false
        }
    }

    /// Transitions the goal to UsageLimited.
    pub fn mark_usage_limited(&self) {
        if let Some(ref acct) = *self.accounting.read().unwrap_or_else(|e| e.into_inner()) {
            if let Some(mut goal) = acct.snapshot() {
                goal.transition_to(crate::goal::types::GoalStatus::UsageLimited);
                self.update_goal(goal);
            }
        } else if let Some(mut goal) = self.get_goal() {
            goal.transition_to(crate::goal::types::GoalStatus::UsageLimited);
            self.update_goal(goal);
        }
    }

    // ── Auto-continuation ──────────────────────────────────────

    /// If the goal is active and should auto-continue, returns a steering
    /// continuation prompt to inject into the conversation.
    /// Acquires the state lock to prevent race conditions with concurrent
    /// set/clear operations.
    pub fn continue_if_idle(&self) -> Option<String> {
        let _lock = self.state_lock.lock().unwrap_or_else(|e| e.into_inner());
        let goal = self.get_goal()?;
        if !goal.status.should_auto_continue() {
            return None;
        }
        let auto = *self.auto_continue.read().unwrap_or_else(|e| e.into_inner());
        if !auto {
            return None;
        }
        Some(steering::build_continuation_prompt(&goal))
    }

    /// Returns a budget-limit steering prompt if the budget is exceeded.
    pub fn budget_limit_prompt(&self) -> Option<String> {
        let goal = self.get_goal()?;
        if goal.is_budget_exceeded() {
            Some(steering::build_budget_limit_prompt(&goal))
        } else {
            None
        }
    }

    /// Returns an objective-updated steering prompt.
    pub fn objective_updated_prompt(&self) -> Option<String> {
        let goal = self.get_goal()?;
        if goal.status == crate::goal::types::GoalStatus::Active {
            Some(steering::build_objective_updated_prompt(&goal))
        } else {
            None
        }
    }

    /// Enables or disables auto-continuation.
    pub fn set_auto_continue(&self, enabled: bool) {
        *self
            .auto_continue
            .write()
            .unwrap_or_else(|e| e.into_inner()) = enabled;
    }

    /// Returns `true` if the goal is in a terminal state.
    pub fn is_terminal(&self) -> bool {
        self.get_goal()
            .map(|g| g.status.is_terminal())
            .unwrap_or(false)
    }
}
