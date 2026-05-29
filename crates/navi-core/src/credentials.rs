use crate::config::{ProviderConfig, model_can_run_publicly};
use crate::ProviderId;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialSource {
    Env,
    Stored,
    External,
    PublicModel,
}

impl CredentialSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Stored => "stored",
            Self::External => "external",
            Self::PublicModel => "public-model",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    pub configured: bool,
    pub source: Option<CredentialSource>,
    pub label: String,
    pub detail: Option<String>,
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get_api_key(&self, provider_id: &str) -> Option<String> {
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers.get(provider_id).map(|c| c.api_key.clone())
    }

    pub fn list_api_key_providers(&self) -> Result<Vec<String>> {
        let mut providers = self.load_file()?.providers.into_keys().collect::<Vec<_>>();
        providers.sort();
        Ok(providers)
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

        let mut file = self.load_file()?;

        file.providers.insert(
            provider_id.to_string(),
            ProviderCredentials {
                api_key: api_key.to_string(),
            },
        );

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;

        Ok(())
    }

    pub fn delete_api_key(&self, provider_id: &str) -> Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }

        ensure_private_parent_dir(&self.path)?;
        let mut file = self.load_file()?;
        let removed = file.providers.remove(provider_id).is_some();
        if removed {
            let content =
                toml::to_string_pretty(&file).context("failed to serialize credentials")?;
            self.write_content(&content)?;
        }
        Ok(removed)
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

    fn load_file(&self) -> Result<CredentialsFile> {
        if !self.path.exists() {
            return Ok(CredentialsFile::default());
        }

        let content = fs::read_to_string(&self.path).context("failed to read credentials file")?;
        Ok(toml::from_str(&content).unwrap_or_default())
    }

    fn write_content(&self, content: &str) -> Result<()> {
        fs::write(&self.path, content)
            .with_context(|| format!("failed to write {}", self.path.display()))?;

        // Restrict permissions so only the owner can read the credentials file.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }
}

pub fn resolve_provider_api_key(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
) -> Option<String> {
    provider_env_api_key_for_config(provider_config)
        .or_else(|| opencode_auth_json_api_key(credential_store, &provider_config.id))
        .or_else(|| credential_store.get_api_key(&provider_config.id))
        .or_else(|| {
            if requested_provider_id != provider_config.id {
                credential_store.get_api_key(requested_provider_id)
            } else {
                None
            }
        })
}

pub fn resolve_provider_credential_status(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
    model: Option<&str>,
) -> CredentialStatus {
    if let Some(env_var) = provider_env_var_for_config(provider_config) {
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::Env),
            label: "env".to_string(),
            detail: Some(env_var),
        };
    }

    if ProviderId::from_config_id(&provider_config.id).is_opencode_family()
        && credential_store.get_opencode_api_key().is_some()
    {
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::External),
            label: "opencode".to_string(),
            detail: Some("OpenCode auth.json".to_string()),
        };
    }

    if credential_store.get_api_key(&provider_config.id).is_some()
        || (requested_provider_id != provider_config.id
            && credential_store
                .get_api_key(requested_provider_id)
                .is_some())
    {
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::Stored),
            label: "stored".to_string(),
            detail: Some("stored credential".to_string()),
        };
    }

    if let Some(model) = model {
        if model_can_run_publicly(requested_provider_id, model)
            || model_can_run_publicly(&provider_config.id, model)
        {
            return CredentialStatus {
                configured: true,
                source: Some(CredentialSource::PublicModel),
                label: "public".to_string(),
                detail: Some("free model access without key".to_string()),
            };
        }
    }

    CredentialStatus {
        configured: false,
        source: None,
        label: "missing".to_string(),
        detail: None,
    }
}

fn provider_env_api_key_for_config(provider_config: &ProviderConfig) -> Option<String> {
    provider_env_var_for_config(provider_config).and_then(|env_var| provider_env_api_key(&env_var))
}

