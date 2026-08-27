//! App-Scoped Storage Namespacing in `{data_dir}` (ADR 0019).
//!
//! Provides application isolation for `{data_dir}` artifacts so first-party
//! applications driving NAVI (Tutor, organize-anything, etc.) store their
//! sessions, plans, memories, and harness packs under `{data_dir}/apps/<app-id>/`
//! while sharing user-owned credentials, registry caches, and binary models.
//!
//! ADR 0018 §5 additionally allows an app profile to request a distinct storage
//! `namespace` (defaulting to the app `id`) and to opt out of credential sharing
//! via `shareCredentials` (default `true`).

use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// App scope identifier and path resolver (ADR 0019, ADR 0018 §5).
///
/// An app scope encapsulates an untrusted `app-id` string and resolves
/// artifact paths under `{data_dir}`. The `namespace` is the directory name
/// used under `{data_dir}/apps/` (defaults to the app id); `share_credentials`
/// controls whether API keys live in the shared user store or are isolated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppScope {
    app_id: Option<String>,
    namespace: Option<String>,
    share_credentials: bool,
}

impl Default for AppScope {
    fn default() -> Self {
        Self::none()
    }
}

impl AppScope {
    /// Create an inactive (unscoped / default user) app scope.
    pub fn none() -> Self {
        Self {
            app_id: None,
            namespace: None,
            share_credentials: true,
        }
    }

    /// Create a validated `AppScope` from a raw app ID string (ADR 0019 §2).
    ///
    /// The storage `namespace` defaults to the app id (ADR 0018 §5).
    /// Validates that:
    /// - Matches `^[a-z0-9][a-z0-9._-]{0,63}$`
    /// - Contains no path separators or `.` / `..` components
    pub fn new(app_id: impl Into<String>) -> Result<Self> {
        let id = app_id.into();
        Self::validate_id(&id)?;
        Ok(Self {
            app_id: Some(id.clone()),
            namespace: Some(id),
            share_credentials: true,
        })
    }

    /// Create a validated `AppScope` with an explicit storage namespace.
    ///
    /// Both the app id and namespace are validated with the same ADR 0019 §2
    /// rules (they are both single path components under `apps/`).
    pub fn with_namespace(app_id: impl Into<String>, namespace: impl Into<String>) -> Result<Self> {
        let id = app_id.into();
        let namespace = namespace.into();
        Self::validate_id(&id)?;
        Self::validate_id(&namespace)?;
        Ok(Self {
            app_id: Some(id),
            namespace: Some(namespace),
            share_credentials: true,
        })
    }

    /// Builder: control credential sharing (ADR 0018 §5 `shareCredentials`).
    ///
    /// When `false`, credentials are stored under the app's scoped root instead
    /// of the shared `{data_dir}/credentials.toml`.
    pub fn with_share_credentials(mut self, shared: bool) -> Self {
        self.share_credentials = shared;
        self
    }

    /// Returns the app ID if active.
    pub fn id(&self) -> Option<&str> {
        self.app_id.as_deref()
    }

    /// Returns the storage namespace if active (defaults to the app id).
    pub fn namespace(&self) -> Option<&str> {
        self.namespace.as_deref()
    }

    /// Whether credentials are shared with the user's global store.
    ///
    /// Inactive scopes always share (there is no app root to isolate into).
    pub fn shares_credentials(&self) -> bool {
        !self.is_active() || self.share_credentials
    }

    /// Returns `true` if an app scope is active.
    pub fn is_active(&self) -> bool {
        self.app_id.is_some()
    }

    /// Validate raw app ID against ADR 0019 rules.
    pub fn validate_id(id: &str) -> Result<()> {
        if id.is_empty() {
            bail!("app ID cannot be empty");
        }
        if id.len() > 64 {
            bail!("app ID too long (maximum 64 characters, got {})", id.len());
        }

        // Validate character set: ^[a-z0-9][a-z0-9._-]{0,63}$
        let mut chars = id.chars();
        let first = chars.next().unwrap();
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            bail!(
                "invalid app ID '{id}': first character must be lowercase alphanumeric (a-z, 0-9)"
            );
        }

        for c in chars {
            if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '.' && c != '_' && c != '-' {
                bail!(
                    "invalid app ID '{id}': contains invalid character '{c}' (allowed: a-z, 0-9, ., _, -)"
                );
            }
        }

        // Check path component safety (no separators or special components).
        let path = Path::new(id);
        for comp in path.components() {
            match comp {
                Component::Normal(_) => {}
                _ => bail!("invalid app ID '{id}': path traversal components are prohibited"),
            }
        }

