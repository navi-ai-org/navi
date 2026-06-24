use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default lock lease TTL: 5 minutes.
const DEFAULT_LEASE_TTL_SECS: u64 = 300;

/// Information about an active file lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLockInfo {
    /// The absolute path of the locked file.
    pub path: PathBuf,
    /// Session id that holds the lock.
    pub session_id: String,
    /// Unix timestamp when the lock was acquired.
    pub locked_at: u64,
    /// Unix timestamp after which the lock is considered stale.
    pub locked_until: u64,
    /// A machine-readable identifier for the instance holding the lock
    /// (e.g. `hostname-pid`).
    pub instance_id: String,
}

/// Manages file-level locks using the filesystem as the coordination primitive.
///
/// Locks are stored as individual files under `<project_root>/.navi/file_locks/`.
/// Each lock file is named after a deterministic hash of the locked file's
/// absolute path, making the scheme cross-instance and cross-process.
///
/// Lock acquisition uses `File::create_new()` (O_CREAT | O_EXCL on Unix),
/// ensuring atomic, race-free locking without a central coordinator.
pub struct FileLockManager {
    /// Project root used to normalize project-relative lock targets.
    project_root: PathBuf,
    /// Directory where lock files are stored: `<project_root>/.navi/file_locks/`
    locks_dir: PathBuf,
    /// Identifier for this instance (e.g. `hostname-pid`).
    instance_id: String,
    /// Current hostname portion of instance_id, used for stale detection.
    hostname: String,
    /// Current session ID, set after session starts.
    session_id: RwLock<String>,
    /// Lease TTL in seconds. After this time, the lock can be reclaimed.
    lease_ttl_secs: u64,
}

/// A RAII guard that releases the lock when dropped.
///
/// If the guard is dropped without calling [`release`](Self::release), the lock
/// file is removed automatically.
#[derive(Debug)]
pub struct LockGuard {
    lock_path: Option<PathBuf>,
    target_path: PathBuf,
}

impl LockGuard {
    /// Returns the path of the locked file.
    pub fn target_path(&self) -> &Path {
        &self.target_path
    }

    /// Releases the lock immediately. This is idempotent: calling it multiple
    /// times is safe.
    pub fn release(mut self) -> std::io::Result<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> std::io::Result<()> {
        if let Some(path) = self.lock_path.take() {
            let _ = fs::remove_file(&path);
        }
        Ok(())
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.lock_path.is_some() {
            let _ = self.release_inner();
        }
    }
}

impl FileLockManager {
    /// Creates a new lock manager.
    ///
    /// The `locks_dir` is typically `<project_root>/.navi/file_locks/`. The
    /// method creates the directory if it does not exist.
    pub fn new(project_root: &Path, instance_id: String) -> std::io::Result<Self> {
        let locks_dir = project_root.join(".navi").join("file_locks");
        fs::create_dir_all(&locks_dir)?;
        let hostname = instance_id
            .rsplit_once('-')
            .map(|(h, _)| h.to_string())
            .unwrap_or_else(|| instance_id.clone());
        Ok(Self {
            project_root: project_root.to_path_buf(),
            locks_dir,
            instance_id,
            hostname,
            session_id: RwLock::new(String::new()),
            lease_ttl_secs: DEFAULT_LEASE_TTL_SECS,
        })
    }

    /// Sets a custom lease TTL (in seconds). Default is 300 (5 minutes).
    pub fn set_lease_ttl(&mut self, ttl_secs: u64) {
        self.lease_ttl_secs = ttl_secs;
    }

    /// Returns the path to the lock directory.
    pub fn locks_dir(&self) -> &Path {
        &self.locks_dir
    }

    /// Sets the current session ID for lock metadata.
    pub fn set_session_id(&self, session_id: &str) {
        if let Ok(mut guard) = self.session_id.write() {
            *guard = session_id.to_string();
        }
    }

