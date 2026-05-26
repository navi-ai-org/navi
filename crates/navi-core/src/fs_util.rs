use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

#[cfg(unix)]
pub fn set_private_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict {}", path.display()))
}

#[cfg(not(unix))]
pub fn set_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub fn set_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict {}", path.display()))
}

#[cfg(not(unix))]
pub fn set_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
