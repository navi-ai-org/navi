use crate::goal::types::SessionGoal;
use std::sync::{
    Mutex,
    atomic::{AtomicI64, AtomicU64, Ordering},
};

/// Tracks token and time consumption for an active goal during a turn.
pub struct GoalAccountingState {
    /// Tokens consumed so far during the current turn.
    tokens_this_turn: AtomicI64,
    /// Unix timestamp when the current turn started (seconds).
    turn_start_time: AtomicU64,
    /// The goal being tracked, protected by a mutex for status transitions.
    goal: Mutex<SessionGoal>,
}

impl GoalAccountingState {
    /// Creates a new accounting state for the given goal.
    pub fn new(goal: SessionGoal) -> Self {
        Self {
            tokens_this_turn: AtomicI64::new(0),
            turn_start_time: AtomicU64::new(0),
            goal: Mutex::new(goal),
        }
    }

    /// Starts a new turn: records the start time and resets the turn token counter.
    pub fn start_turn(&self) {
        let now = crate::session::current_unix_timestamp();
        self.turn_start_time.store(now, Ordering::SeqCst);
        self.tokens_this_turn.store(0, Ordering::SeqCst);
    }

    /// Records token usage during a turn. Returns `true` if the budget was exceeded.
    pub fn record_token_usage(&self, delta: i64) -> bool {
        self.tokens_this_turn.fetch_add(delta, Ordering::SeqCst);
        let mut goal = self.goal.lock().unwrap_or_else(|e| e.into_inner());
        goal.record_tokens(delta)
    }

    /// Finishes the turn: records elapsed time and returns a snapshot.
    pub fn finish_turn(&self) -> Option<SessionGoal> {
        let turn_start = self.turn_start_time.load(Ordering::SeqCst);
        if turn_start > 0 {
            let now = crate::session::current_unix_timestamp();
            let seconds = (now.saturating_sub(turn_start)) as i64;
            let mut goal = self.goal.lock().unwrap_or_else(|e| e.into_inner());
            goal.record_time(seconds);
        }
        self.turn_start_time.store(0, Ordering::SeqCst);
        self.tokens_this_turn.store(0, Ordering::SeqCst);
        self.snapshot()
    }

    /// Returns a clone of the current goal state.
    pub fn snapshot(&self) -> Option<SessionGoal> {
        let goal = self.goal.lock().unwrap_or_else(|e| e.into_inner());
        Some(goal.clone())
    }

    /// Returns `true` if the goal is active (not terminal, not paused).
    pub fn is_active(&self) -> bool {
        let goal = self.goal.lock().unwrap_or_else(|e| e.into_inner());
        goal.status.should_auto_continue()
    }
}
