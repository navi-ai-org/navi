use anyhow::{Context, Result};
use directories::BaseDirs;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Resolves a memory configuration path.
///
/// Relative paths are rooted in NAVI's data directory so memory files do not
/// spill into the project workspace. `~` and absolute paths remain explicit
/// user-selected locations.
pub fn resolve_memory_path(path_str: &str, data_dir: &Path) -> PathBuf {
    if path_str.starts_with("~/") || path_str == "~" {
        if let Some(base_dirs) = BaseDirs::new() {
            let home = base_dirs.home_dir();
            if path_str == "~" {
                home.to_path_buf()
            } else {
                home.join(&path_str[2..])
            }
        } else {
            PathBuf::from(path_str)
        }
    } else {
        let configured = PathBuf::from(path_str);
        if configured.is_absolute() {
            configured
        } else {
            data_dir.join(configured)
        }
    }
}

/// Stable per-project directory name used for data-dir scoped memory.
pub fn project_hash(project_dir: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    project_dir.to_string_lossy().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Validates that `path` is not a symlink and that its resolved parent directory
/// resides physically within the canonicalized `expected_root`.
pub fn validate_write_path(path: &Path, expected_root: &Path) -> Result<()> {
    // 1. If path itself exists, ensure it is not a symlink
    if path.exists() {
        let meta = fs::symlink_metadata(path)
            .with_context(|| format!("Failed to read metadata for {:?}", path))?;
        if meta.is_symlink() {
            anyhow::bail!(
                "Path safety violation: target path is a symlink: {:?}",
                path
            );
        }
    }

    // 2. Resolve parent directory
    let parent = path.parent().context("Path must have a parent directory")?;
    if !parent.exists() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directories: {:?}", parent))?;
    }

    // 3. Resolve canonical paths to prevent path traversal/escaping
    let canon_parent = parent
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize parent: {:?}", parent))?;
    let canon_root = expected_root
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize expected root: {:?}", expected_root))?;

    if !canon_parent.starts_with(&canon_root) {
        anyhow::bail!(
            "Path safety violation: target path parent {:?} is outside expected root {:?}",
            canon_parent,
            canon_root
        );
    }

    Ok(())
}

/// Atomically writes content after verifying the target resides within `expected_root`.
pub fn write_atomic_safe(path: &Path, expected_root: &Path, content: &str) -> Result<()> {
    validate_write_path(path, expected_root)?;
    write_atomic(path, content)
}

pub fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directories: {:?}", parent))?;
        }
    }

    // Create backup if target exists
    if path.exists() {
        let backup_path = path.with_extension("bak");
        let _ = fs::copy(path, &backup_path);
    }

    // Create temp file in the same directory to guarantee atomic rename
    let pid = std::process::id();
    let thread_id = std::thread::current().id();
    let temp_name = format!(
        ".tmp-{}-{:?}-{}.tmp",
        pid,
        thread_id,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let temp_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(temp_name);

    {
        let mut file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp file: {:?}", temp_path))?;
        file.write_all(content.as_bytes())
            .with_context(|| format!("Failed to write to temp file: {:?}", temp_path))?;
        file.sync_all()
            .with_context(|| format!("Failed to fsync temp file: {:?}", temp_path))?;
    }

    fs::rename(&temp_path, path)
        .with_context(|| format!("Failed to rename temp file to target: {:?}", path))?;

    Ok(())
}

/// Manager for long-horizon memory files on the local filesystem.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    pub project_dir: PathBuf,
    pub memory_root: PathBuf,
    pub global_memory_path: PathBuf,
}

