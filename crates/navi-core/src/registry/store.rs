//! Local SQLite cache for the provider registry.

use crate::config::types::{
    ModelTaskSize, ProviderConfig, ProviderKind, ProviderModelConfig, ToolCallingMode,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Mutex;

use super::types::{
    ModelCapability, ModelPricing, Profile, RankedModel, RegistryAttachments, RegistryManifest,
    RegistryModel, RegistryProvider,
};

/// SQLite-backed registry store.
///
/// Thread-safe via internal `Mutex<Connection>` — registry operations are
/// short-lived and infrequent so contention is negligible.
pub struct RegistryStore {
    conn: Mutex<Connection>,
}

impl RegistryStore {
    /// Opens (or creates) the registry database at `<data_dir>/registry.db`.
    ///
    /// On first run (empty database), seeds the cache from the embedded registry
    /// snapshot so the provider catalog is immediately available without a
    /// network fetch.
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

        // Seed from the embedded snapshot if the cache is empty.
        if store.is_empty()? {
            if let Ok(providers) = super::embedded::embedded_providers() {
                tracing::info!(
                    providers = providers.len(),
                    "seeding registry cache from embedded snapshot"
                );
                store.replace_all(&providers)?;
            }
            if let Ok(manifest) = super::embedded::embedded_manifest() {
                let _ = store.save_manifest_meta(&manifest);
                // Also persist the full manifest JSON so load_cached_registry
                // and check_registry_manifest can find it.
                let manifest_json = serde_json::to_string(&manifest).ok();
                if let Some(json) = manifest_json {
                    let _ = store.meta_set("registry_manifest_json", &json);
                }
            }
        }

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
                tool_calling_mode TEXT,
                request_options TEXT NOT NULL DEFAULT '{}',
                sha256          TEXT,
                aggregator      INTEGER NOT NULL DEFAULT 0,
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS models (
                provider_id         TEXT NOT NULL,
                name                TEXT NOT NULL,
                task_size           TEXT,
                context_window_tokens INTEGER,
                max_output_tokens   INTEGER,
                recommended_temperature REAL,
                supports_thinking   INTEGER,
                supports_images     INTEGER,
                supports_audio      INTEGER,
                supports_video      INTEGER,
                supports_documents  INTEGER,
                tool_prompt_manifest INTEGER,
                reasoning_levels    TEXT NOT NULL DEFAULT '[]',
                default_reasoning_effort TEXT,
                PRIMARY KEY (provider_id, name),
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS model_capabilities (
                model_id    TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                capability  TEXT NOT NULL,
                value       TEXT NOT NULL,
                PRIMARY KEY (model_id, capability),
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS model_pricing (
                model_id    TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL,
                input_price REAL,
                output_price REAL,
                currency    TEXT NOT NULL DEFAULT 'USD',
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS model_profiles (
                model_id    TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                profile_id  TEXT NOT NULL,
                score       REAL NOT NULL DEFAULT 0.0,
                PRIMARY KEY (model_id, profile_id),
                FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS profiles (
                id              TEXT PRIMARY KEY,
                description     TEXT NOT NULL DEFAULT '',
                min_context     INTEGER,
                max_input_price REAL,
                requires_tools  INTEGER NOT NULL DEFAULT 0
            );
            ",
        )?;
        ensure_provider_request_options_column(&conn)?;
        ensure_model_output_columns(&conn)?;
        ensure_provider_tool_calling_mode_column(&conn)?;
        ensure_provider_sha256_column(&conn)?;
        ensure_provider_aggregator_column(&conn)?;
        relax_models_task_size_not_null(&conn)?;
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

    /// Returns the stored SHA-256 hash for a provider, or `None`.
    pub fn provider_sha256(&self, provider_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT sha256 FROM providers WHERE id = ?1")?;
        let mut rows =
            stmt.query_map(params![provider_id], |row| row.get::<_, Option<String>>(0))?;
        match rows.next() {
            Some(Ok(v)) => Ok(v),
            _ => Ok(None),
        }
    }

    /// Returns the set of provider ids currently in the cache.
    pub fn provider_ids(&self) -> Result<std::collections::HashSet<String>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT id FROM providers")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = std::collections::HashSet::new();
        for row in rows {
            ids.insert(row?);
        }
        Ok(ids)
    }

    /// Loads existing models for a provider from the cache, keyed by model name.
    /// Used by aggregator sync to preserve metadata (context_window, etc) for
    /// models that the API returns without rich metadata.
    pub fn load_provider_models(
        &self,
        provider_id: &str,
    ) -> Result<std::collections::HashMap<String, RegistryModel>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT name, task_size, context_window_tokens, max_output_tokens, recommended_temperature, supports_thinking, supports_images, supports_audio, supports_video, supports_documents, reasoning_levels, default_reasoning_effort
             FROM models WHERE provider_id = ?1",
        )?;
        let rows = stmt.query_map(params![provider_id], |row| {
            let name: String = row.get(0)?;
            let task_size_str: Option<String> = row.get(1)?;
            let ctx: Option<i64> = row.get(2)?;
            let max_out: Option<i64> = row.get(3)?;
            let temp: Option<f64> = row.get(4)?;
            let thinking: Option<i64> = row.get(5)?;
            let images: Option<i64> = row.get(6)?;
            let audio: Option<i64> = row.get(7)?;
            let video: Option<i64> = row.get(8)?;
            let documents: Option<i64> = row.get(9)?;
            let levels_json: Option<String> = row.get(10)?;
            let default_effort: Option<String> = row.get(11)?;

            Ok(RegistryModel {
                name: name.clone(),
                task_size: task_size_str,
                context_window_tokens: ctx.map(|v| v as u64),
                max_output_tokens: max_out.map(|v| v as u64),
                recommended_temperature: temp,
                supports_thinking: thinking.map(|v| v != 0),
                reasoning_levels: parse_reasoning_levels_json(levels_json.as_deref()),
                default_reasoning_effort: default_effort,
                supports_images: images.map(|v| v != 0),
                supports_audio: audio.map(|v| v != 0),
                supports_video: video.map(|v| v != 0),
                supports_documents: documents.map(|v| v != 0),
                supports_attachments: None,
                attachments: RegistryAttachments::default(),
                capabilities: Vec::new(),
                pricing: None,
            })
        })?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            let model = row?;
            map.insert(model.name.clone(), model);
        }
        Ok(map)
    }

    /// Deletes providers that are not in the given set of ids.
    /// Used during sync to remove providers that were deleted from the remote registry.
    pub fn delete_providers_not_in(&self, keep: &std::collections::HashSet<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare("SELECT id FROM providers")?;
        let to_delete: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .filter(|id| !keep.contains(id.as_str()))
            .collect();
        drop(stmt);
        for id in &to_delete {
            conn.execute("DELETE FROM providers WHERE id = ?1", params![id])?;
        }
        if !to_delete.is_empty() {
            tracing::info!(
                removed = to_delete.len(),
                "removed stale providers from cache"
            );
        }
        Ok(())
    }

    /// Upserts a provider and its models from a [`RegistryProvider`].
    pub fn upsert_provider(&self, provider: &RegistryProvider) -> Result<()> {
        self.upsert_provider_with_sha256(provider, None)
    }

    /// Upserts a provider with its SHA-256 hash for diff-based sync.
    pub fn upsert_provider_with_sha256(
        &self,
        provider: &RegistryProvider,
        sha256: Option<&str>,
    ) -> Result<()> {
        let kind = parse_provider_kind(&provider.kind);

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;

        tx.execute(
            "INSERT OR REPLACE INTO providers (id, label, description, kind, api_key_env, base_url, tool_calling_mode, request_options, sha256, aggregator, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))",
            params![
                provider.id,
                provider.label,
                provider.description,
                provider.kind,
                provider.api_key_env,
                provider.base_url,
                provider.tool_calling_mode,
                serde_json::to_string(&provider.request_options)?,
                sha256,
                provider.aggregator as i64,
            ],
        )?;

        // Delete existing models for this provider, then re-insert.
        tx.execute(
            "DELETE FROM models WHERE provider_id = ?1",
            params![provider.id],
        )?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO models (provider_id, name, task_size, context_window_tokens, max_output_tokens, recommended_temperature, supports_thinking, supports_images, supports_audio, supports_video, supports_documents, tool_prompt_manifest, reasoning_levels, default_reasoning_effort)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13)",
            )?;

            let attachment_defaults = &provider.defaults.attachments;
            for model in &provider.models {
                let levels_json =
                    serde_json::to_string(&model.reasoning_levels).unwrap_or_else(|_| "[]".into());
                stmt.execute(params![
                    provider.id,
                    model.name,
                    model.task_size,
                    model.context_window_tokens.map(|v| v as i64),
                    model.max_output_tokens.map(|v| v as i64),
                    model.recommended_temperature,
                    model.supports_thinking.map(|v| v as i64),
                    registry_model_supports_images(model, attachment_defaults).map(|v| v as i64),
                    registry_model_supports_audio(model, attachment_defaults).map(|v| v as i64),
                    registry_model_supports_video(model, attachment_defaults).map(|v| v as i64),
                    registry_model_supports_documents(model, attachment_defaults).map(|v| v as i64),
                    levels_json,
                    model.default_reasoning_effort,
                ])?;
            }
        }

        // Seed/refresh pricing from registry JSON (per 1M token rates).
        tx.execute(
            "DELETE FROM model_pricing WHERE provider_id = ?1",
            params![provider.id],
        )?;
        {
            let mut price_stmt = tx.prepare(
                "INSERT OR REPLACE INTO model_pricing (model_id, provider_id, input_price, output_price, currency)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for model in &provider.models {
                let Some(pricing) = model.pricing.as_ref() else {
                    continue;
                };
                if pricing.is_empty() {
                    continue;
                }
                let model_id = format!("{}:{}", provider.id, model.name);
                price_stmt.execute(params![
                    model_id,
                    provider.id,
                    pricing.input_per_1m,
                    pricing.output_per_1m,
                    pricing.currency.as_deref().unwrap_or("USD"),
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
            "SELECT id, label, description, kind, api_key_env, base_url, tool_calling_mode, request_options, aggregator FROM providers ORDER BY id",
        )?;

        let provider_rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })?;

        let mut providers = Vec::new();

        for row in provider_rows {
            let (
                id,
                label,
                description,
                kind_str,
                api_key_env,
                base_url,
                tool_calling_mode_str,
                request_options_json,
                aggregator_val,
            ) = row?;
            let kind = parse_provider_kind(&kind_str);
            let request_options = serde_json::from_str(&request_options_json).ok();
            let tool_calling_mode = tool_calling_mode_str
                .as_deref()
                .map(parse_tool_calling_mode);
            let aggregator = aggregator_val.unwrap_or(0) != 0;

            let mut model_stmt = conn.prepare(
                "SELECT m.name, m.task_size, m.context_window_tokens, m.max_output_tokens,
                        m.recommended_temperature, m.supports_thinking, m.supports_images,
                        m.supports_audio, m.supports_video, m.supports_documents,
                        m.tool_prompt_manifest, pr.input_price, pr.output_price,
                        m.reasoning_levels, m.default_reasoning_effort
                 FROM models m
                 LEFT JOIN model_pricing pr
                   ON pr.model_id = (m.provider_id || ':' || m.name)
                 WHERE m.provider_id = ?1
                 ORDER BY m.rowid",
            )?;

            let models = model_stmt
                .query_map(params![id], |row| {
                    let name: String = row.get(0)?;
                    let task_size_str: Option<String> = row.get(1)?;
                    let ctx: Option<i64> = row.get(2)?;
                    let max_out: Option<i64> = row.get(3)?;
                    let temp: Option<f64> = row.get(4)?;
                    let thinking: Option<i64> = row.get(5)?;
                    let images: Option<i64> = row.get(6)?;
                    let audio: Option<i64> = row.get(7)?;
                    let video: Option<i64> = row.get(8)?;
                    let documents: Option<i64> = row.get(9)?;
                    let tpm: Option<i64> = row.get(10)?;
                    let input_price: Option<f64> = row.get(11)?;
                    let output_price: Option<f64> = row.get(12)?;
                    let levels_json: Option<String> = row.get(13)?;
                    let default_effort: Option<String> = row.get(14)?;

                    Ok(ProviderModelConfig {
                        name,
                        task_size: task_size_str.as_deref().and_then(|s| match s {
                            "small" => Some(ModelTaskSize::Small),
                            "large" => Some(ModelTaskSize::Large),
                            _ => None,
                        }),
                        context_window_tokens: ctx.map(|v| v as u64),
                        max_output_tokens: max_out.map(|v| v as u64),
                        recommended_temperature: temp,
                        supports_thinking: thinking.map(|v| v != 0),
                        reasoning_levels: parse_reasoning_levels_json(levels_json.as_deref()),
                        default_reasoning_effort: default_effort,
                        supports_images: images.map(|v| v != 0),
                        supports_audio: audio.map(|v| v != 0),
                        supports_video: video.map(|v| v != 0),
                        supports_documents: documents.map(|v| v != 0),
                        tool_prompt_manifest: tpm.map(|v| v != 0),
                        pricing_input_per_1m: input_price,
                        pricing_output_per_1m: output_price,
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
                tool_calling_mode,
                aggregator,
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

    // ── Capabilities CRUD ───────────────────────────────────────────────

    /// Upserts capabilities for a model. Replaces all existing capabilities for that model.
    pub fn upsert_capabilities(
        &self,
        model_id: &str,
        provider_id: &str,
        capabilities: &[(String, String)],
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM model_capabilities WHERE model_id = ?1",
            params![model_id],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO model_capabilities (model_id, provider_id, capability, value)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (cap, value) in capabilities {
                stmt.execute(params![model_id, provider_id, cap, value])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Loads all capabilities for a model.
    pub fn load_capabilities(&self, model_id: &str) -> Result<Vec<ModelCapability>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT model_id, provider_id, capability, value
             FROM model_capabilities WHERE model_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![model_id], |row| {
                Ok(ModelCapability {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    capability: row.get(2)?,
                    value: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── Pricing CRUD ────────────────────────────────────────────────────

    /// Upserts pricing for a model.
    pub fn upsert_pricing(
        &self,
        model_id: &str,
        provider_id: &str,
        input_price: Option<f64>,
        output_price: Option<f64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (model_id, provider_id, input_price, output_price)
             VALUES (?1, ?2, ?3, ?4)",
            params![model_id, provider_id, input_price, output_price],
        )?;
        Ok(())
    }

    /// Loads pricing for a model.
    pub fn load_pricing(&self, model_id: &str) -> Result<Option<ModelPricing>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT model_id, provider_id, input_price, output_price, currency
             FROM model_pricing WHERE model_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![model_id], |row| {
            Ok(ModelPricing {
                model_id: row.get(0)?,
                provider_id: row.get(1)?,
                input_price: row.get(2)?,
                output_price: row.get(3)?,
                currency: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(Ok(p)) => Ok(Some(p)),
            _ => Ok(None),
        }
    }

    // ── Profiles CRUD ───────────────────────────────────────────────────

    /// Upserts a profile definition.
    pub fn upsert_profile(&self, profile: &Profile) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO profiles (id, description, min_context, max_input_price, requires_tools)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                profile.id,
                profile.description,
                profile.min_context.map(|v| v as i64),
                profile.max_input_price,
                profile.requires_tools as i64,
            ],
        )?;
        Ok(())
    }

    /// Upserts a model-profile association.
    pub fn upsert_model_profile(
        &self,
        model_id: &str,
        provider_id: &str,
        profile_id: &str,
        score: f64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT OR REPLACE INTO model_profiles (model_id, provider_id, profile_id, score)
             VALUES (?1, ?2, ?3, ?4)",
            params![model_id, provider_id, profile_id, score],
        )?;
        Ok(())
    }

    /// Queries for models matching a profile, ranked by score and price.
    ///
    /// Returns models that satisfy the profile's constraints (min context, max price,
    /// tool support) ordered by score descending, then input price ascending.
    pub fn query_models_by_profile(&self, profile_id: &str) -> Result<Vec<RankedModel>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT
                mp.model_id,
                mp.provider_id,
                m.name,
                mp.score,
                pr.input_price,
                pr.output_price,
                m.context_window_tokens
             FROM model_profiles mp
             JOIN models m ON m.provider_id = mp.provider_id AND m.name = (
                SELECT SUBSTR(mp.model_id, INSTR(mp.model_id, ':') + 1)
             )
             LEFT JOIN model_pricing pr ON pr.model_id = mp.model_id
             LEFT JOIN profiles p ON p.id = mp.profile_id
             WHERE mp.profile_id = ?1
               AND (p.min_context IS NULL OR m.context_window_tokens >= p.min_context)
               AND (p.max_input_price IS NULL OR pr.input_price IS NULL OR pr.input_price <= p.max_input_price)
               AND (p.requires_tools = 0 OR m.supports_thinking IS NOT NULL)
             ORDER BY mp.score DESC, pr.input_price ASC, pr.output_price ASC",
        )?;
        let rows = stmt
            .query_map(params![profile_id], |row| {
                Ok(RankedModel {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    model_name: row.get(2)?,
                    score: row.get(3)?,
                    input_price: row.get(4)?,
                    output_price: row.get(5)?,
                    context_window_tokens: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Seeds the default built-in profile definitions.
    pub fn seed_default_profiles(&self) -> Result<()> {
        let defaults = vec![
            Profile {
                id: "cheap_general".to_string(),
                description: "General-purpose cheap model".to_string(),
                min_context: Some(32_000),
                max_input_price: Some(0.50),
                requires_tools: false,
            },
            Profile {
                id: "cheap_code".to_string(),
                description: "Cheap code-focused model with tool support".to_string(),
                min_context: Some(64_000),
                max_input_price: Some(1.00),
                requires_tools: true,
            },
            Profile {
                id: "repo_search".to_string(),
                description: "Fast repository exploration".to_string(),
                min_context: Some(64_000),
                max_input_price: Some(0.50),
                requires_tools: true,
            },
            Profile {
                id: "naming".to_string(),
                description: "Session title generation".to_string(),
                min_context: Some(8_000),
                max_input_price: Some(0.20),
                requires_tools: false,
            },
            Profile {
                id: "long_context_cheap".to_string(),
                description: "Compaction and summarization".to_string(),
                min_context: Some(128_000),
                max_input_price: Some(1.00),
                requires_tools: false,
            },
            Profile {
                id: "research_synthesis".to_string(),
                description: "Research subagent with tool access".to_string(),
                min_context: Some(64_000),
                max_input_price: Some(1.00),
                requires_tools: true,
            },
        ];
        for profile in &defaults {
            self.upsert_profile(profile)?;
        }
        Ok(())
    }

    /// Deletes all capabilities, pricing, and model-profile entries for a provider.
    pub fn delete_provider_metadata(&self, provider_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "DELETE FROM model_capabilities WHERE provider_id = ?1",
            params![provider_id],
        )?;
        conn.execute(
            "DELETE FROM model_pricing WHERE provider_id = ?1",
            params![provider_id],
        )?;
        conn.execute(
            "DELETE FROM model_profiles WHERE provider_id = ?1",
            params![provider_id],
        )?;
        Ok(())
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

fn parse_tool_calling_mode(s: &str) -> ToolCallingMode {
    match s {
        "native" => ToolCallingMode::Native,
        "text-extracted" => ToolCallingMode::TextExtracted,
        "manifest-only" => ToolCallingMode::ManifestOnly,
        "disabled" => ToolCallingMode::Disabled,
        _ => ToolCallingMode::Native,
    }
}

/// Converts a [`RegistryProvider`] into a [`ProviderConfig`].
///
/// This is the single conversion path used by both the SQLite cache loader and
/// the embedded snapshot fallback, ensuring consistent field mapping.
pub fn registry_provider_to_config(rp: RegistryProvider) -> ProviderConfig {
    let kind = parse_provider_kind(&rp.kind);
    let tool_calling_mode = rp.tool_calling_mode.as_deref().map(parse_tool_calling_mode);
    let attachment_defaults = rp.defaults.attachments;

    let models = rp
        .models
        .into_iter()
        .map(|m| {
            let supports_images = registry_model_supports_images(&m, &attachment_defaults);
            let supports_audio = registry_model_supports_audio(&m, &attachment_defaults);
            let supports_video = registry_model_supports_video(&m, &attachment_defaults);
            let supports_documents = registry_model_supports_documents(&m, &attachment_defaults);
            ProviderModelConfig {
                name: m.name,
                task_size: m.task_size.as_deref().and_then(|s| match s {
                    "small" => Some(ModelTaskSize::Small),
                    "large" => Some(ModelTaskSize::Large),
                    _ => None,
                }),
                context_window_tokens: m.context_window_tokens,
                max_output_tokens: m.max_output_tokens,
                recommended_temperature: m.recommended_temperature,
                supports_thinking: m.supports_thinking,
                reasoning_levels: m.reasoning_levels,
                default_reasoning_effort: m.default_reasoning_effort,
                supports_images,
                supports_audio,
                supports_video,
                supports_documents,
                tool_prompt_manifest: None,
                pricing_input_per_1m: m.pricing.as_ref().and_then(|p| p.input_per_1m),
                pricing_output_per_1m: m.pricing.as_ref().and_then(|p| p.output_per_1m),
            }
        })
        .collect();

    ProviderConfig {
        id: rp.id,
        label: rp.label,
        description: rp.description,
        kind,
        api_key_env: rp.api_key_env,
        base_url: rp.base_url,
        models,
        tool_calling_mode,
        request_options: if rp.request_options.is_empty() {
            None
        } else {
            Some(rp.request_options)
        },
        aggregator: rp.aggregator,
        ..Default::default()
    }
}

fn registry_model_has_capability(model: &super::types::RegistryModel, names: &[&str]) -> bool {
    model.capabilities.iter().any(|capability| {
        let normalized = capability.trim().to_ascii_lowercase();
        names.iter().any(|name| normalized == *name)
    })
}

fn registry_model_supports_images(
    model: &super::types::RegistryModel,
    defaults: &super::types::RegistryAttachments,
) -> Option<bool> {
    model
        .attachments
        .images
        .or(model.supports_images)
        .or_else(|| {
            (model.supports_attachments == Some(true)
                || registry_model_has_capability(model, &["image", "images", "vision"]))
            .then_some(true)
        })
        .or(defaults.images)
}

fn registry_model_supports_audio(
    model: &super::types::RegistryModel,
    defaults: &super::types::RegistryAttachments,
) -> Option<bool> {
    model
        .attachments
        .audio
        .or(model.supports_audio)
        .or_else(|| {
            registry_model_has_capability(model, &["audio", "sound", "speech"]).then_some(true)
        })
        .or(defaults.audio)
}

fn registry_model_supports_video(
    model: &super::types::RegistryModel,
    defaults: &super::types::RegistryAttachments,
) -> Option<bool> {
    model
        .attachments
        .video
        .or(model.supports_video)
        .or_else(|| registry_model_has_capability(model, &["video"]).then_some(true))
        .or(defaults.video)
}

fn registry_model_supports_documents(
    model: &super::types::RegistryModel,
    defaults: &super::types::RegistryAttachments,
) -> Option<bool> {
    model
        .attachments
        .documents
        .or(model.supports_documents)
        .or_else(|| {
            (model.supports_attachments == Some(true)
                || registry_model_has_capability(
                    model,
                    &["document", "documents", "pdf", "file", "files"],
                ))
            .then_some(true)
        })
        .or(defaults.documents)
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

fn parse_reasoning_levels_json(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn ensure_model_output_columns(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(models)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();

    if !columns.contains(&"max_output_tokens".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN max_output_tokens INTEGER",
            [],
        )?;
    }
    if !columns.contains(&"recommended_temperature".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN recommended_temperature REAL",
            [],
        )?;
    }
    if !columns.contains(&"supports_thinking".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN supports_thinking INTEGER",
            [],
        )?;
    }
    if !columns.contains(&"supports_images".to_string()) {
        conn.execute("ALTER TABLE models ADD COLUMN supports_images INTEGER", [])?;
    }
    if !columns.contains(&"supports_audio".to_string()) {
        conn.execute("ALTER TABLE models ADD COLUMN supports_audio INTEGER", [])?;
    }
    if !columns.contains(&"supports_video".to_string()) {
        conn.execute("ALTER TABLE models ADD COLUMN supports_video INTEGER", [])?;
    }
    if !columns.contains(&"supports_documents".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN supports_documents INTEGER",
            [],
        )?;
    }
    if !columns.contains(&"reasoning_levels".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN reasoning_levels TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    if !columns.contains(&"default_reasoning_effort".to_string()) {
        conn.execute(
            "ALTER TABLE models ADD COLUMN default_reasoning_effort TEXT",
            [],
        )?;
    }

    Ok(())
}

fn ensure_provider_tool_calling_mode_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| matches!(name, Ok(name) if name == "tool_calling_mode"));

    if !has_column {
        conn.execute(
            "ALTER TABLE providers ADD COLUMN tool_calling_mode TEXT",
            [],
        )?;
    }

    Ok(())
}

fn ensure_provider_sha256_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| matches!(name, Ok(name) if name == "sha256"));

    if !has_column {
        conn.execute("ALTER TABLE providers ADD COLUMN sha256 TEXT", [])?;
    }

    Ok(())
}

fn ensure_provider_aggregator_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(providers)")?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| matches!(name, Ok(name) if name == "aggregator"));

    if !has_column {
        conn.execute(
            "ALTER TABLE providers ADD COLUMN aggregator INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    Ok(())
}

/// Migrates the `models` table from an older schema where `task_size` was
/// `NOT NULL` to the current nullable version. SQLite doesn't support
/// `ALTER COLUMN`, so we rebuild the table.
fn relax_models_task_size_not_null(conn: &Connection) -> Result<()> {
    // Check if task_size has a NOT NULL constraint.
    let mut stmt = conn.prepare("PRAGMA table_info(models)")?;
    let has_not_null: bool = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            let notnull: i64 = row.get(3)?;
            Ok((name, notnull))
        })?
        .filter_map(|r| r.ok())
        .any(|(name, notnull)| name == "task_size" && notnull != 0);

    if !has_not_null {
        return Ok(());
    }

    tracing::info!("migrating models table: relaxing task_size NOT NULL constraint");

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS models_new (
            provider_id         TEXT NOT NULL,
            name                TEXT NOT NULL,
            task_size           TEXT,
            context_window_tokens INTEGER,
            max_output_tokens   INTEGER,
            recommended_temperature REAL,
            supports_thinking   INTEGER,
            supports_images     INTEGER,
            supports_audio      INTEGER,
            supports_video      INTEGER,
            supports_documents  INTEGER,
            tool_prompt_manifest INTEGER,
            PRIMARY KEY (provider_id, name),
            FOREIGN KEY (provider_id) REFERENCES providers(id) ON DELETE CASCADE
        );

        INSERT INTO models_new (provider_id, name, task_size, context_window_tokens, max_output_tokens, recommended_temperature, supports_thinking, supports_images, supports_audio, supports_video, supports_documents, tool_prompt_manifest)
        SELECT provider_id, name, task_size, context_window_tokens, max_output_tokens, recommended_temperature, supports_thinking, supports_images, supports_audio, supports_video, supports_documents, tool_prompt_manifest
        FROM models;

        DROP TABLE models;
        ALTER TABLE models_new RENAME TO models;
        ",
    )?;

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
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![
                RegistryModel {
                    name: "test-model-large".to_string(),
                    task_size: Some("large".to_string()),
                    context_window_tokens: Some(200_000),
                    max_output_tokens: Some(8_192),
                    recommended_temperature: Some(0.7),
                    supports_thinking: None,
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                    supports_attachments: None,
                    supports_images: None,
                    supports_audio: None,
                    supports_video: None,
                    supports_documents: None,
                    attachments: Default::default(),
                    capabilities: Vec::new(),
                    pricing: None,
                },
                RegistryModel {
                    name: "test-model-small".to_string(),
                    task_size: Some("small".to_string()),
                    context_window_tokens: Some(128_000),
                    max_output_tokens: Some(4_096),
                    recommended_temperature: Some(0.5),
                    supports_thinking: None,
                    reasoning_levels: Vec::new(),
                    default_reasoning_effort: None,
                    supports_attachments: None,
                    supports_images: None,
                    supports_audio: None,
                    supports_video: None,
                    supports_documents: None,
                    attachments: Default::default(),
                    capabilities: Vec::new(),
                    pricing: None,
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
    fn tool_calling_mode_roundtrips_through_store() {
        let store = RegistryStore::open_memory().expect("open");
        let mut provider = sample_provider();
        provider.tool_calling_mode = Some("native".to_string());
        store.upsert_provider(&provider).expect("upsert");

        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].tool_calling_mode, Some(ToolCallingMode::Native));
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
        assert_eq!(loaded[0].models[0].max_output_tokens, Some(8_192));
        assert_eq!(loaded[0].models[0].recommended_temperature, Some(0.7));
        assert_eq!(loaded[0].models[0].task_size, Some(ModelTaskSize::Large));
        assert_eq!(loaded[0].models[1].task_size, Some(ModelTaskSize::Small));
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
            task_size: Some("large".to_string()),
            context_window_tokens: Some(500_000),
            max_output_tokens: Some(16_384),
            recommended_temperature: Some(0.8),
            supports_thinking: None,
            reasoning_levels: Vec::new(),
            default_reasoning_effort: None,
            supports_attachments: None,
            supports_images: None,
            supports_audio: None,
            supports_video: None,
            supports_documents: None,
            attachments: Default::default(),
            capabilities: Vec::new(),
            pricing: None,
        }];
        store.upsert_provider(&provider).expect("upsert again");

        assert_eq!(store.model_count().unwrap(), 1);
        let loaded = store.load_all_providers().expect("load");
        assert_eq!(loaded[0].models[0].name, "new-model");
        assert_eq!(loaded[0].models[0].context_window_tokens, Some(500_000));
        assert_eq!(loaded[0].models[0].max_output_tokens, Some(16_384));
        assert_eq!(loaded[0].models[0].recommended_temperature, Some(0.8));
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
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
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
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                name: "m".to_string(),
                task_size: Some("small".to_string()),
                context_window_tokens: None,
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
                supports_attachments: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                attachments: Default::default(),
                capabilities: Vec::new(),
                pricing: None,
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

    #[test]
    fn capabilities_upsert_and_load() {
        let store = RegistryStore::open_memory().expect("open");
        store.upsert_provider(&sample_provider()).expect("upsert");

        let model_id = "test-provider:test-model-large";
        let caps = vec![
            ("tool_calling".to_string(), "true".to_string()),
            ("fast".to_string(), "true".to_string()),
            ("cheap".to_string(), "true".to_string()),
        ];
        store
            .upsert_capabilities(model_id, "test-provider", &caps)
            .expect("upsert caps");

        let loaded = store.load_capabilities(model_id).expect("load caps");
        assert_eq!(loaded.len(), 3);
        assert!(loaded.iter().any(|c| c.capability == "tool_calling"));

        // Replace with fewer caps.
        let caps2 = vec![("fast".to_string(), "true".to_string())];
        store
            .upsert_capabilities(model_id, "test-provider", &caps2)
            .expect("replace caps");
        let loaded2 = store.load_capabilities(model_id).expect("load caps 2");
        assert_eq!(loaded2.len(), 1);
        assert_eq!(loaded2[0].capability, "fast");
    }

    #[test]
    fn pricing_upsert_and_load() {
        let store = RegistryStore::open_memory().expect("open");
        store.upsert_provider(&sample_provider()).expect("upsert");

        let model_id = "test-provider:test-model-large";
        store
            .upsert_pricing(model_id, "test-provider", Some(0.10), Some(0.30))
            .expect("upsert pricing");

        let loaded = store.load_pricing(model_id).expect("load pricing");
        let pricing = loaded.expect("pricing exists");
        assert_eq!(pricing.input_price, Some(0.10));
        assert_eq!(pricing.output_price, Some(0.30));
        assert_eq!(pricing.currency, "USD");
    }

    #[test]
    fn pricing_returns_none_for_missing() {
        let store = RegistryStore::open_memory().expect("open");
        let loaded = store.load_pricing("nonexistent:model").expect("load");
        assert!(loaded.is_none());
    }

    #[test]
    fn profiles_seed_and_query() {
        let store = RegistryStore::open_memory().expect("open");
        store.upsert_provider(&sample_provider()).expect("upsert");

        let model_id = "test-provider:test-model-large";
        store
            .upsert_pricing(model_id, "test-provider", Some(0.10), Some(0.30))
            .expect("pricing");
        store
            .upsert_model_profile(model_id, "test-provider", "cheap_general", 0.9)
            .expect("profile");

        store.seed_default_profiles().expect("seed profiles");

        let ranked = store
            .query_models_by_profile("cheap_general")
            .expect("query");
        assert!(!ranked.is_empty());
        assert_eq!(ranked[0].model_id, model_id);
        assert_eq!(ranked[0].score, 0.9);
    }

    #[test]
    fn query_respects_min_context_filter() {
        let store = RegistryStore::open_memory().expect("open");

        // Insert a provider with a small-context model.
        let provider = RegistryProvider {
            id: "tiny".to_string(),
            label: "Tiny".to_string(),
            description: String::new(),
            kind: "openai-chat-completions".to_string(),
            api_key_env: "TINY_KEY".to_string(),
            base_url: None,
            tool_calling_mode: None,
            aggregator: false,
            defaults: Default::default(),
            request_options: Default::default(),
            models: vec![RegistryModel {
                name: "tiny-model".to_string(),
                task_size: Some("small".to_string()),
                context_window_tokens: Some(4_000),
                max_output_tokens: None,
                recommended_temperature: None,
                supports_thinking: None,
                reasoning_levels: Vec::new(),
                default_reasoning_effort: None,
                supports_attachments: None,
                supports_images: None,
                supports_audio: None,
                supports_video: None,
                supports_documents: None,
                attachments: Default::default(),
                capabilities: Vec::new(),
                pricing: None,
            }],
        };
        store.upsert_provider(&provider).expect("upsert");

        let model_id = "tiny:tiny-model";
        store
            .upsert_model_profile(model_id, "tiny", "cheap_general", 1.0)
            .expect("profile");

        store.seed_default_profiles().expect("seed");

        // cheap_general requires min_context 32k, tiny-model has 4k.
        let ranked = store
            .query_models_by_profile("cheap_general")
            .expect("query");
        assert!(
            ranked.is_empty(),
            "tiny-model should be filtered out by min_context"
        );
    }

    #[test]
    fn delete_provider_metadata_cascades() {
        let store = RegistryStore::open_memory().expect("open");
        store.upsert_provider(&sample_provider()).expect("upsert");

        let model_id = "test-provider:test-model-large";
        store
            .upsert_capabilities(model_id, "test-provider", &[("fast".into(), "true".into())])
            .expect("caps");
        store
            .upsert_pricing(model_id, "test-provider", Some(0.10), Some(0.30))
            .expect("pricing");
        store
            .upsert_model_profile(model_id, "test-provider", "cheap_general", 0.9)
            .expect("profile");

        store
            .delete_provider_metadata("test-provider")
            .expect("delete");

        assert!(store.load_capabilities(model_id).unwrap().is_empty());
        assert!(store.load_pricing(model_id).unwrap().is_none());
        let ranked = store.query_models_by_profile("cheap_general").unwrap();
        assert!(ranked.is_empty());
    }
}
