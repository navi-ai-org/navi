use crate::ProviderId;
use crate::config::{ProviderConfig, canonical_provider_id, model_can_run_publicly};
use anyhow::{Context, Result};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CredentialsFile {
    #[serde(default)]
    ignored_providers: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    project_accounts: HashMap<String, HashMap<String, String>>,
    #[serde(flatten)]
    providers: HashMap<String, ProviderCredentials>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProviderCredentials {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commandcode: Option<CommandCodeCredentialMetadata>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    accounts: BTreeMap<String, CredentialAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialAccount {
    api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commandcode: Option<CommandCodeCredentialMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authenticated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth_api_kind: Option<String>,
    /// OAuth refresh token (e.g. xAI Grok CLI session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth_refresh_token: Option<String>,
    /// Unix epoch seconds when the access token expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth_expires_at: Option<i64>,
}

/// OAuth credential kinds that can be used as model API Bearer tokens.
pub const XAI_GROK_CLI_OAUTH_KIND: &str = "xai-grok-cli";

/// Returns true when an OAuth credential kind is usable for model API calls.
pub fn is_model_usable_oauth_kind(kind: &str) -> bool {
    kind == XAI_GROK_CLI_OAUTH_KIND
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCodeCredentialMetadata {
    pub user_id: String,
    pub user_name: String,
    pub key_name: String,
    pub authenticated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialAccountInfo {
    pub account_id: String,
    pub label: String,
    pub is_default: bool,
    pub is_project_selected: bool,
    pub commandcode: Option<CommandCodeCredentialMetadata>,
}

const DEFAULT_ACCOUNT_ID: &str = "default";

impl ProviderCredentials {
    fn default_account_id(&self) -> Option<String> {
        self.default_account
            .clone()
            .or_else(|| self.accounts.keys().next().cloned())
            .or_else(|| (!self.api_key.is_empty()).then(|| DEFAULT_ACCOUNT_ID.to_string()))
    }

    fn default_api_key(&self) -> Option<String> {
        self.default_account_id()
            .and_then(|account_id| self.account_api_key(&account_id))
            .or_else(|| (!self.api_key.is_empty()).then(|| self.api_key.clone()))
    }

    fn default_oauth_api_kind(&self) -> Option<String> {
        self.default_account_id()
            .and_then(|account_id| self.accounts.get(&account_id))
            .and_then(|account| account.oauth_api_kind.clone())
    }

    fn account_api_key(&self, account_id: &str) -> Option<String> {
        if account_id == DEFAULT_ACCOUNT_ID && !self.api_key.is_empty() {
            return Some(self.api_key.clone());
        }
        self.accounts
            .get(account_id)
            .map(|account| account.api_key.clone())
    }

    fn default_model_api_key(&self) -> Option<String> {
        self.default_account_id()
            .and_then(|account_id| self.account_model_api_key(&account_id))
            .or_else(|| {
                if !self.api_key.is_empty()
                    && self
                        .default_oauth_api_kind()
                        .as_deref()
                        .is_none_or(|kind| is_model_usable_oauth_kind(kind))
                {
                    Some(self.api_key.clone())
                } else {
                    None
                }
            })
    }

    fn account_model_api_key(&self, account_id: &str) -> Option<String> {
        if account_id == DEFAULT_ACCOUNT_ID
            && !self.api_key.is_empty()
            && self
                .default_oauth_api_kind()
                .as_deref()
                .is_none_or(|kind| is_model_usable_oauth_kind(kind))
        {
            return Some(self.api_key.clone());
        }
        self.accounts
            .get(account_id)
            .filter(|account| {
                account
                    .oauth_api_kind
                    .as_deref()
                    .is_none_or(is_model_usable_oauth_kind)
            })
            .map(|account| account.api_key.clone())
    }

    fn has_oauth_credential(&self) -> bool {
        self.default_oauth_api_kind().is_some()
            || self
                .accounts
                .values()
                .any(|account| account.oauth_api_kind.is_some())
    }

    /// True when the only stored credential is OAuth that cannot call model APIs.
    fn has_non_model_oauth_only(&self) -> bool {
        self.has_oauth_credential() && self.default_model_api_key().is_none()
    }

    fn default_oauth_refresh_token(&self) -> Option<String> {
        self.default_account_id()
            .and_then(|account_id| self.accounts.get(&account_id))
            .and_then(|account| account.oauth_refresh_token.clone())
    }

    fn default_oauth_expires_at(&self) -> Option<i64> {
        self.default_account_id()
            .and_then(|account_id| self.accounts.get(&account_id))
            .and_then(|account| account.oauth_expires_at)
    }

    fn default_commandcode_metadata(&self) -> Option<CommandCodeCredentialMetadata> {
        self.default_account_id()
            .and_then(|account_id| {
                self.accounts
                    .get(&account_id)
                    .and_then(|account| account.commandcode.clone())
            })
            .or_else(|| self.commandcode.clone())
    }
}

/// Where a provider's API key was resolved from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialSource {
    /// An environment variable.
    Env,
    /// NAVI's encrypted credential store on disk.
    Stored,
    /// An external auth source (e.g. OpenCode auth.json).
    External,
    /// The model is free and requires no key.
    PublicModel,
}

impl CredentialSource {
    /// Returns a lowercase string label for this source.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Env => "env",
            Self::Stored => "stored",
            Self::External => "external",
            Self::PublicModel => "public-model",
        }
    }
}

/// The resolved credential status for a provider, indicating whether a key is
/// available and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatus {
    /// Whether a usable credential was found.
    pub configured: bool,
    /// The source of the credential, if found.
    pub source: Option<CredentialSource>,
    /// Short label for display (e.g. `"env"`, `"stored"`, `"missing"`).
    pub label: String,
    /// Optional detail string (e.g. the env var name or auth path).
    pub detail: Option<String>,
}