impl MemoryStore {
    pub fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        root_config: &str,
        global_config_path: &str,
    ) -> Self {
        let memory_root =
            resolve_memory_path(root_config, &data_dir).join(project_hash(&project_dir));
        let global_memory_path = resolve_memory_path(global_config_path, &data_dir);
        Self {
            project_dir,
            memory_root,
            global_memory_path,
        }
    }

    pub fn checkpoint_path(&self) -> PathBuf {
        self.memory_root.join("checkpoint.md")
    }

    pub fn notes_path(&self) -> PathBuf {
        self.memory_root.join("notes.md")
    }

    pub fn project_memory_path(&self) -> PathBuf {
        self.memory_root.join("MEMORY.md")
    }

    pub fn global_memory_path(&self) -> PathBuf {
        self.global_memory_path.clone()
    }

    pub fn read_checkpoint(&self) -> Result<String> {
        let path = self.checkpoint_path();
        if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("Failed to read checkpoint: {:?}", path))
        } else {
            Ok(String::new())
        }
    }

    pub fn write_checkpoint(&self, content: &str) -> Result<()> {
        write_atomic_safe(&self.checkpoint_path(), &self.memory_root, content)
    }

    pub fn read_notes(&self) -> Result<String> {
        let path = self.notes_path();
        if path.exists() {
            fs::read_to_string(&path).with_context(|| format!("Failed to read notes: {:?}", path))
        } else {
            Ok(String::new())
        }
    }

    pub fn append_note(&self, content: &str) -> Result<()> {
        let path = self.notes_path();
        validate_write_path(&path, &self.memory_root)?;
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open notes for append: {:?}", path))?;

        writeln!(file, "\n{}", content.trim())
            .with_context(|| format!("Failed to write to notes: {:?}", path))?;
        file.sync_all()?;
        Ok(())
    }

    pub fn clear_notes(&self) -> Result<()> {
        let path = self.notes_path();
        if path.exists() {
            write_atomic_safe(&path, &self.memory_root, "")?;
        }
        Ok(())
    }

    pub fn archive_notes(&self, notes_content: &str) -> Result<()> {
        if notes_content.trim().is_empty() {
            return Ok(());
        }
        let archive_dir = self.memory_root.join("archive");
        if !archive_dir.exists() {
            fs::create_dir_all(&archive_dir)?;
        }
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let archive_path = archive_dir.join(format!("notes-{}.md", timestamp));
        write_atomic_safe(&archive_path, &self.memory_root, notes_content)?;
        self.clear_notes()?;
        Ok(())
    }

    pub fn read_project_memory(&self) -> Result<String> {
        let path = self.project_memory_path();
        if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("Failed to read project memory: {:?}", path))
        } else {
            Ok(String::new())
        }
    }

    pub fn write_project_memory(&self, content: &str) -> Result<()> {
        write_atomic_safe(&self.project_memory_path(), &self.memory_root, content)
    }

    pub fn read_global_memory(&self) -> Result<String> {
        let path = self.global_memory_path();
        if path.exists() {
            fs::read_to_string(&path)
                .with_context(|| format!("Failed to read global memory: {:?}", path))
        } else {
            Ok(String::new())
        }
    }

    pub fn write_global_memory(&self, content: &str) -> Result<()> {
        let path = self.global_memory_path();
        if let Some(parent) = path.parent() {
            write_atomic_safe(&path, parent, content)
        } else {
            write_atomic(&path, content)
        }
    }

    /// Initializes default files if they do not exist
    pub fn ensure_initialized(&self) -> Result<()> {
        if !self.memory_root.exists() {
            fs::create_dir_all(&self.memory_root)?;
        }
        let notes_p = self.notes_path();
        if !notes_p.exists() {
            write_atomic_safe(&notes_p, &self.memory_root, "")?;
        }
        let check_p = self.checkpoint_path();
        if !check_p.exists() {
            write_atomic_safe(&check_p, &self.memory_root, "# Session Checkpoint\n")?;
        }
        let pm_p = self.project_memory_path();
        if !pm_p.exists() {
            write_atomic_safe(&pm_p, &self.memory_root, "# Project Memory\n")?;
        }
        let gm_p = self.global_memory_path();
        if !gm_p.exists() {
            if let Some(parent) = gm_p.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
                write_atomic_safe(&gm_p, parent, "# Global Memory\n")?;
            } else {
                write_atomic(&gm_p, "# Global Memory\n")?;
            }
        }
        Ok(())
    }

    pub fn migrate_legacy_project_memory_if_empty(&self) -> Result<()> {
        let legacy_root = self.project_dir.join(".agent-memory");
        if !legacy_root.exists()
            || !legacy_root.is_dir()
            || !target_dir_is_empty(&self.memory_root)?
        {
            return Ok(());
        }

        copy_dir_without_symlinks(&legacy_root, &self.memory_root)?;
        Ok(())
    }
}

fn target_dir_is_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(true);
    }
    Ok(fs::read_dir(path)?.next().is_none())
}

fn copy_dir_without_symlinks(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source).with_context(|| format!("Failed to read {:?}", source))? {
        let entry = entry?;
        let source_path = entry.path();
        let metadata = fs::symlink_metadata(&source_path)
            .with_context(|| format!("Failed to inspect {:?}", source_path))?;
        if metadata.file_type().is_symlink() {
            continue;
        }

        let target_path = target.join(entry.file_name());
        if metadata.is_dir() {
            copy_dir_without_symlinks(&source_path, &target_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "Failed to copy legacy memory file {:?} to {:?}",
                    source_path, target_path
                )
            })?;
        }
    }
    Ok(())
}
