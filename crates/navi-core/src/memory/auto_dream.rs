//! Auto-dream: automatic memory consolidation triggered after turns.
//!
//! After each completed turn,
//! a 3-gate check decides whether to run a dream consolidation pass.
//!
//! ## Gates
//!
//! | Gate      | Condition                        | How                          |
//! |-----------|----------------------------------|------------------------------|
//! | Time      | >= 24h since last dream           | Read `last_dream_at` file    |
//! | Sessions  | >= 5 sessions since last dream    | Count modified transcripts    |
//! | Lock      | No other dream in progress        | Atomic lock file              |
//!
//! When all 3 gates pass, a dream consolidation is spawned in background
//! (tokio task) so the TUI/agent is not blocked.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimum hours between automatic dream runs.
const MIN_DREAM_INTERVAL_HOURS: u64 = 24;

/// Minimum number of sessions touched since last dream.
const MIN_SESSIONS_SINCE_DREAM: usize = 5;

/// File name for persisting the last dream timestamp.
const LAST_DREAM_FILE: &str = "last_dream_at";

/// File name for the dream lock (prevents concurrent dreams).
const DREAM_LOCK_FILE: &str = "dream.lock";

/// In-process locks held by path, preventing overlapping dream/distill runs
/// within the same NAVI process. Keyed by the lock file's parent directory
/// so that dream and distill (which use different directories) can run
/// concurrently.
static HELD_LOCKS: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

fn is_lock_held(lock_path: &Path) -> bool {
    // Recover from poison: lock-tracking state is non-critical; prefer progress.
    let mut guard = HELD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    set.contains(lock_path)
}

fn mark_lock_held(lock_path: PathBuf) {
    let mut guard = HELD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    set.insert(lock_path);
}

fn mark_lock_released(lock_path: &Path) {
    let mut guard = HELD_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = guard.as_mut() {
        set.remove(lock_path);
    }
}

/// State for the auto-dream gate check.
#[derive(Debug, Clone)]
pub struct AutoDreamState {
    pub memory_root: PathBuf,
    pub dream_interval_hours: u64,
    pub min_sessions: usize,
}

impl AutoDreamState {
    pub fn new(memory_root: PathBuf) -> Self {
        Self {
            memory_root,
            dream_interval_hours: MIN_DREAM_INTERVAL_HOURS,
            min_sessions: MIN_SESSIONS_SINCE_DREAM,
        }
    }

    pub fn with_interval(mut self, hours: u64) -> Self {
        self.dream_interval_hours = hours;
        self
    }

    pub fn with_min_sessions(mut self, count: usize) -> Self {
        self.min_sessions = count;
        self
    }

    /// Returns the path to the `last_dream_at` file.
    fn last_dream_path(&self) -> PathBuf {
        self.memory_root.join(LAST_DREAM_FILE)
    }

    /// Returns the path to the dream lock file.
    fn lock_path(&self) -> PathBuf {
        self.memory_root.join(DREAM_LOCK_FILE)
    }

