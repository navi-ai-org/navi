//! CloakBrowser Rust client adapter
//! ([CloakHQ/CloakBrowser#438](https://github.com/CloakHQ/CloakBrowser/pull/438)).
//!
//! Implements [`BrowserEngine`] / [`BrowserEngineFactory`] on top of the
//! community `cloakbrowser` crate (playwright-rs + stealth Chromium binary).

use crate::config::BrowserRuntimeConfig;
use crate::engine::{
    BrowserEngine, BrowserEngineFactory, ContentKind, DoctorReport, EngineContext, EngineStatus,
    NavigateResult, SnapshotResult,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use cloakbrowser::{
    CloakBrowser, HumanPage, LaunchOptions, Proxy, binary_info, ensure_binary, launch,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Factory id used by config `backend = "cloakbrowser"` / `auto`.
pub const FACTORY_ID: &str = "cloakbrowser";

pub struct CloakBrowserEngineFactory;

impl BrowserEngineFactory for CloakBrowserEngineFactory {
    fn id(&self) -> &str {
        FACTORY_ID
    }

    fn available(&self, config: &BrowserRuntimeConfig) -> bool {
        config.enabled
    }

    fn doctor(&self, config: &BrowserRuntimeConfig) -> DoctorReport {
        let mut hints = Vec::new();
        let details = match binary_info(None) {
            Ok(info) => {
                if !info.installed {
                    hints.push(
                        "CloakBrowser binary not cached yet — first launch downloads ~200MB \
                         (or: `cargo run -p cloakbrowser-cli --manifest-path … -- install`)."
                            .into(),
                    );
                }
                json!({
                    "wrapper": "cloakbrowser (Rust / playwright-rs)",
                    "pr": "https://github.com/CloakHQ/CloakBrowser/pull/438",
                    "binary": {
                        "version": info.version,
                        "tier": info.tier,
                        "platform": info.platform,
                        "installed": info.installed,
                        "path": info.binary_path,
                        "cache_dir": info.cache_dir,
                    },
                    "headless": config.headless,
                    "humanize": config.humanize,
                    "proxy_set": !config.proxy.trim().is_empty(),
                })
            }
            Err(e) => {
                hints.push(format!("binary_info failed: {e}"));
                json!({ "error": e.to_string() })
            }
        };
        if !config.proxy.is_empty() {
            hints
                .push("Proxy is set — CloakBrowser enables geoip when proxy is configured.".into());
        }
        DoctorReport {
            backend: FACTORY_ID.into(),
            available: true,
            details,
            hints,
        }
    }

    fn create(
        &self,
        config: &BrowserRuntimeConfig,
        ctx: EngineContext,
    ) -> Result<Arc<dyn BrowserEngine>> {
        std::fs::create_dir_all(&ctx.profile_dir).ok();
        std::fs::create_dir_all(&ctx.artifacts_dir).ok();
        Ok(Arc::new(CloakBrowserEngine {
            config: config.clone(),
            ctx,
            state: Mutex::new(None),
        }))
    }
}

struct Live {
    browser: CloakBrowser,
    page: playwright_rs::Page,
    human: Option<HumanPage>,
    current_url: String,
    humanize: bool,
}

struct CloakBrowserEngine {
    config: BrowserRuntimeConfig,
    ctx: EngineContext,
    state: Mutex<Option<Live>>,
}

#[async_trait]
impl BrowserEngine for CloakBrowserEngine {
    fn backend_id(&self) -> &str {
        FACTORY_ID
    }

    async fn open(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        *guard = Some(self.boot().await?);
        Ok(())
    }

    async fn goto(&self, url: &str) -> Result<NavigateResult> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cloakbrowser not open")?;
        if let Some(human) = live.human.as_mut() {
            human
                .goto(url)
                .await
                .map_err(|e| anyhow::anyhow!("cloakbrowser goto: {e}"))?;
        } else {
            live.page
                .goto(url, None)
                .await
                .map_err(|e| anyhow::anyhow!("playwright goto: {e}"))?;
        }
        live.current_url = url.to_string();
        let title = live.page.title().await.unwrap_or_default();
        Ok(NavigateResult {
            url: live.current_url.clone(),
            title,
        })
    }

    async fn snapshot(&self, max_chars: usize) -> Result<SnapshotResult> {
        self.open().await?;
        let guard = self.state.lock().await;
        let live = guard.as_ref().context("cloakbrowser not open")?;
        let max_chars = max_chars.clamp(500, 80_000);
        let snapshot = live
            .page
            .evaluate_value(&snapshot_js(max_chars))
            .await
            .map_err(|e| anyhow::anyhow!("evaluate snapshot: {e}"))?;
        Ok(SnapshotResult {
            url: live.current_url.clone(),
            snapshot,
        })
    }

    async fn screenshot_png(&self) -> Result<Vec<u8>> {
        self.open().await?;
        let guard = self.state.lock().await;
        let live = guard.as_ref().context("cloakbrowser not open")?;
        live.page
            .screenshot(None)
            .await
            .map_err(|e| anyhow::anyhow!("screenshot: {e}"))
    }

    async fn click(&self, selector: &str) -> Result<()> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cloakbrowser not open")?;
        if let Some(human) = live.human.as_mut() {
            human
                .click(selector)
                .await
                .map_err(|e| anyhow::anyhow!("human click: {e}"))?;
        } else {
            let loc = live.page.locator(selector).await;
            loc.click(None)
                .await
                .map_err(|e| anyhow::anyhow!("click: {e}"))?;
        }
        Ok(())
    }

    async fn type_text(&self, selector: &str, text: &str) -> Result<()> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cloakbrowser not open")?;
        if let Some(human) = live.human.as_mut() {
            human
                .fill(selector, text)
                .await
                .map_err(|e| anyhow::anyhow!("human fill: {e}"))?;
        } else {
            let loc = live.page.locator(selector).await;
            loc.fill(text, None)
                .await
                .map_err(|e| anyhow::anyhow!("fill: {e}"))?;
        }
        Ok(())
    }

    async fn press(&self, key: &str) -> Result<()> {
        self.open().await?;
        let guard = self.state.lock().await;
        let live = guard.as_ref().context("cloakbrowser not open")?;
        let key = key.trim();
        if key.is_empty() {
            bail!("key is required");
        }
        live.page
            .keyboard()
            .press(key, None)
            .await
            .map_err(|e| anyhow::anyhow!("press: {e}"))?;
        Ok(())
    }

    async fn content(&self, kind: ContentKind, max_chars: usize) -> Result<String> {
        self.open().await?;
        let guard = self.state.lock().await;
        let live = guard.as_ref().context("cloakbrowser not open")?;
        let max_chars = max_chars.clamp(200, 100_000);
        match kind {
            ContentKind::Html => {
                let html = live
                    .page
                    .content()
                    .await
                    .map_err(|e| anyhow::anyhow!("content: {e}"))?;
                Ok(html.chars().take(max_chars).collect())
            }
            ContentKind::Text => {
                let expr = format!(
                    "(document.body && document.body.innerText || '').slice(0, {max_chars})"
                );
                live.page
                    .evaluate_value(&expr)
                    .await
                    .map_err(|e| anyhow::anyhow!("innerText: {e}"))
            }
        }
    }

    async fn evaluate(&self, expression: &str) -> Result<Value> {
        self.open().await?;
        let guard = self.state.lock().await;
        let live = guard.as_ref().context("cloakbrowser not open")?;
        let raw = live
            .page
            .evaluate_value(expression)
            .await
            .map_err(|e| anyhow::anyhow!("evaluate: {e}"))?;
        match serde_json::from_str(&raw) {
            Ok(v) => Ok(v),
            Err(_) => Ok(Value::String(raw)),
        }
    }

    async fn close(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        if let Some(live) = guard.take() {
            drop(live.human);
            drop(live.page);
            live.browser
                .close()
                .await
                .map_err(|e| anyhow::anyhow!("close: {e}"))?;
        }
        Ok(())
    }

    fn status(&self) -> EngineStatus {
        match self.state.try_lock() {
            Ok(guard) => match guard.as_ref() {
                Some(live) => EngineStatus {
                    open: true,
                    backend: FACTORY_ID.into(),
                    binary_label: Some("cloakbrowser".into()),
                    current_url: Some(live.current_url.clone()),
                    extra: json!({
                        "humanize": live.humanize,
                        "profile_dir": self.ctx.profile_dir.display().to_string(),
                    }),
                },
                None => EngineStatus {
                    open: false,
                    backend: FACTORY_ID.into(),
                    ..Default::default()
                },
            },
            Err(_) => EngineStatus {
                open: false,
                backend: FACTORY_ID.into(),
                extra: json!({ "note": "busy" }),
                ..Default::default()
            },
        }
    }
}