        Ok(())
    }

    /// Resolve the root application directory under `{data_dir}` (ADR 0019 §1).
    ///
    /// When active: `{data_dir}/apps/<namespace>/`
    /// When inactive: `{data_dir}/`
    pub fn app_root(&self, data_dir: &Path) -> PathBuf {
        match &self.namespace {
            Some(namespace) => data_dir.join("apps").join(namespace),
            None => data_dir.to_path_buf(),
        }
    }

    /// Verify that a resolved path stays strictly inside `{data_dir}/apps/<namespace>/`
    /// (ADR 0019 §2).
    ///
    /// Canonicalizes both the expected root and the candidate path where possible
    /// so that a symlink pointing outside the app root cannot defeat the check.
    pub fn verify_path_jail(&self, data_dir: &Path, resolved_path: &Path) -> Result<()> {
        if !self.is_active() {
            return Ok(());
        }
        let expected_root = canonicalize_existing_or_parent(&self.app_root(data_dir))
            .unwrap_or_else(|| self.app_root(data_dir));
        let resolved = canonicalize_existing_or_parent(resolved_path)
            .unwrap_or_else(|| resolved_path.to_path_buf());
        let display_root = expected_root.display().to_string();
        let display_path = resolved.display().to_string();

        if !resolved.starts_with(&expected_root) {
            bail!(
                "path traversal security error: path '{display_path}' escapes app root '{display_root}'"
            );
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Scoped Artifact Paths (App-owned, ADR 0019 §1)
    // -----------------------------------------------------------------------

    /// `{data_dir}/apps/<namespace>/sessions` (scoped) or `{data_dir}/sessions` (unscoped).
    pub fn sessions_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("sessions")
    }

    /// `{data_dir}/apps/<namespace>/plans` (scoped) or `{data_dir}/plans` (unscoped).
    pub fn plans_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("plans")
    }

    /// `{data_dir}/apps/<namespace>/memory` (scoped) or `{data_dir}/memory` (unscoped).
    pub fn memory_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("memory")
    }

    /// `{data_dir}/apps/<namespace>/harnesses` (scoped) or `{data_dir}/harnesses` (unscoped).
    pub fn harnesses_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("harnesses")
    }

    /// `{data_dir}/apps/<namespace>/skills` (scoped) or `{data_dir}/skills` (unscoped).
    pub fn skills_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("skills")
    }

    /// `{data_dir}/apps/<namespace>/sprints` (scoped) or `{data_dir}/sprints` (unscoped).
    pub fn sprints_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("sprints")
    }

    /// `{data_dir}/apps/<namespace>/traces` (scoped) or `{data_dir}/traces` (unscoped).
    pub fn traces_dir(&self, data_dir: &Path) -> PathBuf {
        self.app_root(data_dir).join("traces")
    }

    // -----------------------------------------------------------------------
    // Shared Artifact Paths (User-owned, ADR 0019 §1)
    // -----------------------------------------------------------------------

    /// Credentials path (ADR 0019 §1 + ADR 0018 §5 `shareCredentials`).
    ///
    /// - Shared (`shareCredentials` unset or `true`): `{data_dir}/credentials.toml`
    /// - Scoped (`shareCredentials` `false`): `{data_dir}/apps/<namespace>/credentials.toml`
    pub fn credentials_path(&self, data_dir: &Path) -> PathBuf {
        if self.shares_credentials() {
            data_dir.join("credentials.toml")
        } else {
            self.app_root(data_dir).join("credentials.toml")
        }
    }

    /// Always `{data_dir}/registry` (shared).
    pub fn registry_dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("registry")
    }

    /// Always `{data_dir}/plugins` (shared install).
    pub fn plugins_dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("plugins")
    }

    /// Always `{data_dir}/voice` (shared).
    pub fn voice_dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("voice")
    }

    /// Always `{data_dir}/browser` (shared).
    pub fn browser_dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("browser")
    }
}

