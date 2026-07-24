use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SNAPSHOT_FILE_CONTENT_BYTES: u64 = 1_000_000; // 1MB

// ── SnapshotEntry ──────────────────────────────────────────────────────────

/// A single file entry within a workspace snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotEntry {
    /// Absolute path to the file at snapshot time.
    pub path: PathBuf,
    /// SHA-256 hex digest of the file content.
    pub hash: String,
    /// Full file content, or `None` for files > 1 MB.
    pub content: Option<String>,
}

// ── WorkspaceSnapshot ──────────────────────────────────────────────────────

/// Pre-execution snapshot of file state for a workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceSnapshot {
    /// Machine-readable identifier (e.g. `snap_1748300000`).
    pub id: String,
    /// Individual file entries captured at snapshot time.
    pub entries: Vec<SnapshotEntry>,
    /// The original root paths that were scanned. Used to discover new files
    /// when computing changes or rolling back.
    pub roots: Vec<PathBuf>,
    /// Unix timestamp (seconds since epoch) when the snapshot was created.
    pub created_at: u64,
}

impl WorkspaceSnapshot {
    /// Returns `true` if `path` was one of the snapshotted entries.
    pub fn is_path_in_entry(&self, path: &Path) -> bool {
        self.entries.iter().any(|e| e.path == path)
    }
}

// ── ChangeSet ──────────────────────────────────────────────────────────────

/// Records what actually changed between a snapshot and the current filesystem.
#[derive(Debug, Clone)]
pub struct ChangeSet {
    /// Files that exist on disk but were not present in the snapshot.
    pub files_created: Vec<PathBuf>,
    /// Files that were in the snapshot but have different content now.
    pub files_modified: Vec<PathBuf>,
    /// Files that were in the snapshot but no longer exist on disk.
    pub files_deleted: Vec<PathBuf>,
    /// Optional unified diff of all changes (requires diff crate; `None` for MVP).
    pub diff: Option<String>,
}

impl ChangeSet {
    /// Returns `true` when no changes were detected.
    pub fn is_empty(&self) -> bool {
        self.files_created.is_empty()
            && self.files_modified.is_empty()
            && self.files_deleted.is_empty()
    }

    /// Total number of changed files.
    pub fn total(&self) -> usize {
        self.files_created.len() + self.files_modified.len() + self.files_deleted.len()
    }
}

// ── Sandbox Manager ────────────────────────────────────────────────────────

/// Low-level sandbox operations: create snapshots, compute changes, and roll
/// back file state.
pub struct SandboxManager;

impl SandboxManager {
    /// Creates a snapshot of the given paths (files and directories).
    ///
    /// Directories are walked recursively (respecting common ignore patterns).
    /// Files are hashed with SHA-256; content is stored for files <= 1 MB.
    pub fn create_snapshot(paths: &[PathBuf]) -> WorkspaceSnapshot {
        let mut entries = Vec::new();
        let mut visited = BTreeSet::new();
        let mut root_set = BTreeSet::new();

        for p in paths {
            if p.is_dir() {
                root_set.insert(p.canonicalize().unwrap_or_else(|_| p.clone()));
                collect_files(p, &mut entries, &mut visited);
            } else if p.is_file() {
                let snapshot_path = p.canonicalize().unwrap_or_else(|_| p.clone());
                if let Some(parent) = snapshot_path.parent() {
                    root_set.insert(parent.to_path_buf());
                }
                if visited.insert(snapshot_path.clone()) {
                    entries.push(snapshot_file(&snapshot_path));
                }
            } else {
                let parent = p.parent().unwrap_or(Path::new("."));
                if let Ok(canon_parent) = parent.canonicalize() {
                    root_set.insert(canon_parent);
                } else {
                    root_set.insert(parent.to_path_buf());
                }
            }
        }

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let id = format!("snap_{}", created_at);

        WorkspaceSnapshot {
            id,
            entries,
            roots: root_set.into_iter().collect(),
            created_at,
        }
    }