/// Manages API key storage in a TOML credentials file at `<data_dir>/credentials.toml`.
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

    /// Returns the path to the credentials file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the stored API key for the given provider, or `None` if not found.
    pub fn get_api_key(&self, provider_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_api_key)
    }

    /// Reads a stored credential usable for model API calls.
    ///
    /// OAuth credentials obtained through OpenAI browser login are excluded:
    /// they are account/connector tokens, not OpenAI Platform API keys.
    pub fn get_model_api_key(&self, provider_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_model_api_key)
    }

    /// Returns oauth_api_kind metadata for a stored OAuth token, or `None`.
    pub fn get_oauth_api_kind(&self, provider_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_oauth_api_kind)
    }

    pub fn get_api_key_for_account(&self, provider_id: &str, account_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        let credentials = file.providers.get(&provider_id)?;
        credentials.account_api_key(account_id)
    }

    pub fn get_model_api_key_for_account(
        &self,
        provider_id: &str,
        account_id: &str,
    ) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        let credentials = file.providers.get(&provider_id)?;
        credentials.account_model_api_key(account_id)
    }

    pub fn has_oauth_credential(&self, provider_id: &str) -> bool {
        let provider_id = credential_provider_key(provider_id);
        let content = match fs::read_to_string(&self.path) {
            Ok(content) => content,
            Err(_) => return false,
        };
        let file: CredentialsFile = match toml::from_str(&content) {
            Ok(file) => file,
            Err(_) => return false,
        };
        file.providers
            .get(&provider_id)
            .is_some_and(ProviderCredentials::has_oauth_credential)
    }

    pub fn get_project_account(&self, project_dir: &Path, provider_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.project_accounts
            .get(&project_account_key(project_dir))
            .and_then(|providers| providers.get(&provider_id))
            .cloned()
    }

    pub fn set_project_account(
        &self,
        project_dir: &Path,
        provider_id: &str,
        account_id: &str,
    ) -> Result<()> {
        let provider_id = credential_provider_key(provider_id);
        ensure_private_parent_dir(&self.path)?;
        let mut file = self.load_file()?;
        let has_account = file
            .providers
            .get(&provider_id)
            .and_then(|credentials| credentials.account_api_key(account_id))
            .is_some();
        if !has_account {
            anyhow::bail!("unknown account '{account_id}' for provider '{provider_id}'");
        }
        if let Some(credentials) = file.providers.get_mut(&provider_id) {
            credentials.default_account = Some(account_id.to_string());
        }
        file.project_accounts
            .entry(project_account_key(project_dir))
            .or_default()
            .insert(provider_id, account_id.to_string());
        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)
    }

    pub fn list_credential_accounts(
        &self,
        provider_id: &str,
        project_dir: Option<&Path>,
    ) -> Result<Vec<CredentialAccountInfo>> {
        let provider_id = credential_provider_key(provider_id);
        let file = self.load_file()?;
        let Some(credentials) = file.providers.get(&provider_id) else {
            return Ok(Vec::new());
        };
        let default_account = credentials.default_account_id();
        let project_account = project_dir.and_then(|path| {
            file.project_accounts
                .get(&project_account_key(path))
                .and_then(|providers| providers.get(&provider_id))
                .cloned()
        });
        let selected_account = project_account.as_deref().or(default_account.as_deref());
        let mut accounts = credentials.accounts.clone();
        if accounts.is_empty() && !credentials.api_key.is_empty() {
            accounts.insert(
                DEFAULT_ACCOUNT_ID.to_string(),
                CredentialAccount {
                    api_key: credentials.api_key.clone(),
                    label: Some("Default".to_string()),
                    commandcode: credentials.commandcode.clone(),
                    authenticated_at: credentials
                        .commandcode
                        .as_ref()
                        .map(|metadata| metadata.authenticated_at.clone()),
                    oauth_api_kind: None,
                    oauth_refresh_token: None,
                    oauth_expires_at: None,
                },
            );
        }
        Ok(accounts
            .into_iter()
            .map(|(account_id, account)| CredentialAccountInfo {
                label: account.label.unwrap_or_else(|| account_id.clone()),
                is_default: default_account.as_deref() == Some(account_id.as_str()),
                is_project_selected: selected_account == Some(account_id.as_str()),
                commandcode: account.commandcode,
                account_id,
            })
            .collect())
    }

    /// Returns `true` if the user explicitly ignored this provider (e.g. by deleting an env-provided key).
    pub fn is_ignored(&self, provider_id: &str) -> bool {
        let provider_id = credential_provider_key(provider_id);
        let content = match fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let file: CredentialsFile = match toml::from_str(&content) {
            Ok(f) => f,
            Err(_) => return false,
        };
        file.ignored_providers.contains(&provider_id)
    }

    /// Returns the list of provider ids that have stored API keys.
    pub fn list_api_key_providers(&self) -> Result<Vec<String>> {
        let mut providers = self.load_file()?.providers.into_keys().collect::<Vec<_>>();
        providers.sort();
        Ok(providers)
    }

    /// Reads an API key from OpenCode's auth.json file, if it exists.
    pub fn get_opencode_api_key(&self) -> Option<String> {
        if let Ok(content) = std::env::var("OPENCODE_AUTH_CONTENT")
            && let Some(key) = opencode_key_from_auth_content(&content)
        {
            return Some(key);
        }

        opencode_auth_paths()
            .into_iter()
            .find_map(|path| opencode_key_from_auth_file(&path))
    }

    /// Stores an API key for the given provider, creating the credentials file
    /// if needed.
    ///
    /// **Note:** this replaces the provider's credential map with a single
    /// default account. Prefer [`Self::add_api_key_account`] for multi-account.
    pub fn set_api_key(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let provider_id = credential_provider_key(provider_id);
        ensure_private_parent_dir(&self.path)?;

        let mut file = self.load_file()?;
        file.ignored_providers.retain(|p| p != &provider_id);

        file.providers.insert(
            provider_id,
            ProviderCredentials {
                api_key: api_key.to_string(),
                commandcode: None,
                accounts: BTreeMap::from([(
                    DEFAULT_ACCOUNT_ID.to_string(),
                    CredentialAccount {
                        api_key: api_key.to_string(),
                        label: Some("Default".to_string()),
                        commandcode: None,
                        authenticated_at: None,
                        oauth_api_kind: None,
                        oauth_refresh_token: None,
                        oauth_expires_at: None,
                    },
                )]),
                default_account: Some(DEFAULT_ACCOUNT_ID.to_string()),
            },
        );

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;

        Ok(())
    }

    /// Add (or update) one API-key account without wiping sibling accounts.
    ///
    /// Returns the `account_id`. When `account_id` is `None`, a new id is generated.
    pub fn add_api_key_account(
        &self,
        provider_id: &str,
        api_key: &str,
        label: Option<&str>,
        account_id: Option<&str>,
    ) -> Result<String> {
        let provider_id = credential_provider_key(provider_id);
        let api_key = api_key.trim();
        if api_key.is_empty() {
            anyhow::bail!("api key cannot be empty");
        }
        ensure_private_parent_dir(&self.path)?;

        let mut file = self.load_file()?;
        file.ignored_providers.retain(|p| p != &provider_id);

        let credentials = file.providers.entry(provider_id.clone()).or_default();
        let id = account_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Stable-ish id from key suffix so re-adding the same key updates.
                let suffix: String = api_key
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                format!("acct-{}", suffix)
            });
        let display = label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if id == DEFAULT_ACCOUNT_ID {
                    "Default".to_string()
                } else {
                    format!("Account {id}")
                }
            });

        credentials.accounts.insert(
            id.clone(),
            CredentialAccount {
                api_key: api_key.to_string(),
                label: Some(display),
                commandcode: None,
                authenticated_at: Some(current_unix_timestamp_string()),
                oauth_api_kind: None,
                oauth_refresh_token: None,
                oauth_expires_at: None,
            },
        );
        if credentials.default_account.is_none() {
            credentials.default_account = Some(id.clone());
        }
        // Keep legacy top-level key in sync with the default account for older readers.
        if credentials.default_account.as_deref() == Some(id.as_str())
            || credentials.api_key.is_empty()
        {
            credentials.api_key = api_key.to_string();
            if credentials.default_account.is_none() {
                credentials.default_account = Some(id.clone());
            }
        }

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;
        Ok(id)
    }

    /// Set the default account for a provider (global selection).
    pub fn set_default_account(&self, provider_id: &str, account_id: &str) -> Result<()> {
        let provider_id = credential_provider_key(provider_id);
        ensure_private_parent_dir(&self.path)?;
        let mut file = self.load_file()?;
        let Some(credentials) = file.providers.get_mut(&provider_id) else {
            anyhow::bail!("unknown provider '{provider_id}'");
        };
        if credentials.account_api_key(account_id).is_none() {
            anyhow::bail!("unknown account '{account_id}' for provider '{provider_id}'");
        }
        credentials.default_account = Some(account_id.to_string());
        if let Some(key) = credentials.account_api_key(account_id) {
            credentials.api_key = key;
        }
        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)
    }
    /// Stores an API key with oauth_api_kind metadata (e.g. "chat-completions"
    /// for OAuth tokens that only work with Chat Completions API).
    pub fn set_oauth_credential(
        &self,
        provider_id: &str,
        api_key: &str,
        oauth_api_kind: &str,
    ) -> Result<()> {
        self.set_oauth_credential_full(provider_id, api_key, oauth_api_kind, None, None)
    }

    /// Stores an OAuth credential with optional refresh token and expiry.
    pub fn set_oauth_credential_full(
        &self,
        provider_id: &str,
        api_key: &str,
        oauth_api_kind: &str,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<()> {
        let provider_id = credential_provider_key(provider_id);
        ensure_private_parent_dir(&self.path)?;

        let mut file = self.load_file()?;
        file.ignored_providers.retain(|p| p != &provider_id);

        let label = if is_model_usable_oauth_kind(oauth_api_kind) {
            "Grok OAuth".to_string()
        } else {
            "Default".to_string()
        };

        file.providers.insert(
            provider_id,
            ProviderCredentials {
                api_key: api_key.to_string(),
                commandcode: None,
                accounts: BTreeMap::from([(
                    DEFAULT_ACCOUNT_ID.to_string(),
                    CredentialAccount {
                        api_key: api_key.to_string(),
                        label: Some(label),
                        commandcode: None,
                        authenticated_at: Some(current_unix_timestamp_string()),
                        oauth_api_kind: Some(oauth_api_kind.to_string()),
                        oauth_refresh_token: refresh_token
                            .filter(|value| !value.is_empty())
                            .map(str::to_string),
                        oauth_expires_at: expires_at,
                    },
                )]),
                default_account: Some(DEFAULT_ACCOUNT_ID.to_string()),
            },
        );

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;

        Ok(())
    }

    /// Returns the stored OAuth refresh token, if any.
    pub fn get_oauth_refresh_token(&self, provider_id: &str) -> Option<String> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_oauth_refresh_token)
    }

    /// Returns the stored OAuth access-token expiry (unix seconds), if any.
    pub fn get_oauth_expires_at(&self, provider_id: &str) -> Option<i64> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_oauth_expires_at)
    }

    /// Stores a Command Code OAuth-created API key and callback metadata.
    pub fn set_commandcode_credential(
        &self,
        provider_id: &str,
        api_key: &str,
        metadata: CommandCodeCredentialMetadata,
    ) -> Result<String> {
        let provider_id = credential_provider_key(provider_id);
        ensure_private_parent_dir(&self.path)?;

        let mut file = self.load_file()?;
        file.ignored_providers.retain(|p| p != &provider_id);
        let account_id = commandcode_account_id(&metadata);
        let credentials = file.providers.entry(provider_id).or_default();
        credentials.accounts.insert(
            account_id.clone(),
            CredentialAccount {
                api_key: api_key.to_string(),
                label: Some(commandcode_account_label(&metadata)),
                authenticated_at: Some(metadata.authenticated_at.clone()),
                commandcode: Some(metadata),
                oauth_api_kind: None,
                oauth_refresh_token: None,
                oauth_expires_at: None,
            },
        );
        if credentials.default_account.is_none() && credentials.api_key.is_empty() {
            credentials.default_account = Some(account_id.clone());
        }

        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;
        Ok(account_id)
    }

    pub fn get_commandcode_metadata(
        &self,
        provider_id: &str,
    ) -> Option<CommandCodeCredentialMetadata> {
        let provider_id = credential_provider_key(provider_id);
        let content = fs::read_to_string(&self.path).ok()?;
        let file: CredentialsFile = toml::from_str(&content).ok()?;
        file.providers
            .get(&provider_id)
            .and_then(ProviderCredentials::default_commandcode_metadata)
    }

    pub fn delete_credential_account(&self, provider_id: &str, account_id: &str) -> Result<bool> {
        let provider_id = credential_provider_key(provider_id);
        let mut file = self.load_file().unwrap_or_default();
        let Some(credentials) = file.providers.get_mut(&provider_id) else {
            return Ok(false);
        };
        let removed = credentials.accounts.remove(account_id).is_some();
        if !removed {
            return Ok(false);
        }
        if credentials.default_account.as_deref() == Some(account_id) {
            credentials.default_account = credentials.accounts.keys().next().cloned();
        }
        for providers in file.project_accounts.values_mut() {
            if providers
                .get(&provider_id)
                .is_some_and(|selected| selected == account_id)
            {
                providers.remove(&provider_id);
            }
        }
        ensure_private_parent_dir(&self.path)?;
        let content = toml::to_string_pretty(&file).context("failed to serialize credentials")?;
        self.write_content(&content)?;
        Ok(true)
    }

    /// Deletes a stored API key and adds the provider to the ignored list.
    /// Returns `true` if a stored key was removed.
    pub fn delete_api_key(&self, provider_id: &str) -> Result<bool> {
        let provider_id = credential_provider_key(provider_id);
        let mut file = self.load_file().unwrap_or_default();
        let removed = file.providers.remove(&provider_id).is_some();
        for providers in file.project_accounts.values_mut() {
            providers.remove(&provider_id);
        }
        let mut ignored = false;
        if !file.ignored_providers.contains(&provider_id) {
            file.ignored_providers.push(provider_id);
            ignored = true;
        }

        if removed || ignored {
            ensure_private_parent_dir(&self.path)?;
            let content =
                toml::to_string_pretty(&file).context("failed to serialize credentials")?;
            self.write_content(&content)?;
        }
        Ok(removed)
    }

    /// Resolve API key for a provider: env var first (explicit override), then stored credential.
    /// Resolves an API key by checking the environment variable first, then
    /// the stored credential. Returns `None` if neither is set.
    pub fn resolve_api_key(&self, provider_id: &str, env_var: &str) -> Option<String> {
        if let Ok(key) = std::env::var(env_var)
            && !key.is_empty()
        {
            return Some(key);
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

/// Resolves the API key for a provider by id, using the configured env var from
/// the provider's `ProviderConfig`. Returns `None` if no key is found.
pub fn resolve_provider_api_key(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
) -> Option<String> {
    if credential_store.is_ignored(&provider_config.id) {
        return None;
    }

    provider_env_api_key_for_config(provider_config)
        .or_else(|| opencode_auth_json_api_key(credential_store, &provider_config.id))
        .or_else(|| credential_store.get_model_api_key(&provider_config.id))
        .or_else(|| {
            if requested_provider_id != provider_config.id {
                credential_store.get_model_api_key(requested_provider_id)
            } else {
                None
            }
        })
        // Fallback: reuse xAI CLI session (~/.grok/auth.json) when NAVI
        // has no stored/env credential yet.
        .or_else(|| grok_auth_json_access_token(credential_store, &provider_config.id))
        .or_else(|| {
            if requested_provider_id != provider_config.id {
                grok_auth_json_access_token(credential_store, requested_provider_id)
            } else {
                None
            }
        })
}

pub fn resolve_provider_api_key_for_project(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
    project_dir: &Path,
) -> Option<String> {
    if credential_store.is_ignored(&provider_config.id) {
        return None;
    }

    credential_store
        .get_project_account(project_dir, &provider_config.id)
        .and_then(|account_id| {
            credential_store.get_model_api_key_for_account(&provider_config.id, &account_id)
        })
        .or_else(|| {
            if requested_provider_id != provider_config.id {
                credential_store
                    .get_project_account(project_dir, requested_provider_id)
                    .and_then(|account_id| {
                        credential_store
                            .get_model_api_key_for_account(requested_provider_id, &account_id)
                    })
            } else {
                None
            }
        })
        .or_else(|| {
            resolve_provider_api_key(credential_store, provider_config, requested_provider_id)
        })
}

/// Resolves the full [`CredentialStatus`] for a provider, including source
/// label, env var name, and whether the model is public.
pub fn resolve_provider_credential_status(
    credential_store: &CredentialStore,
    provider_config: &ProviderConfig,
    requested_provider_id: &str,
    model: Option<&str>,
) -> CredentialStatus {
    if credential_store.is_ignored(&provider_config.id) {
        return CredentialStatus {
            configured: false,
            source: None,
            label: "ignored".to_string(),
            detail: Some("disabled by user".to_string()),
        };
    }

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

    if credential_store
        .get_model_api_key(&provider_config.id)
        .is_some()
        || (requested_provider_id != provider_config.id
            && credential_store
                .get_model_api_key(requested_provider_id)
                .is_some())
    {
        let oauth_kind = credential_store
            .get_oauth_api_kind(&provider_config.id)
            .or_else(|| {
                (requested_provider_id != provider_config.id)
                    .then(|| credential_store.get_oauth_api_kind(requested_provider_id))
                    .flatten()
            });
        let (label, detail) = if oauth_kind.as_deref() == Some(XAI_GROK_CLI_OAUTH_KIND) {
            ("oauth".to_string(), Some("xAI Grok OAuth".to_string()))
        } else {
            ("stored".to_string(), Some("stored credential".to_string()))
        };
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::Stored),
            label,
            detail,
        };
    }

    if is_xai_provider_id(&provider_config.id)
        && grok_auth_json_access_token(credential_store, &provider_config.id).is_some()
    {
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::External),
            label: "grok".to_string(),
            detail: Some("Grok CLI auth.json".to_string()),
        };
    }

    if is_non_model_oauth_only(credential_store, &provider_config.id)
        || (requested_provider_id != provider_config.id
            && is_non_model_oauth_only(credential_store, requested_provider_id))
    {
        return CredentialStatus {
            configured: false,
            source: None,
            label: "oauth-only".to_string(),
            detail: Some(
                "stored OAuth credential is not usable for model API calls; configure an API key"
                    .to_string(),
            ),
        };
    }

    if let Some(model) = model
        && (model_can_run_publicly(requested_provider_id, model)
            || model_can_run_publicly(&provider_config.id, model))
    {
        return CredentialStatus {
            configured: true,
            source: Some(CredentialSource::PublicModel),
            label: "public".to_string(),
            detail: Some("free model access without key".to_string()),
        };
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
    if provider_config.id == ProviderId::COMMANDCODE {
        let mut env_vars = vec![
            "COMMAND_CODE_API_KEY".to_string(),
            "CMD_API_KEY".to_string(),
        ];
        if !env_vars
            .iter()
            .any(|env_var| env_var == &provider_config.api_key_env)
        {
            env_vars.push(provider_config.api_key_env.clone());
        }
        return env_vars;
    }

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

fn is_non_model_oauth_only(credential_store: &CredentialStore, provider_id: &str) -> bool {
    let provider_id = credential_provider_key(provider_id);
    let content = match fs::read_to_string(credential_store.path()) {
        Ok(content) => content,
        Err(_) => return false,
    };
    let file: CredentialsFile = match toml::from_str(&content) {
        Ok(file) => file,
        Err(_) => return false,
    };
    file.providers
        .get(&provider_id)
        .is_some_and(ProviderCredentials::has_non_model_oauth_only)
}

fn is_xai_provider_id(provider_id: &str) -> bool {
    credential_provider_key(provider_id) == ProviderId::XAI
}

/// Reads a still-valid access token from Grok CLI's `~/.grok/auth.json`.
fn grok_auth_json_access_token(
    credential_store: &CredentialStore,
    provider_id: &str,
) -> Option<String> {
    if !is_xai_provider_id(provider_id) || credential_store.is_ignored(provider_id) {
        return None;
    }

    if let Ok(content) = std::env::var("GROK_AUTH_CONTENT")
        && let Some(key) = grok_key_from_auth_content(&content)
    {
        return Some(key);
    }

    grok_auth_paths()
        .into_iter()
        .find_map(|path| grok_key_from_auth_file(&path))
}

fn grok_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        paths.push(PathBuf::from(home).join(".grok").join("auth.json"));
    }
    if let Some(base_dirs) = BaseDirs::new() {
        paths.push(base_dirs.home_dir().join(".grok").join("auth.json"));
    }
    paths.sort();
    paths.dedup();
    paths
}