    /// Returns the current session ID.
    pub fn get_session_id(&self) -> String {
        self.session_id
            .read()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Attempts to acquire a lock for the given file path.
    ///
    /// If the file is locked by a stale instance (lease expired or PID dead),
    /// the stale lock is removed first and the lock is re-acquired.
    ///
    /// Returns `Ok(Some(LockGuard))` if the lock was acquired, `Ok(None)` if
    /// the file is locked by an active instance, or `Err` on I/O error.
    pub fn try_lock(&self, path: &Path) -> std::io::Result<Option<LockGuard>> {
        let lock_path = self.lock_path_for(path);

        // If a lock exists, check if it's stale.
        if lock_path.exists() {
            match self.is_locked(path) {
                Ok(Some(_info)) => {
                    // Lock is held by an active instance — can't acquire.
                    return Ok(None);
                }
                Ok(None) => {
                    // Lock was stale and has been cleaned up by is_locked.
                    // Fall through to create the new lock below.
                }
                Err(e) => {
                    // Can't read the lock file; try to continue.
                    tracing::warn!(error = %e, "failed to check stale lock, attempting fresh lock");
                    // Clean up the potentially corrupt lock file so we can proceed.
                    let _ = fs::remove_file(&lock_path);
                }
            }
        }

        // Atomically create the lock file (O_CREAT | O_EXCL).
        let file = match fs::File::create_new(&lock_path) {
            Ok(file) => file,
            // Race: another instance created the lock between our check and now.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(None),
            Err(e) => return Err(e),
        };

        let now = unix_now();
        let session_id = self.get_session_id();

        let info = FileLockInfo {
            path: path.to_path_buf(),
            session_id,
            locked_at: now,
            locked_until: now + self.lease_ttl_secs,
            instance_id: self.instance_id.clone(),
        };
        let json = serde_json::to_string(&info)?;
        let mut writer = std::io::BufWriter::new(file);
        writer.write_all(json.as_bytes())?;
        writer.flush()?;

        Ok(Some(LockGuard {
            lock_path: Some(lock_path),
            target_path: path.to_path_buf(),
        }))
    }

    /// Checks whether the given file is currently locked.
    ///
    /// If the lock file exists but is stale (lease expired OR PID is dead),
    /// it is removed automatically and `Ok(None)` is returned.
    ///
    /// Returns `Ok(Some(FileLockInfo))` if actively locked, `Ok(None)` if
    /// free (or stale and cleaned), or `Err` on I/O error.
    pub fn is_locked(&self, path: &Path) -> std::io::Result<Option<FileLockInfo>> {
        let lock_path = self.lock_path_for(path);
        if !lock_path.exists() {
            return Ok(None);
        }
        let content = match fs::read_to_string(&lock_path) {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let info: FileLockInfo = match serde_json::from_str(&content) {
            Ok(i) => i,
            Err(_) => {
                // Corrupt lock file — remove it.
                let _ = fs::remove_file(&lock_path);
                return Ok(None);
            }
        };

        if self.is_stale(&info) {
            tracing::info!(
                path = %info.path.display(),
                instance = %info.instance_id,
                locked_until = info.locked_until,
                "removing stale file lock"
            );
            let _ = fs::remove_file(&lock_path);
            return Ok(None);
        }

        Ok(Some(info))
    }

    /// Returns `true` if the lock holder is no longer alive.
    fn is_stale(&self, info: &FileLockInfo) -> bool {
        // 1. Check lease TTL expiry.
        if unix_now() >= info.locked_until {
            return true;
        }

        // The current process owns this lock and the lease is still valid.
        // Do not classify it as stale based on a test/fallback instance id
        // that happens to end with a non-live PID-like suffix.
        if info.instance_id == self.instance_id {
            return false;
        }

        // 2. If the lock is from the *current host*, check if the PID is alive.
        //    Instance ID format: `hostname-pid` (e.g. "myhost-1234").
        if let Some(pid_str) = info
            .instance_id
            .strip_prefix(&format!("{}-", self.hostname))
        {
            if let Ok(pid) = pid_str.parse::<u32>() {
                if !is_pid_alive(pid) {
                    return true;
                }
            }
        }

        // Lock is still valid.
        false
    }

    /// Blocks until the lock for the given file is released, or until the
    /// timeout expires.
    ///
    /// Returns `Ok(Some(FileLockInfo))` with the last known lock holder info,
    /// or `Ok(None)` if the file was never locked or was released.
    pub fn wait_for_unlock(
        &self,
        path: &Path,
        timeout: Duration,
        poll_interval: Duration,
    ) -> std::io::Result<Option<FileLockInfo>> {
        let start = std::time::Instant::now();

        let poll_us = poll_interval.as_micros() as u64;
        let timeout_us = timeout.as_micros() as u64;

        loop {
            if start.elapsed() >= timeout {
                // Return current state (stale detection auto-cleans if needed).
                return self.is_locked(path);
            }

            // is_locked handles stale detection and auto-removal.
            if self.is_locked(path)?.is_none() {
                return Ok(None);
            }

            std::thread::sleep(Duration::from_micros(
                poll_us.min(timeout_us.saturating_sub(start.elapsed().as_micros() as u64)),
            ));
        }
    }

    /// Cleans up all stale lock files in the lock directory.
    ///
    /// Returns the number of stale locks removed.
    pub fn cleanup_stale_locks(&self) -> std::io::Result<usize> {
        let mut removed = 0;
        if !self.locks_dir.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(&self.locks_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lock") {
                continue;
            }
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let info: FileLockInfo = match serde_json::from_str(&content) {
                Ok(i) => i,
                Err(_) => {
                    // Corrupt file — remove it.
                    let _ = fs::remove_file(&path);
                    removed += 1;
                    continue;
                }
            };
            if self.is_stale(&info) {
                let _ = fs::remove_file(&path);
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Computes the lock file path for a given target path using a
    /// deterministic hash (DJB2).
    fn lock_path_for(&self, path: &Path) -> PathBuf {
        let normalized = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        };
        let hash = djb2_hash(normalized.to_string_lossy().as_bytes());
        self.locks_dir.join(format!("{hash:016x}.lock"))
    }
}

/// A simple, cross-platform DJB2 hash for deterministic lock filenames.
fn djb2_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 5381;
    for &b in bytes {
        hash = hash.wrapping_mul(33).wrapping_add(b as u64);
    }
    hash
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Check if a process with the given PID is alive.
///
/// On Linux: checks `/proc/{pid}/` existence.
/// On other Unix: sends signal 0 via `kill`.
/// On unsupported platforms: assumes alive (false negative is safer than false positive).
fn is_pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(unix)]
        {
            std::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(true)
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_lock_and_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let project_root = dir.path();
        let target = project_root.join("test.rs");
        let manager = FileLockManager::new(project_root, "test-instance".into()).unwrap();
        manager.set_session_id("session-1");

        assert!(manager.is_locked(&target).unwrap().is_none());

        let guard = manager
            .try_lock(&target)
            .unwrap()
            .expect("should acquire lock");
        assert!(manager.is_locked(&target).unwrap().is_some());

        let second = manager.try_lock(&target).unwrap();
        assert!(second.is_none());

        drop(guard);
        assert!(manager.is_locked(&target).unwrap().is_none());
    }

    #[test]
    fn test_guard_drop_releases_lock() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("drop_test.rs");
        let manager = FileLockManager::new(dir.path(), "test".into()).unwrap();
        manager.set_session_id("s1");

        let guard = manager.try_lock(&target).unwrap().unwrap();
        drop(guard);
        assert!(manager.is_locked(&target).unwrap().is_none());
    }

