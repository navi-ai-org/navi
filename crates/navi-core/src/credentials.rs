use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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

    pub fn get_opencode_api_key(&self) -> Option<String> {
        if let Ok(content) = std::env::var("OPENCODE_AUTH_CONTENT") {
            if let Some(key) = opencode_key_from_auth_content(&content) {
                return Some(key);
            }
        }

        opencode_auth_paths()
            .into_iter()
            .find_map(|path| opencode_key_from_auth_file(&path))
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

fn opencode_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        if !data_home.is_empty() {
            paths.push(PathBuf::from(data_home).join("opencode").join("auth.json"));
        }
    }

    if let Some(base_dirs) = BaseDirs::new() {
        paths.push(base_dirs.data_dir().join("opencode").join("auth.json"));
    }

    paths.sort();
    paths.dedup();
    paths
}

fn opencode_key_from_auth_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    opencode_key_from_auth_content(&content)
}

fn opencode_key_from_auth_content(content: &str) -> Option<String> {
    let data: Value = serde_json::from_str(content).ok()?;
    ["opencode", "opencode/"]
        .into_iter()
        .find_map(|provider_id| api_key_from_opencode_auth_entry(data.get(provider_id)?))
}

fn api_key_from_opencode_auth_entry(entry: &Value) -> Option<String> {
    if entry.get("type")?.as_str()? != "api" {
        return None;
    }

    let key = entry.get("key")?.as_str()?.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
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

    #[test]
    fn reads_opencode_api_key_from_auth_content() {
        let content = r#"{
            "opencode": {
                "type": "api",
                "key": "zen-key"
            },
            "openai": {
                "type": "api",
                "key": "openai-key"
            }
        }"#;

        assert_eq!(
            opencode_key_from_auth_content(content).as_deref(),
            Some("zen-key")
        );
    }

    #[test]
    fn ignores_non_api_opencode_auth_content() {
        let content = r#"{
            "opencode": {
                "type": "oauth",
                "access": "access-token",
                "refresh": "refresh-token",
                "expires": 999999
            }
        }"#;

        assert!(opencode_key_from_auth_content(content).is_none());
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