fn grok_key_from_auth_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    grok_key_from_auth_content(&content)
}

fn grok_key_from_auth_content(content: &str) -> Option<String> {
    let data: Value = serde_json::from_str(content).ok()?;
    let now = current_unix_timestamp_secs();
    let mut best: Option<(i64, String)> = None;

    for (key, entry) in data.as_object()? {
        if !key.contains("auth.x.ai") {
            continue;
        }
        let Some(access) = entry
            .get("key")
            .or_else(|| entry.get("access_token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let expires_at = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_to_unix)
            .unwrap_or(i64::MAX);

        if expires_at + 300 < now {
            continue;
        }

        match &best {
            Some((best_exp, _)) if *best_exp >= expires_at => {}
            _ => best = Some((expires_at, access.to_string())),
        }
    }

    best.map(|(_, token)| token)
}

fn parse_rfc3339_to_unix(value: &str) -> Option<i64> {
    let trimmed = value.trim().trim_end_matches('Z');
    let (date, time) = trimmed.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let time = time.split(['.', '+']).next()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn current_unix_timestamp_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn current_unix_timestamp_string() -> String {
    current_unix_timestamp_secs().to_string()
}

fn project_account_key(project_dir: &Path) -> String {
    project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn credential_provider_key(provider_id: &str) -> String {
    canonical_provider_id(provider_id).to_string()
}

fn commandcode_account_id(metadata: &CommandCodeCredentialMetadata) -> String {
    let base = if metadata.user_name.trim().is_empty() {
        format!("{}-{}", metadata.user_id, metadata.key_name)
    } else {
        format!("{}-{}", metadata.user_name, metadata.key_name)
    };
    slugify_account_id(&base)
}

fn commandcode_account_label(metadata: &CommandCodeCredentialMetadata) -> String {
    if metadata.key_name.trim().is_empty() {
        metadata.user_name.clone()
    } else if metadata.user_name.trim().is_empty() {
        metadata.key_name.clone()
    } else {
        format!("{} ({})", metadata.user_name, metadata.key_name)
    }
}

fn slugify_account_id(value: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        DEFAULT_ACCOUNT_ID.to_string()
    } else {
        slug
    }
}

fn provider_env_api_key(env_var: &str) -> Option<String> {
    let key = std::env::var(env_var).ok()?;
    if key.is_empty() { None } else { Some(key) }
}

fn opencode_auth_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(data_home) = std::env::var("XDG_DATA_HOME")
        && !data_home.is_empty()
    {
        paths.push(PathBuf::from(data_home).join("opencode").join("auth.json"));
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

fn ensure_private_parent_dir(path: &Path) -> Result<()> {
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
    fn openai_oauth_credential_does_not_resolve_as_model_api_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "openai".to_string(),
            label: "OpenAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98771".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            ..Default::default()
        };

        store
            .set_oauth_credential("openai", "oauth-access-token", "chat-completions")
            .expect("save oauth");

        assert_eq!(
            store.get_api_key("openai").as_deref(),
            Some("oauth-access-token")
        );
        assert!(store.get_model_api_key("openai").is_none());
        assert!(resolve_provider_api_key(&store, &provider, "openai").is_none());

        let status = resolve_provider_credential_status(&store, &provider, "openai", None);
        assert!(!status.configured);
        assert_eq!(status.label, "oauth-only");
    }

    #[test]
    fn xai_grok_oauth_credential_resolves_as_model_api_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "xai".to_string(),
            label: "xAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_XAI_OAUTH".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            ..Default::default()
        };

        store
            .set_oauth_credential_full(
                "xai",
                "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig",
                XAI_GROK_CLI_OAUTH_KIND,
                Some("refresh-token-xyz"),
                Some(9_999_999_999),
            )
            .expect("save oauth");

        assert_eq!(
            store.get_model_api_key("xai").as_deref(),
            Some("eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.sig")
        );
        assert_eq!(
            store.get_oauth_api_kind("xai").as_deref(),
            Some(XAI_GROK_CLI_OAUTH_KIND)
        );
        assert_eq!(
            store.get_oauth_refresh_token("xai").as_deref(),
            Some("refresh-token-xyz")
        );
        assert!(resolve_provider_api_key(&store, &provider, "xai").is_some());

        let status = resolve_provider_credential_status(&store, &provider, "xai", None);
        assert!(status.configured);
        assert_eq!(status.label, "oauth");
    }

    #[test]
    fn grok_auth_json_content_is_used_as_external_xai_credential() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: "xai".to_string(),
            label: "xAI".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiResponses,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_XAI_GROK".to_string(),
            base_url: Some("https://api.x.ai/v1".to_string()),
            ..Default::default()
        };

        let far_future = "2099-01-01T00:00:00Z";
        let content = format!(
            r#"{{
                "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {{
                    "key": "eyJ.test.token",
                    "auth_mode": "oidc",
                    "expires_at": "{far_future}",
                    "oidc_issuer": "https://auth.x.ai"
                }}
            }}"#
        );
        // SAFETY: test-only env mutation.
        unsafe { std::env::set_var("GROK_AUTH_CONTENT", &content) };
        let key = resolve_provider_api_key(&store, &provider, "xai");
        let status = resolve_provider_credential_status(&store, &provider, "xai", None);
        unsafe { std::env::remove_var("GROK_AUTH_CONTENT") };

        assert_eq!(key.as_deref(), Some("eyJ.test.token"));
        assert!(status.configured);
        assert_eq!(status.label, "grok");
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

    #[test]
    fn stores_commandcode_oauth_metadata_in_navi_credentials() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let metadata = CommandCodeCredentialMetadata {
            user_id: "user-1".to_string(),
            user_name: "test-user".to_string(),
            key_name: "NAVI".to_string(),
            authenticated_at: "123".to_string(),
        };

        store
            .set_commandcode_credential(ProviderId::COMMANDCODE, "cmd-key", metadata.clone())
            .expect("save commandcode credential");

        assert_eq!(
            store.get_api_key(ProviderId::COMMANDCODE).as_deref(),
            Some("cmd-key")
        );
        assert_eq!(
            store.get_commandcode_metadata(ProviderId::COMMANDCODE),
            Some(metadata)
        );
    }

    #[test]
    fn commandcode_provider_checks_cli_env_aliases() {
        let provider = ProviderConfig {
            id: ProviderId::COMMANDCODE.to_string(),
            label: "Command Code".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "CMD_API_KEY".to_string(),
            base_url: Some("https://api.commandcode.ai".to_string()),
            ..Default::default()
        };

        assert_eq!(
            provider_env_vars_for_config(&provider),
            vec![
                "COMMAND_CODE_API_KEY".to_string(),
                "CMD_API_KEY".to_string()
            ]
        );
    }

    #[test]
    fn provider_resolver_uses_stored_commandcode_oauth_key() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = CredentialStore::new(tempdir.path().to_path_buf());
        let provider = ProviderConfig {
            id: ProviderId::COMMANDCODE.to_string(),
            label: "Command Code".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98770".to_string(),
            base_url: Some("https://api.commandcode.ai".to_string()),
            ..Default::default()
        };

        store
            .set_commandcode_credential(
                ProviderId::COMMANDCODE,
                "cmd-stored-key",
                CommandCodeCredentialMetadata {
                    user_id: "user-1".to_string(),
                    user_name: "test-user".to_string(),
                    key_name: "NAVI".to_string(),
                    authenticated_at: "123".to_string(),
                },
            )
            .expect("save commandcode credential");
        let result = resolve_provider_api_key(&store, &provider, ProviderId::COMMANDCODE);
        let expected = std::env::var("COMMAND_CODE_API_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .or_else(|| {
                std::env::var("CMD_API_KEY")
                    .ok()
                    .filter(|key| !key.is_empty())
            })
            .unwrap_or_else(|| "cmd-stored-key".to_string());

        assert_eq!(result.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn selected_provider_account_persists_as_project_and_default_account() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("data");
        let project_dir = tempdir.path().join("project");
        fs::create_dir_all(&project_dir).expect("project dir");
        let store = CredentialStore::new(data_dir.clone());
        let provider = ProviderConfig {
            id: ProviderId::COMMANDCODE.to_string(),
            label: "Command Code".to_string(),
            description: String::new(),
            kind: ProviderKind::OpenAiChatCompletions,
            api_key_env: "NAVI_NONEXISTENT_ENV_VAR_98772".to_string(),
            base_url: Some("https://api.commandcode.ai".to_string()),
            ..Default::default()
        };

        let first = store
            .set_commandcode_credential(
                ProviderId::COMMANDCODE,
                "cmd-first-key",
                CommandCodeCredentialMetadata {
                    user_id: "user-1".to_string(),
                    user_name: "first-user".to_string(),
                    key_name: "NAVI".to_string(),
                    authenticated_at: "123".to_string(),
                },
            )
            .expect("save first commandcode credential");
        let second = store
            .set_commandcode_credential(
                ProviderId::COMMANDCODE,
                "cmd-second-key",
                CommandCodeCredentialMetadata {
                    user_id: "user-2".to_string(),
                    user_name: "second-user".to_string(),
                    key_name: "NAVI".to_string(),
                    authenticated_at: "456".to_string(),
                },
            )
            .expect("save second commandcode credential");

        assert_ne!(first, second);
        store
            .set_project_account(&project_dir, ProviderId::COMMANDCODE, &second)
            .expect("select project account");

        let reopened = CredentialStore::new(data_dir);
        assert_eq!(
            reopened.get_project_account(&project_dir, ProviderId::COMMANDCODE),
            Some(second.clone())
        );
        assert_eq!(
            reopened.get_api_key(ProviderId::COMMANDCODE).as_deref(),
            Some("cmd-second-key")
        );
        assert_eq!(
            resolve_provider_api_key_for_project(
                &reopened,
                &provider,
                ProviderId::COMMANDCODE,
                &project_dir
            )
            .as_deref(),
            Some("cmd-second-key")
        );

        let accounts = reopened
            .list_credential_accounts(ProviderId::COMMANDCODE, Some(&project_dir))
            .expect("list accounts");
        assert!(
            accounts
                .iter()
                .any(|account| account.account_id == second && account.is_project_selected)
        );
        let accounts_without_project = reopened
            .list_credential_accounts(ProviderId::COMMANDCODE, None)
            .expect("list accounts without project");
        assert!(
            accounts_without_project
                .iter()
                .any(|account| account.account_id == second && account.is_project_selected)
        );
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

    // ── Regression tests ──────────────────────────────────────────────────────

    #[test]
    fn regression_corrupt_credentials_file_returns_none() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = CredentialStore::new(data_dir.clone());

        // Write corrupt TOML
        fs::create_dir_all(&data_dir).expect("create");
        fs::write(data_dir.join("credentials.toml"), "{not valid toml!!!").expect("write");

        let key = store.get_api_key("openai");
        assert!(key.is_none(), "corrupt credentials must return None");
    }

    #[test]
    fn regression_delete_api_key_missing_file_returns_false() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = CredentialStore::new(data_dir);

        let result = store.delete_api_key("openai").expect("delete");
        assert!(!result, "deleting from missing file should return false");
    }

    #[test]
    fn regression_empty_env_var_falls_through() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = CredentialStore::new(data_dir);

        // Store a key
        store.set_api_key("openai", "sk-stored").expect("save");

        // Set env var to empty
        unsafe { std::env::set_var("OPENAI_API_KEY", "") };
        let key = store.resolve_api_key("openai", "OPENAI_API_KEY");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };

        // Empty env var should fall through to stored key
        assert_eq!(key.as_deref(), Some("sk-stored"));
    }

    #[test]
    fn regression_set_empty_api_key_stores_empty() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let data_dir = tempdir.path().join("navi-data");
        let store = CredentialStore::new(data_dir);

        store.set_api_key("openai", "").expect("save");
        // The store doesn't validate non-empty, so empty is stored
        // This is a known behavior - callers should validate
        let key = store.get_api_key("openai");
        assert_eq!(key.as_deref(), Some(""));
    }

    #[test]
    fn regression_opencode_key_from_invalid_json_returns_none() {
        let result = opencode_key_from_auth_content("{not json");
        assert!(result.is_none());
    }
}