/// Canonicalize an existing path, or walk up to the nearest existing ancestor
/// and canonicalize that, re-appending the missing components. Returns `None`
/// when canonicalization is impossible (e.g. no existing ancestor).
fn canonicalize_existing_or_parent(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path.canonicalize().ok();
    }

    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        let component = current.file_name()?;
        missing.push(component.to_os_string());
        current = current.parent()?;
    }

    let mut normalized = current.canonicalize().ok()?;
    for component in missing.iter().rev() {
        normalized.push(component);
    }
    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_app_ids_pass_validation() {
        let valid = [
            "tutor",
            "organize-anything",
            "navi_app_1",
            "app.id.test",
            "a",
            "123app",
        ];
        for id in valid {
            assert!(
                AppScope::validate_id(id).is_ok(),
                "should accept valid app ID: {id}"
            );
            assert_eq!(AppScope::new(id).unwrap().id(), Some(id));
        }
    }

    #[test]
    fn invalid_app_ids_fail_validation() {
        let invalid = [
            "", "App",      // uppercase
            "app/sub",  // path separator
            "app\\sub", // windows path separator
            "../app",   // path traversal
            "..",       // parent dir
            ".",        // current dir
            "_app",     // starts with underscore
            "-app",     // starts with hyphen
            ".app",     // starts with dot
            "app id",   // space
            "app@id",   // special character
        ];
        for id in invalid {
            assert!(
                AppScope::validate_id(id).is_err(),
                "should reject invalid app ID: {id}"
            );
            assert!(AppScope::new(id).is_err());
        }
    }

    #[test]
    fn app_id_too_long_fails() {
        let long_id = "a".repeat(65);
        assert!(AppScope::validate_id(&long_id).is_err());
    }

    #[test]
    fn unscoped_app_scope_returns_unscoped_paths() {
        let scope = AppScope::none();
        assert!(!scope.is_active());
        assert_eq!(scope.id(), None);
        assert_eq!(scope.namespace(), None);
        assert!(scope.shares_credentials());

        let data_dir = Path::new("/data/navi");
        assert_eq!(scope.app_root(data_dir), PathBuf::from("/data/navi"));
        assert_eq!(
            scope.sessions_dir(data_dir),
            PathBuf::from("/data/navi/sessions")
        );
        assert_eq!(
            scope.memory_dir(data_dir),
            PathBuf::from("/data/navi/memory")
        );
        assert_eq!(
            scope.credentials_path(data_dir),
            PathBuf::from("/data/navi/credentials.toml")
        );
    }

    #[test]
    fn scoped_app_scope_resolves_scoped_vs_shared_paths() {
        let scope = AppScope::new("tutor").unwrap();
        assert!(scope.is_active());
        assert_eq!(scope.id(), Some("tutor"));
        assert_eq!(scope.namespace(), Some("tutor"));

        let data_dir = Path::new("/data/navi");
        assert_eq!(
            scope.app_root(data_dir),
            PathBuf::from("/data/navi/apps/tutor")
        );
        assert_eq!(
            scope.sessions_dir(data_dir),
            PathBuf::from("/data/navi/apps/tutor/sessions")
        );
        assert_eq!(
            scope.plans_dir(data_dir),
            PathBuf::from("/data/navi/apps/tutor/plans")
        );
        assert_eq!(
            scope.memory_dir(data_dir),
            PathBuf::from("/data/navi/apps/tutor/memory")
        );
        assert_eq!(
            scope.harnesses_dir(data_dir),
            PathBuf::from("/data/navi/apps/tutor/harnesses")
        );

        // Shared paths remain shared.
        assert_eq!(
            scope.credentials_path(data_dir),
            PathBuf::from("/data/navi/credentials.toml")
        );
        assert_eq!(
            scope.plugins_dir(data_dir),
            PathBuf::from("/data/navi/plugins")
        );
    }

    #[test]
    fn with_namespace_resolves_namespace_root() {
        let scope = AppScope::with_namespace("tutor", "tutor-prod").unwrap();
        assert_eq!(scope.id(), Some("tutor"));
        assert_eq!(scope.namespace(), Some("tutor-prod"));

        let data_dir = Path::new("/data/navi");
        assert_eq!(
            scope.app_root(data_dir),
            PathBuf::from("/data/navi/apps/tutor-prod")
        );
        assert_eq!(
            scope.sessions_dir(data_dir),
            PathBuf::from("/data/navi/apps/tutor-prod/sessions")
        );
    }

    #[test]
    fn with_namespace_validates_namespace() {
        assert!(AppScope::with_namespace("tutor", "bad/ns").is_err());
        assert!(AppScope::with_namespace("tutor", "..").is_err());
        assert!(AppScope::with_namespace("tutor", "").is_err());
    }

    #[test]
    fn share_credentials_false_scopes_credentials_path() {
        let shared = AppScope::new("tutor").unwrap();
        let scoped = AppScope::new("tutor")
            .unwrap()
            .with_share_credentials(false);

        let data_dir = Path::new("/data/navi");
        assert!(shared.shares_credentials());
        assert!(!scoped.shares_credentials());
        assert_eq!(
            shared.credentials_path(data_dir),
            PathBuf::from("/data/navi/credentials.toml")
        );
        assert_eq!(
            scoped.credentials_path(data_dir),
            PathBuf::from("/data/navi/apps/tutor/credentials.toml")
        );
    }

    #[test]
    fn default_scope_is_inactive_and_shares_credentials() {
        let scope = AppScope::default();
        assert!(!scope.is_active());
        assert!(scope.shares_credentials());
        assert_eq!(scope.id(), None);
        assert_eq!(scope.namespace(), None);
    }

    #[test]
    fn verify_path_jail_detects_escapes() {
        let scope = AppScope::new("app1").unwrap();
        let data_dir = Path::new("/data/navi");

        let valid_path = data_dir
            .join("apps")
            .join("app1")
            .join("sessions")
            .join("s1");
        assert!(scope.verify_path_jail(data_dir, &valid_path).is_ok());

        let invalid_path = data_dir
            .join("apps")
            .join("app2")
            .join("sessions")
            .join("s1");
        assert!(scope.verify_path_jail(data_dir, &invalid_path).is_err());
    }
}
