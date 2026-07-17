//! Durable per-user-turn file snapshots for session rewind.
//!
//! Layout under `{data_dir}/sessions/{session_id}/rewind/`:
//! ```text
//! points.jsonl          # one RewindPointMeta per line
//! blobs/{sha256}        # raw file bytes (binary-safe)
//! ```
//!
//! Never writes inside the project tree (AGENTS.md: no agent bookkeeping in worktree).

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Soft cap per file when capturing (32 MiB). Larger files are skipped with a note.
pub const MAX_REWIND_BLOB_BYTES: u64 = 32 * 1024 * 1024;

/// Preview length for palette / modal listing.
pub const PROMPT_PREVIEW_CHARS: usize = 120;

/// One file entry inside a rewind point (pre-turn state).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RewindFileEntry {
    /// Project-relative path (forward slashes preferred for portability).
    pub rel_path: String,
    /// Content-addressed blob id (sha256 hex), or `None` if the path did not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<String>,
    /// Whether the path existed as a regular file at capture time.
    pub existed: bool,
}

/// Checkpoint recorded at the start of a user turn (before agent tools run).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RewindPointMeta {
    /// 0-based ordinal among `UserTaskSubmitted` events in the session.
    pub prompt_index: usize,
    pub created_at: u64,
    /// Truncated user text for UI listing.
    #[serde(default)]
    pub prompt_preview: String,
    /// Pre-turn file states for the dirty set.
    #[serde(default)]
    pub files: Vec<RewindFileEntry>,
    /// Relative paths created during this turn (delete on restore to this point).
    #[serde(default)]
    pub created_paths: Vec<String>,
}

/// Summary returned after a filesystem restore.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreSummary {
    pub restored: usize,
    pub deleted: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

impl RestoreSummary {
    pub fn total_changes(&self) -> usize {
        self.restored + self.deleted
    }

    pub fn is_empty(&self) -> bool {
        self.restored == 0 && self.deleted == 0
    }
}

/// Session-scoped dirty path tracker + disk-backed rewind store.
#[derive(Debug, Clone)]
pub struct RewindStore {
    /// `{data_dir}/sessions/{session_id}/rewind`
    root: PathBuf,
    project_root: PathBuf,
    /// Absolute paths touched by write tools since session start / last clear.
    dirty: HashSet<PathBuf>,
    /// Absolute paths written during the *current* turn (for `created_paths`).
    turn_written: HashSet<PathBuf>,
}

impl RewindStore {
    pub fn new(data_dir: impl AsRef<Path>, session_id: &str, project_root: impl AsRef<Path>) -> Self {
        let root = data_dir
            .as_ref()
            .join("sessions")
            .join(session_id)
            .join("rewind");
        Self {
            root,
            project_root: project_root.as_ref().to_path_buf(),
            dirty: HashSet::new(),
            turn_written: HashSet::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn points_path(&self) -> PathBuf {
        self.root.join("points.jsonl")
    }

    fn ensure_dirs(&self) -> std::io::Result<()> {
        fs::create_dir_all(self.blobs_dir())
    }

    /// Mark absolute paths as dirty (session lifetime) and turn-written.
    pub fn note_written_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            let abs = if p.is_absolute() {
                p
            } else {
                self.project_root.join(&p)
            };
            self.dirty.insert(abs.clone());
            self.turn_written.insert(abs);
        }
    }

    /// Clear per-turn write set (call after capturing a point / finishing turn).
    pub fn clear_turn_written(&mut self) {
        self.turn_written.clear();
    }

    /// Whether `abs` is already tracked as dirty for this session.
    pub fn is_dirty(&self, abs: &Path) -> bool {
        let abs = if abs.is_absolute() {
            abs.to_path_buf()
        } else {
            self.project_root.join(abs)
        };
        self.dirty.contains(&abs)
    }

    /// Count of user turns already recorded (for next prompt_index).
    pub fn next_prompt_index(&self) -> usize {
        self.load_points().len()
    }