impl CloakBrowserEngine {
    async fn boot(&self) -> Result<Live> {
        let _path = ensure_binary(None, None)
            .await
            .map_err(|e| anyhow::anyhow!("cloakbrowser ensure_binary: {e}"))?;

        let humanize = self.config.humanize;
        let mut opts = LaunchOptions {
            headless: self.config.headless,
            humanize,
            stealth_args: true,
            ..Default::default()
        };
        if !self.config.proxy.trim().is_empty() {
            opts.proxy = Some(Proxy::from(self.config.proxy.trim().to_string()));
            opts.geoip = true;
        }
        if !self.config.binary_path.trim().is_empty() {
            // Binding respects CLOAKBROWSER_BINARY_PATH for local override.
            // SAFETY: single-process agent config applied before launching the browser.
            unsafe {
                std::env::set_var("CLOAKBROWSER_BINARY_PATH", self.config.binary_path.trim());
            }
        }

        let browser = launch(opts)
            .await
            .map_err(|e| anyhow::anyhow!("cloakbrowser launch: {e}"))?;

        let (page, human) = if humanize {
            let hp = browser
                .new_human_page()
                .await
                .map_err(|e| anyhow::anyhow!("new_human_page: {e}"))?;
            let page = hp.page().clone();
            (page, Some(hp))
        } else {
            let page = browser
                .new_page()
                .await
                .map_err(|e| anyhow::anyhow!("new_page: {e}"))?;
            (page, None)
        };

        Ok(Live {
            browser,
            page,
            human,
            current_url: "about:blank".into(),
            humanize,
        })
    }
}

