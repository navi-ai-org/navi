//! Stable engine contract for browser backends.
//!
//! The CloakBrowser Rust binding (or any other driver) implements
//! [`BrowserEngine`] / [`BrowserEngineFactory`] and registers via
//! [`crate::set_engine_factory`].

use crate::config::BrowserRuntimeConfig;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

/// Per-process / per-session context handed to factories.
#[derive(Debug, Clone)]
pub struct EngineContext {
    pub data_dir: PathBuf,
    pub session_id: String,
    /// Profile / user-data directory (under data_dir/browser/…).
    pub profile_dir: PathBuf,
    /// Directory for screenshots and other artifacts.
    pub artifacts_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Text,
    Html,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigateResult {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotResult {
    pub url: String,
    /// Token-friendly page summary (a11y / simplified DOM / text).
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineStatus {
    pub open: bool,
    pub backend: String,
    pub binary_label: Option<String>,
    pub current_url: Option<String>,
    pub extra: Value,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            open: false,
            backend: "none".into(),
            binary_label: None,
            current_url: None,
            extra: Value::Null,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub backend: String,
    pub available: bool,
    pub details: Value,
    pub hints: Vec<String>,
}

/// High-level browser driver used by the NAVI `browser` tool.
///
/// Implement this on top of the CloakBrowser Rust binding (preferred) or any
/// other automation stack. Keep methods stable — the tool schema maps 1:1.
#[async_trait]
pub trait BrowserEngine: Send + Sync {
    /// Backend id for diagnostics (`cloakbrowser`, `cdp`, …).
    fn backend_id(&self) -> &str;

    /// Ensure browser + at least one page are ready.
    async fn open(&self) -> Result<()>;

    async fn goto(&self, url: &str) -> Result<NavigateResult>;

    async fn snapshot(&self, max_chars: usize) -> Result<SnapshotResult>;

    /// PNG bytes (session layer writes to disk).
    async fn screenshot_png(&self) -> Result<Vec<u8>>;

    async fn click(&self, selector: &str) -> Result<()>;

    async fn type_text(&self, selector: &str, text: &str) -> Result<()>;

    async fn press(&self, key: &str) -> Result<()>;

    async fn content(&self, kind: ContentKind, max_chars: usize) -> Result<String>;

    async fn evaluate(&self, expression: &str) -> Result<Value>;

    async fn close(&self) -> Result<()>;

    fn status(&self) -> EngineStatus;
}

/// Creates [`BrowserEngine`] instances for a given config.
///
/// Register the preferred factory (CloakBrowser binding) at process start:
/// ```ignore
/// navi_browser::set_engine_factory(Arc::new(MyCloakFactory));
/// ```
pub trait BrowserEngineFactory: Send + Sync {
    fn id(&self) -> &str;

    /// Whether this factory can serve the current config (binary present, etc.).
    fn available(&self, config: &BrowserRuntimeConfig) -> bool;

    fn doctor(&self, config: &BrowserRuntimeConfig) -> DoctorReport;

    fn create(
        &self,
        config: &BrowserRuntimeConfig,
        ctx: EngineContext,
    ) -> Result<Arc<dyn BrowserEngine>>;
}
