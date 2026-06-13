//! Local SQLite cache for the provider registry.

use crate::config::types::{ModelTaskSize, ProviderConfig, ProviderKind, ProviderModelConfig};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

use super::types::{RegistryManifest, RegistryProvider};

/// SQLite-backed registry store.
///
/// Thread-safe via internal `Mutex<Connection>` — registry operations are
/// short-lived and infrequent so contention is negligible.
pub struct RegistryStore {
    conn: Mutex<Connection>,
}

impl RegistryStore {
    /// Opens (or creates) the registry database at `<data_dir>/registry.db`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("failed to create data dir {}", data_dir.display()))?;
        let db_path = data_dir.join("registry.db");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open registry DB at {}", db_path.display()))?;

        // WAL mode for concurrent reads, faster writes.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Opens an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS registry_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS providers (
                id              TEXT PRIMARY KEY,
                label           TEXT NOT NULL,
                description     TEXT NOT NULL DEFAULT '',
                kind            TEXT NOT NULL,
                api_key_env     TEXT NOT NULL,
                base_url        TEXT,
                request_options TEXT NOT NULL DEFAULT '{}',
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS models (
                provider_id         TEXT NOT NULL,
                name                TEXT NOT NULL,
                task_size           TEXT NOT NULL,
                context_window_tokens INTEGER,
                tool_prompt_manifest INTEGER,
                PRIMARY KEY (provider_id, name),
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            );
            ",
        )?;
        ensure_provider_request_options_column(&conn)?;
        Ok(())
    }

    // ── Metadata helpers ──────────────────────────────────────────────────

    /// Returns the value of a metadata key, or `None`.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT value FROM registry_meta WHERE key = ?1")
            .context("prepare meta_get")?;
        let mut rows = stmt.query_map(params![key], |row| row.get(0))?;
        match rows.next() {
            Some(Ok(v)) => Ok(Some(v)),
            _ => Ok(None),
        }
    }

    /// Sets a metadata key-value pair (upsert).
    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO registry_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // ── Provider / model CRUD ─────────────────────────────────────────────

    /// Returns `true` if the providers table is empty.
    pub fn is_empty(&self) -> Result<bool> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    /// Returns the number of providers in the cache.
    pub fn provider_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM providers", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Returns the total number of models across all providers.
    pub fn model_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM models", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Upserts a provider and its models from a [`RegistryProvider`].
    pub fn upsert_provider(&self, provider: &RegistryProvider) -> Result<()> {
        let kind = parse_provider_kind(&provider.kind);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "INSERT OR REPLACE INTO providers (id, label, description, kind, api_key_env, base_url, request_options, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            params![
                provider.id,
                provider.label,
                provider.description,
                provider.kind,
                provider.api_key_env,
                provider.base_url,
                serde_json::to_string(&provider.request_options)?,
            ],
        )?;

        // Delete existing models for this provider, then re-insert.
        tx.execute(
            "DELETE FROM models WHERE provider_id = ?1",
            params![provider.id],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO models (provider_id, name, task_size, context_window_tokens, tool_prompt_manifest)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
            )?;

            for model in &provider.models {
                stmt.execute(params![
                    provider.id,
                    model.name,
                    model.task_size,
                    model.context_window_tokens.map(|v| v as i64),
                ])?;
            }
        }

        tx.commit()?;
        let _ = kind; // used above for validation if needed
        Ok(())
    }

    /// Loads all providers from the cache as [`ProviderConfig`] values
    /// compatible with the existing catalog.
    pub fn load_all_providers(&self) -> Result<Vec<ProviderConfig>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        let mut stmt = conn.prepare(
            "SELECT id, label, description, kind, api_key_env, base_url, request_options FROM providers ORDER BY id",
        )?;

        let provider_rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut providers = Vec::new();

        for row in provider_rows {
            let (id, label, description, kind_str, api_key_env, base_url, request_options_json) =
                row?;
            let kind = parse_provider_kind(&kind_str);
            let request_options = serde_json::from_str(&request_options_json).ok();

            let mut model_stmt = conn.prepare(
                "SELECT name, task_size, context_window_tokens, tool_prompt_manifest
                 FROM models WHERE provider_id = ?1 ORDER BY rowid",
            )?;

            let models = model_stmt
                .query_map(params![id], |row| {
                    let name: String = row.get(0)?;
                    let task_size_str: String = row.get(1)?;
                    let ctx: Option<i64> = row.get(2)?;
                    let tpm: Option<i64> = row.get(3)?;

                    Ok(ProviderModelConfig {
                        name,
                        task_size: match task_size_str.as_str() {
                            "small" => ModelTaskSize::Small,
                            _ => ModelTaskSize::Large,
                        },
                        context_window_tokens: ctx.map(|v| v as u64),
                        tool_prompt_manifest: tpm.map(|v| v != 0),
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            providers.push(ProviderConfig {
                id,
                label,
                description,
                kind,
                api_key_env,
                base_url,
                models,
                request_options,
                ..Default::default()
            });
        }

        Ok(providers)
    }

    /// Replaces the entire cache with the given providers (full refresh).
    pub fn replace_all(&self, providers: &[RegistryProvider]) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // Wipe existing data.
        conn.execute("DELETE FROM models", [])?;
        conn.execute("DELETE FROM providers", [])?;

        drop(conn); // release lock before per-provider upsert

        for provider in providers {
            self.upsert_provider(provider)?;
        }

        Ok(())
    }

    /// Saves the manifest metadata.
    pub fn save_manifest_meta(&self, manifest: &RegistryManifest) -> Result<()> {
        self.meta_set("manifest_version", &manifest.version.to_string())?;
        self.meta_set("manifest_updated_at", &manifest.updated_at)?;
        self.meta_set(
            "manifest_provider_count",
            &manifest.providers.len().to_string(),
        )?;
        Ok(())
    }

    /// Returns the stored manifest version, if any.
    pub fn manifest_version(&self) -> Result<Option<u32>> {
        match self.meta_get("manifest_version")? {
            Some(v) => Ok(v.parse().ok()),
            None => Ok(None),
        }
    }

    /// Returns the stored manifest `updated_at`, if any.
    pub fn manifest_updated_at(&self) -> Result<Option<String>> {
        self.meta_get("manifest_updated_at")
    }
}

fn parse_provider_kind(s: &str) -> ProviderKind {
    match s {
        "openai-responses" => ProviderKind::OpenAiResponses,
        "openai-chat-completions" => ProviderKind::OpenAiChatCompletions,
        "anthropic-messages" => ProviderKind::AnthropicMessages,
        "gemini-generate-content" => ProviderKind::GeminiGenerateContent,
        _ => ProviderKind::OpenAiChatCompletions,
    }
}

fn ensure_provider_request_options_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| matches!(name, Ok(name) if name == "request_options"));

    if !has_column {
        conn.execute(
            "ALTER TABLE providers ADD COLUMN request_options TEXT NOT NULL DEFAULT '{}'",
            [],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::config::types::ProviderRequestOptions;

    use super::*;
    use crate::registry::types::RegistryModel;

    fn sample_provider() -> RegistryProvider {
        RegistryProvider {
            id: "test-provider".to_string(),
            label: "Test Provider".to_string(),
            description: "A test".to_string(),
            kind: "openai-chat-completions".to_string(),
            api_key_env: "TEST_API_KEY".to_string(),
            base_url: Some("https://api.test.com/v1".to_string()),
            request_options: Default::default(),
            models: vec![
                RegistryModel {
                    name: "test-model-large".to_string(),
                    task_size: "large".to_string(),
                    context_window_tokens: Some(200_000),
                },
                RegistryModel {
                    name: "test-model-small".to_string(),
                    task_size: "small".to_string(),
                    context_window_tokens: Some(128_000),
                },
            ],
        }
    }

    #[test]
    fn open_and_init_schema() {
        let store = RegistryStore::open_memory().expect("open");
        assert!(store.is_empty().unwrap());
    }

    #[test]
    fn upsert_and_load_provider() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = sample_provider();
        store.upsert_provider(&provider).expect("upsert");

        assert_eq!(store.provider_count().unwrap(), 1);
        assert_eq!(store.model_count().unwrap(), 2);

        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "test-provider");
        assert_eq!(loaded[0].models.len(), 2);
        assert_eq!(loaded[0].models[0].name, "test-model-large");
        assert_eq!(loaded[0].models[0].context_window_tokens, Some(200_000));
        assert_eq!(loaded[0].models[0].task_size, ModelTaskSize::Large);
        assert_eq!(loaded[0].models[1].task_size, ModelTaskSize::Small);
        assert_eq!(loaded[0].kind, ProviderKind::OpenAiChatCompletions);
        assert_eq!(
            loaded[0].base_url,
            Some("https://api.test.com/v1".to_string())
        );
    }

    #[test]
    fn upsert_replaces_models() {
        let store = RegistryStore::open_memory().expect("open");
        let mut provider = sample_provider();
        store.upsert_provider(&provider).expect("upsert");
        assert_eq!(store.model_count().unwrap(), 2);

        // Update with fewer models.
        provider.models = vec![RegistryModel {
            name: "new-model".to_string(),
            task_size: "large".to_string(),
            context_window_tokens: Some(500_000),
        }];
        store.upsert_provider(&provider).expect("upsert again");

        assert_eq!(store.model_count().unwrap(), 1);
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded[0].models[0].name, "new-model");
        assert_eq!(loaded[0].models[0].context_window_tokens, Some(500_000));
    }

    #[test]
    fn replace_all_clears_old_data() {
        let store = RegistryStore::open_memory().expect("open");
        store.upsert_provider(&sample_provider()).expect("upsert");
        assert_eq!(store.provider_count().unwrap(), 1);

        let new_providers = vec![RegistryProvider {
            id: "other".to_string(),
            label: "Other".to_string(),
            description: String::new(),
            kind: "anthropic-messages".to_string(),
            api_key_env: "OTHER_KEY".to_string(),
            base_url: None,
            request_options: Default::default(),
            models: vec![],
        }];
        store.replace_all(&new_providers).expect("replace");

        assert_eq!(store.provider_count().unwrap(), 1);
        assert_eq!(store.model_count().unwrap(), 0);
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded[0].id, "other");
        assert_eq!(loaded[0].kind, ProviderKind::AnthropicMessages);
    }

    #[test]
    fn meta_get_set() {
        let store = RegistryStore::open_memory().expect("open");
        assert_eq!(store.meta_get("foo").unwrap(), None);

        store.meta_set("foo", "bar").expect("set");
        assert_eq!(store.meta_get("foo").unwrap(), Some("bar".to_string()));

        // Overwrite.
        store.meta_set("foo", "baz").expect("set");
        assert_eq!(store.meta_get("foo").unwrap(), Some("baz".to_string()));
    }

    #[test]
    fn context_window_none_survives_roundtrip() {
        let store = RegistryStore::open_memory().expect("open");
        let provider = RegistryProvider {
            id: "p".to_string(),
            label: "P".to_string(),
            description: String::new(),
            kind: "openai-chat-completions".to_string(),
            api_key_env: "K".to_string(),
            base_url: None,
            request_options: Default::default(),
            models: vec![RegistryModel {
                name: "m".to_string(),
                task_size: "small".to_string(),
                context_window_tokens: None,
            }],
        };
        store.upsert_provider(&provider).expect("upsert");
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded[0].models[0].context_window_tokens, None);
    }

    #[test]
    fn request_options_survive_roundtrip() {
        let store = RegistryStore::open_memory().expect("open");
        let mut provider = sample_provider();
        provider.request_options = ProviderRequestOptions {
            prompt_cache_key: Some("openai".to_string()),
            prompt_cache_retention: Some("24h".to_string()),
            anthropic_cache_control: Some(serde_json::json!({
                "type": "ephemeral",
                "ttl": "1h"
            })),
        };

        store.upsert_provider(&provider).expect("upsert");
        let loaded = store.load_all_providers().expect("load");

        let opts = loaded[0]
            .request_options
            .as_ref()
            .expect("request_options roundtripped");
        assert_eq!(opts.prompt_cache_key.as_deref(), Some("openai"));
        assert_eq!(opts.prompt_cache_retention.as_deref(), Some("24h"));
        assert_eq!(
            opts.anthropic_cache_control
                .as_ref()
                .and_then(|value| value.get("ttl"))
                .and_then(serde_json::Value::as_str),
            Some("1h")
        );
    }
}