    /// Compares the current filesystem state against a snapshot and returns a
    /// `ChangeSet` describing what was added, modified, or deleted.
    pub fn compute_changes(snapshot: &WorkspaceSnapshot) -> ChangeSet {
        let mut files_created = Vec::new();
        let mut files_modified = Vec::new();
        let mut files_deleted = Vec::new();

        // Build a set of paths that were in the snapshot.
        let entry_paths: HashSet<&PathBuf> = snapshot.entries.iter().map(|e| &e.path).collect();

        // Check every snapshot entry for modification or deletion.
        for entry in &snapshot.entries {
            if !entry.path.exists() {
                files_deleted.push(entry.path.clone());
            } else if entry.path.is_file()
                && let Ok(current) = hash_file(&entry.path)
                && current != entry.hash
            {
                files_modified.push(entry.path.clone());
            }
        }

        // Re-scan root directories to discover newly-created files.
        for root in &snapshot.roots {
            if root.is_dir() {
                find_new_files(root, &entry_paths, &mut files_created);
            }
        }

        ChangeSet {
            files_created,
            files_modified,
            files_deleted,
            diff: None,
        }
    }

    /// Rolls the workspace back to the state captured in `snapshot`.
    ///
    /// * Files that were in the snapshot but are now missing or modified are
    ///   restored from the stored content.
    /// * Files that exist on disk but were NOT in the snapshot are deleted.
    ///
    /// Returns an error when a file that needs restoration has no stored
    /// content (i.e. it was larger than 1 MB).
    pub fn rollback(snapshot: &WorkspaceSnapshot) -> Result<(), String> {
        // Phase 1: restore every entry that is missing or modified.
        for entry in &snapshot.entries {
            let needs_restore = if !entry.path.exists() {
                true
            } else if entry.path.is_file() {
                hash_file(&entry.path)
                    .map(|h| h != entry.hash)
                    .unwrap_or(false)
            } else {
                false
            };

            if needs_restore {
                let content = entry.content.as_ref().ok_or_else(|| {
                    format!(
                        "cannot restore `{}`: content not available (file > 1 MB)",
                        entry.path.display()
                    )
                })?;
                if let Some(parent) = entry.path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        format!(
                            "failed to create parent directory `{}`: {e}",
                            parent.display()
                        )
                    })?;
                }
                std::fs::write(&entry.path, content)
                    .map_err(|e| format!("failed to restore `{}`: {e}", entry.path.display()))?;
            }
        }

        // Phase 2: delete files that were created after the snapshot.
        let entry_paths: HashSet<&PathBuf> = snapshot.entries.iter().map(|e| &e.path).collect();
        for root in &snapshot.roots {
            if root.is_dir() {
                delete_new_files(root, &entry_paths)?;
            }
        }

        Ok(())
    }

    /// Checks whether the git index has staged (uncommitted) changes.
    ///
    /// Runs `git diff --cached --quiet` and returns `false` if git is not
    /// available or the command fails.
    pub fn has_staged_changes() -> bool {
        let output = std::process::Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match output {
            Ok(status) => !status.success(),
            Err(_) => false,
        }
    }
}

// ── Private helpers ────────────────────────────────────────────────────────

/// Compute SHA-256 hex digest for a file's contents.
fn hash_file(path: &Path) -> Result<String, String> {
    let data =
        std::fs::read(path).map_err(|e| format!("failed to read `{}`: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(hex::encode(hasher.finalize()))
}

/// Build a `SnapshotEntry` for a single file at `path`.
fn snapshot_file(path: &Path) -> SnapshotEntry {
    let data = std::fs::read(path).unwrap_or_default();
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(&data);
        hex::encode(hasher.finalize())
    };
    let content = if data.len() as u64 <= MAX_SNAPSHOT_FILE_CONTENT_BYTES {
        String::from_utf8(data).ok()
    } else {
        None
    };
    SnapshotEntry {
        path: path.to_path_buf(),
        hash,
        content,
    }
}

/// Recursively collect all files under `dir` and build snapshot entries.
fn collect_files(dir: &Path, entries: &mut Vec<SnapshotEntry>, visited: &mut BTreeSet<PathBuf>) {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in iter {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !visited.insert(path.clone()) {
            continue;
        }
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(
                name,
                ".git" | "target" | "node_modules" | ".cache" | ".venv" | "venv" | "__pycache__"
            ) {
                collect_files(&path, entries, visited);
            }
        } else if path.is_file() {
            entries.push(snapshot_file(&path));
        }
    }
}