fn provider_env_var_for_config(provider_config: &ProviderConfig) -> Option<String> {
    provider_env_vars_for_config(provider_config)
        .into_iter()
        .find(|env_var| provider_env_api_key(env_var).is_some())
}

fn provider_env_vars_for_config(provider_config: &ProviderConfig) -> Vec<String> {
    if !ProviderId::from_config_id(&provider_config.id).is_opencode_family() {
        return vec![provider_config.api_key_env.clone()];
    }

    let mut env_vars = vec![
        "OPENCODE_API_KEY".to_string(),
        "OPENCODE_ZEN_API_KEY".to_string(),
    ];
    if !env_vars
        .iter()
        .any(|env_var| env_var == &provider_config.api_key_env)
    {
        env_vars.push(provider_config.api_key_env.clone());
    }
    env_vars
}

fn opencode_auth_json_api_key(
    credential_store: &CredentialStore,
    provider_id: &str,
) -> Option<String> {
    if ProviderId::from_config_id(provider_id).is_opencode_family() {
        credential_store.get_opencode_api_key()
    } else {
        None
    }
}

fn provider_env_api_key(env_var: &str) -> Option<String> {
    let key = std::env::var(env_var).ok()?;
    if key.is_empty() { None } else { Some(key) }
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
    use crate::config::{ProviderConfig, ProviderKind};

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
    fn lists_and_deletes_stored_provider_ids_without_keys() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());

        store.set_api_key("z-provider", "sk-z").expect("save");
        store.set_api_key("a-provider", "sk-a").expect("save");

        assert_eq!(
            store.list_api_key_providers().expect("list"),
            vec!["a-provider".to_string(), "z-provider".to_string()]
        );
        assert!(store.delete_api_key("a-provider").expect("delete"));
        assert!(!store.delete_api_key("missing-provider").expect("delete"));
        assert_eq!(
            store.list_api_key_providers().expect("list"),
            vec!["z-provider".to_string()]
        );
        assert_eq!(store.get_api_key("z-provider").as_deref(), Some("sk-z"));
        assert!(store.get_api_key("a-provider").is_none());
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
    fn provider_resolver_falls_back_to_store_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98766".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            ..Default::default()
        };

        store.set_api_key("openai", "stored-openai").expect("save");

        let result = resolve_provider_api_key(&store, &provider, "openai");
        assert_eq!(result.as_deref(), Some("stored-openai"));
    }

    #[test]
    fn provider_resolver_checks_requested_alias_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "custom-provider".to_string(),
            label: "Custom Provider".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98767".to_string(),
            base_url: Some("https://example.test/v1".to_string()),
            ..Default::default()
        };

        store
            .set_api_key("custom-provider-alias", "stored-alias")
            .expect("save");

        let result = resolve_provider_api_key(&store, &provider, "custom-provider-alias");
        assert_eq!(result.as_deref(), Some("stored-alias"));
    }

    #[test]
    fn provider_status_reports_stored_and_missing_credentials() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98768".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            ..Default::default()
        };

        let missing = resolve_provider_credential_status(&store, &provider, "openai", None);
        assert!(!missing.configured);
        assert_eq!(missing.label, "missing");

        store.set_api_key("openai", "stored-openai").expect("save");
        let stored = resolve_provider_credential_status(&store, &provider, "openai", None);
        assert!(stored.configured);
        assert_eq!(stored.source, Some(CredentialSource::Stored));
        assert_eq!(stored.label, "stored");
    }

    #[test]
    fn provider_status_reports_public_model_access() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "public-test".to_string(),
            label: "OpenCode".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98769".to_string(),
            base_url: None,
            ..Default::default()
        };

        let status = resolve_provider_credential_status(
            &store,
            &provider,
            "opencode",
            Some("deepseek-v4-flash-free"),
        );
        assert!(status.configured);
        assert!(matches!(
            status.source,
            Some(CredentialSource::External) | Some(CredentialSource::PublicModel)
        ));
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