fn snapshot_js(max_chars: usize) -> String {
    format!(
        r#"
(() => {{
  const max = {max_chars};
  const lines = [];
  lines.push('title: ' + (document.title || ''));
  lines.push('url: ' + location.href);
  document.querySelectorAll('h1,h2,h3').forEach((el, i) => {{
    if (i < 30) {{
      const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 200);
      if (t) lines.push('heading: ' + t);
    }}
  }});
  document.querySelectorAll('a[href]').forEach((el, i) => {{
    if (i < 40) {{
      const t = (el.innerText || '').trim().replace(/\s+/g, ' ').slice(0, 120);
      const href = el.getAttribute('href') || '';
      if (t || href) lines.push('link[' + i + ']: ' + t + ' -> ' + href);
    }}
  }});
  document.querySelectorAll('button, input, select, textarea, [role=button]').forEach((el, i) => {{
    if (i < 40) {{
      const name = el.getAttribute('name') || el.id || el.getAttribute('aria-label') || el.getAttribute('placeholder') || (el.innerText || '').trim().slice(0, 80);
      const tag = el.tagName.toLowerCase();
      const type = el.getAttribute('type') || '';
      lines.push('control[' + i + ']: <' + tag + (type ? ' type=' + type : '') + '> ' + name);
    }}
  }});
  const body = (document.body && document.body.innerText || '').trim().replace(/\s+/g, ' ');
  lines.push('body: ' + body.slice(0, Math.max(0, max - lines.join('\\n').length)));
  return lines.join('\\n').slice(0, max);
}})()
"#
    )
}