/// Find files under `dir` that were not in the original snapshot.
fn find_new_files(dir: &Path, snapshot_paths: &HashSet<&PathBuf>, created: &mut Vec<PathBuf>) {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in iter {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(name, ".git" | "target" | "node_modules") {
                find_new_files(&path, snapshot_paths, created);
            }
        } else if path.is_file() && !snapshot_paths.contains(&path) {
            created.push(path);
        }
    }
}

/// Delete files under `dir` that were not in the original snapshot.
fn delete_new_files(dir: &Path, snapshot_paths: &HashSet<&PathBuf>) -> Result<(), String> {
    let Ok(iter) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in iter {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !matches!(name, ".git" | "target" | "node_modules") {
                delete_new_files(&path, snapshot_paths)?;
            }
            // Remove empty directories left behind.
            let _ = std::fs::remove_dir(&path);
        } else if path.is_file() && !snapshot_paths.contains(&path) {
            std::fs::remove_file(&path)
                .map_err(|e| format!("failed to delete `{}`: {e}", path.display()))?;
        }
    }
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    // ── Snapshot creation ─────────────────────────────────────────────────

    #[test]
    fn snapshot_captures_file_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        write(&f1, "hello");
        write(&f2, "world");

        let snap = SandboxManager::create_snapshot(&[f1.clone(), f2.clone()]);
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.entries.iter().any(|e| e.path == f1));
        assert!(snap.entries.iter().any(|e| e.path == f2));
        assert!(snap.entries.iter().all(|e| !e.hash.is_empty()));
        assert!(snap.entries.iter().all(|e| e.content.is_some()));
        assert!(snap.created_at > 0);
    }

    #[test]
    fn snapshot_captures_directory_recursively() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sub = dir.path().join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        write(&sub.join("deep.txt"), "deep");
        write(&dir.path().join("root.txt"), "root");

        let snap = SandboxManager::create_snapshot(&[dir.path().to_path_buf()]);
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.entries.iter().any(|e| e.path == sub.join("deep.txt")));
        assert!(
            snap.entries
                .iter()
                .any(|e| e.path == dir.path().join("root.txt"))
        );
    }

    #[test]
    fn snapshot_skips_git_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = dir.path().join(".git");
        fs::create_dir_all(&git).unwrap();
        write(&git.join("HEAD"), "ref: refs/heads/main");
        write(&dir.path().join("actual.txt"), "real");

        let snap = SandboxManager::create_snapshot(&[dir.path().to_path_buf()]);
        assert_eq!(snap.entries.len(), 1);
        assert!(
            snap.entries
                .iter()
                .any(|e| e.path == dir.path().join("actual.txt"))
        );
    }

    // ── Rollback: restore modified files ─────────────────────────────────

    #[test]
    fn rollback_restores_modified_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("test.txt");
        write(&f, "original");

        let snap = SandboxManager::create_snapshot(&[f.clone()]);
        write(&f, "modified content");
        assert_eq!(fs::read_to_string(&f).unwrap(), "modified content");

        SandboxManager::rollback(&snap).unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "original");
    }

    #[test]
    fn rollback_restores_deleted_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("gone.txt");
        write(&f, "will be restored");

        let snap = SandboxManager::create_snapshot(&[f.clone()]);
        fs::remove_file(&f).unwrap();
        assert!(!f.exists());

        SandboxManager::rollback(&snap).unwrap();
        assert!(f.exists());
        assert_eq!(fs::read_to_string(&f).unwrap(), "will be restored");
    }

    // ── Rollback: delete created files ────────────────────────────────────

    #[test]
    fn rollback_deletes_created_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let existing = dir.path().join("keep.txt");
        write(&existing, "original");

        let snap = SandboxManager::create_snapshot(&[dir.path().to_path_buf()]);
        let created = dir.path().join("created.txt");
        write(&created, "new file");
        assert!(created.exists());

        SandboxManager::rollback(&snap).unwrap();
        assert!(!created.exists(), "created file should be deleted");
        assert!(existing.exists(), "existing file should be kept");
    }

    #[test]
    fn rollback_deletes_file_created_at_snapshotted_future_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let created = dir.path().join("future").join("created.txt");

        let snap = SandboxManager::create_snapshot(std::slice::from_ref(&created));
        write(&created, "new file");
        assert!(created.exists());

        SandboxManager::rollback(&snap).unwrap();
        assert!(!created.exists(), "created file should be deleted");
    }

    #[test]
    fn rollback_restores_modified_and_deletes_created_in_single_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        write(&a, "a original");

        let snap = SandboxManager::create_snapshot(&[dir.path().to_path_buf()]);
        write(&a, "a modified");
        let b = dir.path().join("b.txt");
        write(&b, "b new");

        SandboxManager::rollback(&snap).unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "a original");
        assert!(!b.exists());
    }

    // ── Rollback errors ───────────────────────────────────────────────────

    #[test]
    fn rollback_without_content_returns_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("large.txt");
        write(&f, "data");

        let snap = WorkspaceSnapshot {
            id: "test".into(),
            entries: vec![SnapshotEntry {
                path: f.clone(),
                hash: hash_file(&f).unwrap(),
                content: None,
            }],
            roots: vec![],
            created_at: 0,
        };

        // Modify the file (can't restore because content is None).
        write(&f, "modified");
        let err = SandboxManager::rollback(&snap).unwrap_err();
        assert!(err.contains("not available"), "error: {err}");
    }

    #[test]
    fn snapshot_binary_content_is_not_lossy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("binary.bin");
        fs::write(&f, [0xff, 0x00, 0x61]).unwrap();

        let snap = SandboxManager::create_snapshot(std::slice::from_ref(&f));
        assert_eq!(snap.entries.len(), 1);
        assert!(
            snap.entries[0].content.is_none(),
            "binary content must not be stored through lossy UTF-8"
        );
    }

    // ── Rollback: non-existent snapshot (empty) ───────────────────────────

    #[test]
    fn rollback_of_empty_snapshot_succeeds() {
        let snap = WorkspaceSnapshot {
            id: "empty".into(),
            entries: vec![],
            roots: vec![],
            created_at: 0,
        };
        assert!(SandboxManager::rollback(&snap).is_ok());
    }

    // ── ChangeSet detection ───────────────────────────────────────────────

    #[test]
    fn compute_changes_detects_created_modified_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        write(&a, "a content");
        write(&b, "b content");

        let snap = SandboxManager::create_snapshot(&[dir.path().to_path_buf()]);

        // Modify a, delete b, create c.
        write(&a, "a modified");
        fs::remove_file(&b).unwrap();
        let c = dir.path().join("c.txt");
        write(&c, "c new");

        let changes = SandboxManager::compute_changes(&snap);
        assert!(
            changes.files_modified.iter().any(|p| p == &a),
            "a should be modified"
        );
        assert!(
            changes.files_deleted.iter().any(|p| p == &b),
            "b should be deleted"
        );
        assert!(
            changes.files_created.iter().any(|p| p == &c),
            "c should be created"
        );
    }

    #[test]
    fn compute_changes_empty_when_no_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("stable.txt");
        write(&f, "stable");

        let snap = SandboxManager::create_snapshot(&[f.clone()]);
        let changes = SandboxManager::compute_changes(&snap);
        assert!(changes.is_empty());
        assert_eq!(changes.total(), 0);
    }

    // ── has_staged_changes ────────────────────────────────────────────────

    #[test]
    fn has_staged_changes_returns_bool_without_panicking() {
        // Must not panic regardless of environment. Returns false when
        // not in a git repo or when git index is clean.
        let _ = SandboxManager::has_staged_changes();
    }

    // ── SnapshotEntry helpers ─────────────────────────────────────────────

    #[test]
    fn is_path_in_entry_works_correctly() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        write(&a, "data");
        let snap = SandboxManager::create_snapshot(&[a.clone()]);
        assert!(snap.is_path_in_entry(&a));
        assert!(!snap.is_path_in_entry(&dir.path().join("nonexistent.txt")));
    }
}
