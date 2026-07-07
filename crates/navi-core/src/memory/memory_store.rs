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
}

impl MemoryStore {
    pub fn new(
        project_dir: PathBuf,
        data_dir: PathBuf,
        root_config: &str,
    ) -> Self {
        let memory_root =
            resolve_memory_path(root_config, &data_dir).join(project_hash(&project_dir));
        Self {
            project_dir,
            memory_root,
        }
    }

    /// Initializes the memory root directory if it does not exist.
    pub fn ensure_initialized(&self) -> Result<()> {
        if !self.memory_root.exists() {
            fs::create_dir_all(&self.memory_root)?;
        }
        Ok(())
    }
}