    /// Before a write tool mutates disk: snapshot paths not yet in the dirty set
    /// into the latest rewind point so restore can undo first-touch edits.
    ///
    /// Call this with absolute (or project-relative) paths while the file still
    /// has its pre-write content. Marks paths dirty + turn-written.
    pub fn ensure_pre_write_capture(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> std::io::Result<()> {
        let mut points = self.load_points();
        let Some(point) = points.last_mut() else {
            // No checkpoint yet (turn capture failed / race): still track dirty.
            self.note_written_paths(paths);
            return Ok(());
        };

        let mut changed = false;
        for p in paths {
            let abs = if p.is_absolute() {
                p
            } else {
                self.project_root.join(&p)
            };
            if self.dirty.contains(&abs) {
                self.turn_written.insert(abs);
                continue;
            }
            let Some(rel) = rel_path_for(&self.project_root, &abs) else {
                continue;
            };
            // Skip if already recorded on this point.
            if point.files.iter().any(|f| f.rel_path == rel) {
                self.dirty.insert(abs.clone());
                self.turn_written.insert(abs);
                continue;
            }
            let _ = self.ensure_dirs();
            let entry = capture_file_entry(&abs, &rel, &self.blobs_dir())?;
            point.files.push(entry);
            self.dirty.insert(abs.clone());
            self.turn_written.insert(abs);
            changed = true;
        }
        if changed {
            rewrite_points(&self.points_path(), &points)?;
        }
        Ok(())
    }

    /// Capture pre-turn state for `prompt_index` and append to disk.
    pub fn capture_point(
        &mut self,
        prompt_index: usize,
        prompt_text: &str,
        created_at: u64,
    ) -> std::io::Result<RewindPointMeta> {
        self.ensure_dirs()?;
        let mut files = Vec::new();
        let dirty: Vec<PathBuf> = self.dirty.iter().cloned().collect();
        for abs in dirty {
            let Some(rel) = rel_path_for(&self.project_root, &abs) else {
                continue;
            };
            let entry = capture_file_entry(&abs, &rel, &self.blobs_dir())?;
            files.push(entry);
        }

        // Paths written during *previous* turn that didn't exist at last capture
        // are already in dirty. created_paths filled at end of turn separately.
        let point = RewindPointMeta {
            prompt_index,
            created_at,
            prompt_preview: truncate_preview(prompt_text, PROMPT_PREVIEW_CHARS),
            files,
            created_paths: Vec::new(),
        };
        append_point(&self.points_path(), &point)?;
        Ok(point)
    }

    /// Finalize created_paths for the latest point (paths newly written this turn
    /// that did not exist in that point's pre-snapshot as existing files).
    pub fn finalize_turn_created_paths(&mut self, prompt_index: usize) -> std::io::Result<()> {
        let mut points = self.load_points();
        let Some(point) = points.iter_mut().find(|p| p.prompt_index == prompt_index) else {
            return Ok(());
        };
        let pre_existed: HashSet<&str> = point
            .files
            .iter()
            .filter(|f| f.existed)
            .map(|f| f.rel_path.as_str())
            .collect();
        let mut created = Vec::new();
        for abs in &self.turn_written {
            let Some(rel) = rel_path_for(&self.project_root, abs) else {
                continue;
            };
            if !pre_existed.contains(rel.as_str()) {
                created.push(rel);
            }
        }
        created.sort();
        created.dedup();
        point.created_paths = created;
        rewrite_points(&self.points_path(), &points)?;
        self.clear_turn_written();
        Ok(())
    }

    pub fn load_points(&self) -> Vec<RewindPointMeta> {
        let path = self.points_path();
        let Ok(file) = fs::File::open(&path) else {
            return Vec::new();
        };
        let reader = std::io::BufReader::new(file);
        let mut points = Vec::new();
        for line in reader.lines() {
            let Ok(line) = line else {
                continue;
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(p) = serde_json::from_str::<RewindPointMeta>(line) {
                points.push(p);
            }
        }
        points
    }

    /// Restore filesystem to pre-turn state of `prompt_index`, then drop points ≥ index.
    ///
    /// Also deletes files first created in turns ≥ `prompt_index`.
    pub fn restore_to(&mut self, prompt_index: usize) -> RestoreSummary {
        let points = self.load_points();
        let mut summary = RestoreSummary::default();

        let point = points.iter().find(|p| p.prompt_index == prompt_index);
        if let Some(point) = point {
            for entry in &point.files {
                let abs = self.project_root.join(normalize_rel(&entry.rel_path));
                if entry.existed {
                    let Some(blob_id) = entry.blob_id.as_ref() else {
                        summary.skipped += 1;
                        summary.errors.push(format!(
                            "skip restore `{}`: no blob (too large or missing)",
                            entry.rel_path
                        ));
                        continue;
                    };
                    let blob_path = self.blobs_dir().join(blob_id);
                    match fs::read(&blob_path) {
                        Ok(bytes) => {
                            if let Some(parent) = abs.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            match fs::write(&abs, &bytes) {
                                Ok(()) => summary.restored += 1,
                                Err(e) => summary
                                    .errors
                                    .push(format!("write `{}`: {e}", entry.rel_path)),
                            }
                        }
                        Err(e) => summary
                            .errors
                            .push(format!("read blob for `{}`: {e}", entry.rel_path)),
                    }
                } else if abs.is_file() {
                    match fs::remove_file(&abs) {
                        Ok(()) => summary.deleted += 1,
                        Err(e) => summary
                            .errors
                            .push(format!("delete `{}`: {e}", entry.rel_path)),
                    }
                }
            }
        } else {
            // No point for this index — still try to delete later creates and truncate.
            tracing::debug!(
                prompt_index,
                "rewind: no snapshot for index; history-only restore for files"
            );
        }

        // Delete files first created at or after this turn.
        let mut to_delete: HashSet<String> = HashSet::new();
        for p in points.iter().filter(|p| p.prompt_index >= prompt_index) {
            for c in &p.created_paths {
                to_delete.insert(c.clone());
            }
        }
        // Paths that appear only after K (not in point K file list as existed)
        let pre_paths: HashSet<String> = point
            .map(|p| {
                p.files
                    .iter()
                    .map(|f| f.rel_path.clone())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        for p in points.iter().filter(|p| p.prompt_index > prompt_index) {
            for f in &p.files {
                if !pre_paths.contains(&f.rel_path) {
                    to_delete.insert(f.rel_path.clone());
                }
            }
        }
        for rel in to_delete {
            let abs = self.project_root.join(normalize_rel(&rel));
            if abs.is_file() {
                match fs::remove_file(&abs) {
                    Ok(()) => summary.deleted += 1,
                    Err(e) => summary.errors.push(format!("delete created `{rel}`: {e}")),
                }
            }
        }

        // Truncate points and drop orphaned blobs best-effort.
        let kept: Vec<RewindPointMeta> = points
            .into_iter()
            .filter(|p| p.prompt_index < prompt_index)
            .collect();
        let _ = rewrite_points(&self.points_path(), &kept);

        // Shrink dirty set to paths still known in remaining points.
        let mut still = HashSet::new();
        for p in &kept {
            for f in &p.files {
                still.insert(self.project_root.join(normalize_rel(&f.rel_path)));
            }
            for c in &p.created_paths {
                still.insert(self.project_root.join(normalize_rel(c)));
            }
        }
        self.dirty = still;
        self.turn_written.clear();

        summary
    }

    /// Remove all rewind data for this session (e.g. on session delete).
    pub fn clear_all(&mut self) -> std::io::Result<()> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        self.dirty.clear();
        self.turn_written.clear();
        Ok(())
    }
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() <= max_chars {
        return t;
    }
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if i >= max_chars.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn normalize_rel(rel: &str) -> String {
    rel.replace('\\', "/")
}

fn rel_path_for(project_root: &Path, abs: &Path) -> Option<String> {
    let abs = abs.canonicalize().unwrap_or_else(|_| abs.to_path_buf());
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    abs.strip_prefix(&root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn capture_file_entry(
    abs: &Path,
    rel: &str,
    blobs_dir: &Path,
) -> std::io::Result<RewindFileEntry> {
    if !abs.is_file() {
        return Ok(RewindFileEntry {
            rel_path: rel.to_string(),
            blob_id: None,
            existed: false,
        });
    }
    let meta = fs::metadata(abs)?;
    if meta.len() > MAX_REWIND_BLOB_BYTES {
        tracing::warn!(
            path = %abs.display(),
            size = meta.len(),
            "rewind: skip file larger than cap"
        );
        // Mark existed so restore won't delete it, but we can't rewrite content.
        return Ok(RewindFileEntry {
            rel_path: rel.to_string(),
            blob_id: None,
            existed: true,
        });
    }
    let bytes = fs::read(abs)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let id = hex::encode(hasher.finalize());
    let blob_path = blobs_dir.join(&id);
    if !blob_path.exists() {
        fs::write(&blob_path, &bytes)?;
    }
    Ok(RewindFileEntry {
        rel_path: rel.to_string(),
        blob_id: Some(id),
        existed: true,
    })
}

fn append_point(path: &Path, point: &RewindPointMeta) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(point)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn rewrite_points(path: &Path, points: &[RewindPointMeta]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        for p in points {
            let line = serde_json::to_string(p)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            writeln!(f, "{line}")?;
        }
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn capture_and_restore_modified_file() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("proj");
        let data = tmp.path().join("data");
        fs::create_dir_all(&project).unwrap();
        let file = project.join("src/a.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"v1").unwrap();

        let mut store = RewindStore::new(&data, "session-1", &project);
        store.note_written_paths([file.clone()]);
        // Pre-turn: dirty includes a.rs with v1
        store
            .capture_point(0, "please edit a.rs", 100)
            .unwrap();

        // Agent mutates
        fs::write(&file, b"v2-agent").unwrap();
        store.note_written_paths([file.clone()]);
        store.finalize_turn_created_paths(0).unwrap();

        // New turn capture then rewind to 0
        store.capture_point(1, "again", 200).unwrap();
        let summary = store.restore_to(0);
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(fs::read_to_string(&file).unwrap(), "v1");
        assert_eq!(store.load_points().len(), 0);
    }

    #[test]
    fn restore_deletes_files_created_after_point() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("proj");
        let data = tmp.path().join("data");
        fs::create_dir_all(&project).unwrap();

        let mut store = RewindStore::new(&data, "session-2", &project);
        // Turn 0: no dirty yet
        store.capture_point(0, "create b.rs", 100).unwrap();
        let new_file = project.join("b.rs");
        fs::write(&new_file, b"new").unwrap();
        store.note_written_paths([new_file.clone()]);
        store.finalize_turn_created_paths(0).unwrap();

        assert!(new_file.exists());
        let summary = store.restore_to(0);
        assert!(!new_file.exists(), "created file must be deleted on restore");
        assert!(summary.deleted >= 1);
    }

    #[test]
    fn binary_blob_roundtrip() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("proj");
        let data = tmp.path().join("data");
        fs::create_dir_all(&project).unwrap();
        let file = project.join("img.bin");
        let bytes = vec![0u8, 159, 146, 150, 255, 1, 2, 3];
        fs::write(&file, &bytes).unwrap();

        let mut store = RewindStore::new(&data, "session-3", &project);
        store.note_written_paths([file.clone()]);
        store.capture_point(0, "binary", 1).unwrap();
        fs::write(&file, b"changed").unwrap();
        store.restore_to(0);
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }

    #[test]
    fn prompt_preview_truncates() {
        let long = "a".repeat(200);
        let p = truncate_preview(&long, 20);
        assert!(p.ends_with('…'));
        assert!(p.chars().count() <= 20);
    }

    #[test]
    fn first_touch_pre_write_capture_restores_existing_file() {
        let tmp = tempdir().unwrap();
        let project = tmp.path().join("proj");
        let data = tmp.path().join("data");
        fs::create_dir_all(&project).unwrap();
        let file = project.join("touched.rs");
        fs::write(&file, b"original").unwrap();

        let mut store = RewindStore::new(&data, "session-4", &project);
        // Turn start: dirty empty
        store.capture_point(0, "edit touched.rs", 1).unwrap();
        // Tool is about to write — capture pre-state first
        store.ensure_pre_write_capture([file.clone()]).unwrap();
        fs::write(&file, b"mutated").unwrap();
        store.note_written_paths([file.clone()]);
        store.finalize_turn_created_paths(0).unwrap();

        let summary = store.restore_to(0);
        assert!(summary.errors.is_empty(), "{:?}", summary.errors);
        assert_eq!(fs::read_to_string(&file).unwrap(), "original");
    }
}
