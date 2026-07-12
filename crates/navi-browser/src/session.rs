//! Session-scoped browser handle over a pluggable [`BrowserEngine`].

use crate::config::BrowserRuntimeConfig;
use crate::engine::{BrowserEngine, ContentKind, EngineContext};
use crate::factory::create_engine;
use crate::url_policy::{UrlPolicyError, validate_navigation_url};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Shared browser session for one agent runtime / process.
#[derive(Clone)]
pub struct BrowserSession {
    engine: Arc<Mutex<Option<Arc<dyn BrowserEngine>>>>,
    config: BrowserRuntimeConfig,
    data_dir: PathBuf,
    session_id: String,
}

impl BrowserSession {
    pub fn new(
        config: BrowserRuntimeConfig,
        data_dir: PathBuf,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            engine: Arc::new(Mutex::new(None)),
            config,
            data_dir,
            session_id: session_id.into(),
        }
    }

    /// Inject a pre-built engine (tests / custom hosts).
    pub fn with_engine(
        config: BrowserRuntimeConfig,
        data_dir: PathBuf,
        session_id: impl Into<String>,
        engine: Arc<dyn BrowserEngine>,
    ) -> Self {
        Self {
            engine: Arc::new(Mutex::new(Some(engine))),
            config,
            data_dir,
            session_id: session_id.into(),
        }
    }

    pub fn config(&self) -> &BrowserRuntimeConfig {
        &self.config
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.data_dir
            .join("browser")
            .join(if self.session_id.is_empty() {
                "default"
            } else {
                &self.session_id
            })
    }

    fn context(&self) -> EngineContext {
        let artifacts = self.artifacts_dir();
        EngineContext {
            data_dir: self.data_dir.clone(),
            session_id: self.session_id.clone(),
            profile_dir: artifacts.join("profile"),
            artifacts_dir: artifacts,
        }
    }

    async fn ensure_engine(&self) -> Result<Arc<dyn BrowserEngine>> {
        if !self.config.enabled {
            bail!("browser tool is disabled ([browser].enabled = false)");
        }
        let mut guard = self.engine.lock().await;
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        let engine = create_engine(&self.config, self.context())?;
        engine.open().await?;
        *guard = Some(engine.clone());
        Ok(engine)
    }

    pub async fn status(&self) -> Value {
        let guard = self.engine.lock().await;
        let engine_status = guard.as_ref().map(|e| e.status());
        json!({
            "enabled": self.config.enabled,
            "backend": self.config.backend,
            "headless": self.config.headless,
            "allow_private_network": self.config.allow_private_network,
            "cdp_url": self.config.cdp_url,
            "artifacts_dir": self.artifacts_dir().display().to_string(),
            "open": engine_status.as_ref().map(|s| s.open).unwrap_or(false),
            "engine": engine_status,
            "factories": crate::factory::doctor_report(&self.config),
        })
    }

    pub async fn ensure_open(&self) -> Result<()> {
        let _ = self.ensure_engine().await?;
        Ok(())
    }

    pub async fn goto(&self, url: &str) -> Result<Value> {
        let parsed = validate_navigation_url(url, self.config.allow_private_network)
            .map_err(|e: UrlPolicyError| anyhow::anyhow!(e))?;
        let engine = self.ensure_engine().await?;
        let result = engine.goto(parsed.as_str()).await?;
        Ok(json!(result))
    }

    pub async fn snapshot(&self, max_chars: usize) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        let result = engine.snapshot(max_chars.clamp(500, 80_000)).await?;
        Ok(json!(result))
    }

    pub async fn screenshot(&self) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        let bytes = engine.screenshot_png().await?;
        let dir = self.artifacts_dir().join("screenshots");
        std::fs::create_dir_all(&dir).context("create screenshots dir")?;
        let name = format!(
            "shot-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        let path = dir.join(&name);
        std::fs::write(&path, &bytes).context("write screenshot")?;
        Ok(json!({
            "path": path.display().to_string(),
            "bytes": bytes.len(),
            "backend": engine.backend_id(),
        }))
    }

    pub async fn click(&self, selector: &str) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        engine.click(selector).await?;
        Ok(json!({ "ok": true, "selector": selector }))
    }

    pub async fn type_text(&self, selector: &str, text: &str) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        engine.type_text(selector, text).await?;
        Ok(json!({ "ok": true, "selector": selector }))
    }

    pub async fn press(&self, key: &str) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        engine.press(key).await?;
        Ok(json!({ "ok": true, "key": key }))
    }

    pub async fn content(&self, kind: &str, max_chars: usize) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        let kind = if kind == "html" {
            ContentKind::Html
        } else {
            ContentKind::Text
        };
        let content = engine
            .content(kind, max_chars.clamp(200, 100_000))
            .await?;
        Ok(json!({
            "kind": match kind { ContentKind::Html => "html", ContentKind::Text => "text" },
            "content": content,
        }))
    }

    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        let engine = self.ensure_engine().await?;
        let value = engine.evaluate(expression).await?;
        Ok(json!({ "value": value }))
    }

    pub async fn close(&self) -> Result<Value> {
        let mut guard = self.engine.lock().await;
        if let Some(engine) = guard.take() {
            engine.close().await?;
            Ok(json!({ "closed": true }))
        } else {
            Ok(json!({ "closed": false, "message": "browser was not open" }))
        }
    }
}

