use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static GOAL_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique identifier for a goal. Timestamp-based with an incrementing counter to avoid collisions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GoalId(pub String);

impl GoalId {
    /// Creates a new goal id combining the current Unix timestamp with a counter.
    pub fn new() -> Self {
        let ts = crate::session::current_unix_timestamp();
        let seq = GOAL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!("goal-{ts}-{seq:04x}"))
    }

    /// Creates a goal id from an existing string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the goal id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GoalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for GoalId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The goal status state machine.
///
/// ```text
/// Active ───────────────────────────────► Complete
///   │                                       ▲
///   ├──► Paused (by user)                   │
///   ├──► Blocked (3+ consecutive turns      │
///   │     with same blocker, or fatal error) │
///   ├──► UsageLimited (API usage limit      │
///   │     reached)                           │
///   └──► BudgetLimited (token_budget        │
///         exceeded) ────────────────────────┘
/// ```
///
/// Terminal states: `Complete`, `BudgetLimited`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// The goal is actively being pursued.
    Active,
    /// Paused by the user. Resumable.
    Paused,
    /// Blocked by a persistent error or 3+ consecutive turns with the same blocker.
    Blocked,
    /// API usage limit reached. The agent should wait.
    UsageLimited,
    /// Token budget exceeded. Terminal.
    BudgetLimited,
    /// The goal has been completed successfully. Terminal.
    Complete,
}

impl GoalStatus {
    /// Returns `true` if this status is terminal (no further work possible).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::BudgetLimited)
    }

    /// Returns `true` if the goal should be auto-continued.
    pub fn should_auto_continue(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns a short slug for the status.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
        }
    }
}

impl std::fmt::Display for GoalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A persistent goal associated with a session.
///
/// The goal guides the agent across multiple turns, with budget tracking,
/// auto-continuation, and steering prompt injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGoal {
    /// The session this goal belongs to.
    pub session_id: String,
    /// Unique identifier for this goal.
    pub goal_id: GoalId,
    /// The textual objective.
    pub objective: String,
    /// Current status in the state machine.
    pub status: GoalStatus,
    /// Optional token budget. When exceeded, transitions to `BudgetLimited`.
    pub token_budget: Option<i64>,
    /// Tokens consumed so far under this goal.
    pub tokens_used: i64,
    /// Wall-clock seconds elapsed while the goal was active.
    pub time_used_seconds: i64,
    /// Count of consecutive turns blocked with the same reason.
    pub consecutive_blocked_turns: u32,
    /// The reason for the current block (if any).
    pub block_reason: Option<String>,
    /// When the goal was created (Unix timestamp seconds).
    pub created_at: u64,
    /// When the goal was last updated (Unix timestamp seconds).
    pub updated_at: u64,
}

impl SessionGoal {
    /// Creates a new active goal with the given objective.
    pub fn new(session_id: String, objective: String, token_budget: Option<i64>) -> Self {
        let now = crate::session::current_unix_timestamp();
        Self {
            session_id,
            goal_id: GoalId::new(),
            objective,
            status: GoalStatus::Active,
            token_budget,
            tokens_used: 0,
            time_used_seconds: 0,
            consecutive_blocked_turns: 0,
            block_reason: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Returns the remaining token budget, or `None` if no budget is set.
    pub fn remaining_budget(&self) -> Option<i64> {
        self.token_budget
            .map(|budget| budget.saturating_sub(self.tokens_used))
    }

    /// Returns `true` if the token budget has been exceeded.
    pub fn is_budget_exceeded(&self) -> bool {
        self.token_budget
            .map(|budget| self.tokens_used >= budget)
            .unwrap_or(false)
    }

    /// Records token usage and checks budget.
    /// Returns `true` if the budget was exceeded by this update.
    pub fn record_tokens(&mut self, delta: i64) -> bool {
        self.tokens_used = self.tokens_used.saturating_add(delta);
        self.updated_at = crate::session::current_unix_timestamp();
        if self.is_budget_exceeded() {
            self.status = GoalStatus::BudgetLimited;
            true
        } else {
            false
        }
    }

    /// Records elapsed wall-clock time.
    pub fn record_time(&mut self, seconds: i64) {
        self.time_used_seconds = self.time_used_seconds.saturating_add(seconds);
        self.updated_at = crate::session::current_unix_timestamp();
    }

    /// Transitions the goal to a new status.
    pub fn transition_to(&mut self, status: GoalStatus) {
        self.status = status;
        self.updated_at = crate::session::current_unix_timestamp();
    }

    /// Records a blocked turn. Returns `true` if the goal should transition to Blocked.
    pub fn record_blocked_turn(&mut self, reason: &str) -> bool {
        if self.block_reason.as_deref() == Some(reason) {
            self.consecutive_blocked_turns += 1;
        } else {
            self.consecutive_blocked_turns = 1;
            self.block_reason = Some(reason.to_string());
        }
        self.updated_at = crate::session::current_unix_timestamp();

        if self.consecutive_blocked_turns >= 3 {
            self.status = GoalStatus::Blocked;
            true
        } else {
            false
        }
    }

    /// Returns a progress snapshot suitable for display.
    pub fn progress_snapshot(&self) -> GoalProgress {
        GoalProgress {
            goal_id: self.goal_id.clone(),
            objective: self.objective.clone(),
            status: self.status,
            tokens_used: self.tokens_used,
            token_budget: self.token_budget,
            remaining_budget: self.remaining_budget(),
            time_used_seconds: self.time_used_seconds,
            consecutive_blocked_turns: self.consecutive_blocked_turns,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// A read-only snapshot of goal progress for display and persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgress {
    pub goal_id: GoalId,
    pub objective: String,
    pub status: GoalStatus,
    pub tokens_used: i64,
    pub token_budget: Option<i64>,
    pub remaining_budget: Option<i64>,
    pub time_used_seconds: i64,
    pub consecutive_blocked_turns: u32,
    pub created_at: u64,
    pub updated_at: u64,
}