    /// Reads the last dream timestamp from disk (unix seconds).
    /// Returns 0 if the file doesn't exist (never dreamed).
    pub fn read_last_dream_at(&self) -> u64 {
        match fs::read_to_string(self.last_dream_path()) {
            Ok(content) => content.trim().parse().unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Writes the current timestamp as the last dream time.
    fn write_last_dream_at(&self) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(parent) = self.last_dream_path().parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "Failed to create memory root for last_dream_at: {:?}",
                    parent
                )
            })?;
        }
        fs::write(self.last_dream_path(), now.to_string()).with_context(|| {
            format!(
                "Failed to write last_dream_at to {:?}",
                self.last_dream_path()
            )
        })?;
        Ok(())
    }

    /// Counts sessions in the history store that were modified since the last dream.
    fn count_sessions_since_last_dream(&self, history: &super::HistoryStore) -> usize {
        let last_dream = self.read_last_dream_at();
        let sessions = history.list_sessions().unwrap_or_default();
        if last_dream == 0 {
            // Never dreamed — count all sessions
            return sessions.len();
        }
        // Count sessions started after the last dream.
        // Convert last_dream (unix seconds) to ISO for comparison.
        let last_dream_iso = super::auto_memory::now_iso();
        sessions
            .iter()
            .filter(|s| {
                // ISO 8601 strings sort lexicographically
                s.started_at.as_str() > last_dream_iso.as_str() || s.started_at.is_empty()
            })
            .count()
    }

    /// Attempts to acquire the dream lock (file-based, cross-process).
    /// Returns true if the lock was acquired.
    fn try_acquire_lock(&self) -> bool {
        let lock_path = self.lock_path();

        // Check in-process flag first (keyed by path)
        if is_lock_held(&lock_path) {
            return false;
        }

        // Try to create the lock file exclusively
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => {
                mark_lock_held(lock_path.clone());
                // Write a marker with the current time
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let _ = fs::write(&lock_path, now.to_string());
                true
            }
            Err(_) => {
                // Lock file exists — check if it's stale (> 1 hour old)
                if let Ok(content) = fs::read_to_string(&lock_path)
                    && let Ok(lock_time) = content.trim().parse::<u64>()
                {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if now.saturating_sub(lock_time) > 3600 {
                        // Stale lock — steal it
                        let _ = fs::remove_file(&lock_path);
                        if fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&lock_path)
                            .is_ok()
                        {
                            mark_lock_held(lock_path.clone());
                            let _ = fs::write(&lock_path, now.to_string());
                            return true;
                        }
                    }
                }
                false
            }
        }
    }

    /// Releases the dream lock.
    fn release_lock(&self) {
        let lock_path = self.lock_path();
        mark_lock_released(&lock_path);
        let _ = fs::remove_file(&lock_path);
    }

    /// Runs the 3-gate check and returns true if a dream should be triggered.
    ///
    /// Gate 1 — Time: at least `dream_interval_hours` since last dream.
    /// Gate 2 — Sessions: at least `min_sessions` touched since last dream.
    /// Gate 3 — Lock: no other dream in progress.
    pub fn should_dream(&self, history: &super::HistoryStore) -> bool {
        // Gate 1: Time
        let last_dream = self.read_last_dream_at();
        if last_dream > 0 {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let elapsed_hours = now.saturating_sub(last_dream) / 3600;
            if elapsed_hours < self.dream_interval_hours {
                return false;
            }
        }

        // Gate 2: Sessions
        let session_count = self.count_sessions_since_last_dream(history);
        if session_count < self.min_sessions {
            return false;
        }

        // Gate 3: Lock
        if !self.try_acquire_lock() {
            tracing::debug!("auto-dream skipped: lock held by another process");
            return false;
        }

        true
    }

    /// Marks the dream as completed and releases the lock.
    pub fn mark_completed(&self) {
        if let Err(e) = self.write_last_dream_at() {
            tracing::warn!("Failed to write last_dream_at: {}", e);
        }
        self.release_lock();
    }

    /// Releases the lock without marking completion (for error paths).
    pub fn release(&self) {
        self.release_lock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_state(dir: &Path) -> AutoDreamState {
        AutoDreamState::new(dir.to_path_buf())
    }

    #[test]
    fn test_last_dream_persistence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = test_state(tmp.path());

        // Never dreamed — returns 0
        assert_eq!(state.read_last_dream_at(), 0);

        // Write timestamp
        state.write_last_dream_at().expect("write");
        let first = state.read_last_dream_at();
        assert!(first > 0);

        // Read again — should be the same (within the same second)
        assert_eq!(state.read_last_dream_at(), first);
    }

    #[test]
    fn test_lock_acquire_and_release() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = test_state(tmp.path());

        // First acquire — should succeed
        assert!(state.try_acquire_lock());
        assert!(state.lock_path().exists());

        // Second acquire — should fail (in-process flag)
        assert!(!state.try_acquire_lock());

        // Release
        state.release_lock();
        assert!(!state.lock_path().exists());

        // Acquire again — should succeed
        assert!(state.try_acquire_lock());
        state.release_lock();
    }

    #[test]
    fn test_should_dream_never_dreamed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = test_state(tmp.path());
        let db_path = tmp.path().join("history.db");
        let history = super::super::HistoryStore::new(&db_path).expect("history");

        // Never dreamed, but 0 sessions — should not dream (Gate 2 fails)
        assert!(!state.should_dream(&history));
    }

    #[test]
    fn test_mark_completed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let state = test_state(tmp.path());

        // Acquire lock
        assert!(state.try_acquire_lock());

        // Mark completed — should release lock and write timestamp
        state.mark_completed();
        assert!(!state.lock_path().exists());
        assert!(state.read_last_dream_at() > 0);
    }
}
