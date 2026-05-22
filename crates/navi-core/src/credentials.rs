use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(flatten)]
    providers: HashMap<String, ProviderCredentials>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderCredentials {
    api_key: String,
}

#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            path: data_dir.join("credentials.toml"),
        }
    }

    pub fn get_api_key(&self, provider_id: &str) -> Option<String> {
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers.get(provider_id).map(|c| c.api_key.clone())
    }

    pub fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        ensure_private_parent_dir(&self.path)?;

        let mut file = if self.path.exists() {
            let content =
                fs::read_to_string(&self.path).context("failed to read credentials file")?;
            toml::from_str(&content).unwrap_or_default()
        } else {
            CredentialsFile::default()
        };

        file.providers.insert(
            provider_id.to_string(),
            ProviderCredentials {
                api_key: api_key.to_string(),
            },
        );

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        fs::write(&self.path, &content)
            .with_context(|| format!("failed to write {}", self.path.display()))?;

        // Restrict permissions so only the owner can read the credentials file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// Resolve API key for a provider: env var first (explicit override), then stored credential.
    pub fn resolve_api_key(&self, provider_id: &str, env_var: &str) -> Option<String> {
        if let Ok(key) = std::env::var(env_var) {
            if !key.is_empty() {
                return Some(key);
            }
        }
        self.get_api_key(provider_id)
    }
}

fn ensure_private_parent_dir(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("failed to create credentials directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_store_and_load_api_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        assert!(store.get_api_key("openai").is_none());

        store.set_api_key("openai", "sk-test-123").expect("save");
        assert_eq!(store.get_api_key("openai").as_deref(), Some("sk-test-123"));
    }

    #[test]
    fn overwrite_existing_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        store.set_api_key("openai", "old-key").expect("save");
        store.set_api_key("openai", "new-key").expect("save");
        assert_eq!(store.get_api_key("openai").as_deref(), Some("new-key"));
    }

    #[test]
    fn multiple_providers_stored_independently() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        store.set_api_key("openai", "sk-openai").expect("save");
        store.set_api_key("charm-hyper", "sk-charm").expect("save");

        assert_eq!(store.get_api_key("openai").as_deref(), Some("sk-openai"));
        assert_eq!(
            store.get_api_key("charm-hyper").as_deref(),
            Some("sk-charm")
        );
    }

    #[test]
    fn resolve_prefers_env_var_over_stored() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        store
            .set_api_key("test-provider", "stored-key")
            .expect("save");

        // Set the env var to simulate explicit override
        let key = "NAVI_TEST_RESOLVE_KEY_12345";
        unsafe { std::env::set_var(key, "env-key") };
        let result = store.resolve_api_key("test-provider", key);
        assert_eq!(result.as_deref(), Some("env-key"));
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn resolve_falls_back_to_stored_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        store
            .set_api_key("test-provider", "stored-key")
            .expect("save");
        let result = store.resolve_api_key("test-provider", "NAVI_NONEXISTENT_ENV_VAR_98765");
        assert_eq!(result.as_deref(), Some("stored-key"));
    }

    #[cfg(unix)]
    #[test]
    fn credentials_file_and_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = CredentialStore::new(data_dir.clone());

        store.set_api_key("openai", "sk-test").expect("save");

        let dir_mode = fs::metadata(&data_dir)
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(data_dir.join("credentials.toml"))
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}