    #[test]
    fn test_lock_path_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let manager = FileLockManager::new(dir.path(), "test".into()).unwrap();
        let path = Path::new("/some/project/file.rs");

        let h1 = manager.lock_path_for(path);
        let h2 = manager.lock_path_for(path);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_wait_for_unlock_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("timeout_test.rs");
        let manager = FileLockManager::new(dir.path(), "test".into()).unwrap();
        manager.set_session_id("s1");

        let guard = manager.try_lock(&target).unwrap().unwrap();
        std::mem::forget(guard); // never release — but lease will expire

        // With a very short lease, the lock becomes stale quickly.
        let result = manager
            .wait_for_unlock(
                &target,
                Duration::from_millis(200),
                Duration::from_millis(50),
            )
            .unwrap();
        // After timeout, the lease hasn't expired yet (DEFAULT_LEASE_TTL_SECS=300).
        // So it should return Some (lock still held).
        assert!(result.is_some());
    }

    #[test]
    fn test_stale_lock_cleaned_by_lease_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("stale_lease.rs");
        let mut manager = FileLockManager::new(dir.path(), "test".into()).unwrap();
        manager.set_session_id("s1");
        // Set a very short lease (1 second).
        manager.set_lease_ttl(1);

        let guard = manager.try_lock(&target).unwrap().unwrap();
        let lock_path = dir.path().join(".navi/file_locks").join(format!(
            "{:016x}.lock",
            djb2_hash(target.to_string_lossy().as_bytes())
        ));
        drop(guard);

        // Re-create a lock manually with an expired lease.
        let expired_info = FileLockInfo {
            path: target.clone(),
            session_id: "dead-session".into(),
            locked_at: 1000,
            locked_until: 1, // expired
            instance_id: "dead-instance".into(),
        };
        fs::write(&lock_path, serde_json::to_string(&expired_info).unwrap()).unwrap();

        // is_locked should detect the stale lease and remove the lock file.
        assert!(manager.is_locked(&target).unwrap().is_none());
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_stale_lock_cleaned_by_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("stale_pid.rs");
        let manager = FileLockManager::new(dir.path(), "stale-check-999999".into()).unwrap();
        manager.set_session_id("s1");

        let lock_path = dir.path().join(".navi/file_locks").join(format!(
            "{:016x}.lock",
            djb2_hash(target.to_string_lossy().as_bytes())
        ));

        // PID 1 (init) is always alive, PID 99999999 almost certainly dead.
        let stale_info = FileLockInfo {
            path: target.clone(),
            session_id: "dead-session".into(),
            locked_at: unix_now(),
            locked_until: unix_now() + 3600, // lease valid for 1h
            instance_id: "stale-check-99999999".into(), // PID 99999999
        };
        fs::write(&lock_path, serde_json::to_string(&stale_info).unwrap()).unwrap();

        // is_locked should detect the dead PID and remove the lock.
        let result = manager.is_locked(&target).unwrap();
        assert!(
            result.is_none(),
            "stale lock with dead PID should be removed"
        );
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_cleanup_stale_locks() {
        let dir = tempfile::tempdir().unwrap();
        let manager = FileLockManager::new(dir.path(), "cleanup-test-99999999".into()).unwrap();
        manager.set_session_id("s1");

        // Create a live lock.
        let live = dir.path().join("live_file.rs");
        let guard = manager.try_lock(&live).unwrap().unwrap();

        // Create a stale lock manually.
        let stale = dir.path().join("stale_file.rs");
        let stale_lock = manager.lock_path_for(&stale);
        let stale_info = FileLockInfo {
            path: stale.clone(),
            session_id: "dead".into(),
            locked_at: 1000,
            locked_until: 1, // expired
            instance_id: "cleanup-test-99999999".into(),
        };
        fs::write(&stale_lock, serde_json::to_string(&stale_info).unwrap()).unwrap();

        // cleanup_stale_locks should remove only the stale one.
        let removed = manager.cleanup_stale_locks().unwrap();
        assert_eq!(removed, 1, "one stale lock should be removed");
        assert!(!stale_lock.exists(), "stale lock file should be gone");

        // Live lock is still held — file should still exist.
        let live_lock = manager.lock_path_for(&live);
        assert!(live_lock.exists(), "live lock should remain");

        drop(guard);
    }

    #[test]
    fn test_cross_instance_deterministic_hash() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let mgr1 = FileLockManager::new(dir1.path(), "inst-a".into()).unwrap();
        let mgr2 = FileLockManager::new(dir2.path(), "inst-b".into()).unwrap();

        let target = Path::new("/shared/project/main.rs");
        let lock1 = mgr1.lock_path_for(target);
        let lock2 = mgr2.lock_path_for(target);
        assert_eq!(lock1.file_name(), lock2.file_name());
    }

    #[test]
    fn test_stale_lock_reacquire() {
        let dir = tempfile::tempdir().unwrap();
        let manager = FileLockManager::new(dir.path(), "reacquire-test-99999998".into()).unwrap();
        manager.set_session_id("s2");

        let target = dir.path().join("reacquire.rs");
        let lock_path = manager.lock_path_for(&target);

        // Create a stale lock manually (dead PID + expired lease).
        let stale_info = FileLockInfo {
            path: target.clone(),
            session_id: "stale-session".into(),
            locked_at: 1000,
            locked_until: 1, // expired
            instance_id: "reacquire-test-99999998".into(),
        };
        fs::write(&lock_path, serde_json::to_string(&stale_info).unwrap()).unwrap();

        // try_lock should clean the stale lock and acquire a fresh one.
        let guard = manager
            .try_lock(&target)
            .unwrap()
            .expect("should reacquire stale lock");
        assert!(manager.is_locked(&target).unwrap().is_some());
        drop(guard);
    }
}
