//! CDP fallback engine (Chrome / CloakBrowser binary via remote debugging).
//!
//! This is temporary until the CloakBrowser Rust binding is wired as the primary factory.

mod launch {
    include!("cdp_launch.rs");
}
mod protocol {
    include!("cdp_protocol.rs");
}

use crate::config::BrowserRuntimeConfig;
use crate::engine::{
    BrowserEngine, BrowserEngineFactory, ContentKind, DoctorReport, EngineContext, EngineStatus,
    NavigateResult, SnapshotResult,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use launch::{
    BrowserBackendKind, LaunchOptions, LaunchedBrowser, cdp_http_ready, discover_browser,
    launch_browser,
};
use protocol::{CdpConnection, new_page_ws};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct CdpEngineFactory;

impl BrowserEngineFactory for CdpEngineFactory {
    fn id(&self) -> &str {
        "cdp"
    }

    fn available(&self, config: &BrowserRuntimeConfig) -> bool {
        if !config.cdp_url.trim().is_empty() {
            return true;
        }
        discover_browser(non_empty_path(&config.binary_path), &config.backend).is_some()
    }

    fn doctor(&self, config: &BrowserRuntimeConfig) -> DoctorReport {
        let binary = discover_browser(
            non_empty_path(&config.binary_path),
            if config.backend.eq_ignore_ascii_case("auto") {
                "auto"
            } else {
                &config.backend
            },
        );
        let mut hints = Vec::new();
        if !config.cdp_url.trim().is_empty() {
            hints.push(format!(
                "CDP URL configured: {} — ensure cloakserve/Chrome remote debugging is running.",
                config.cdp_url
            ));
        } else if binary.is_none() {
            hints.push(
                "CDP fallback: no Chrome/Chromium/CloakBrowser binary found. \
                 Prefer the CloakBrowser Rust binding when ready."
                    .into(),
            );
        } else if matches!(
            binary.as_ref().map(|b| b.kind),
            Some(BrowserBackendKind::CloakBrowser)
        ) {
            hints.push(
                "Found CloakBrowser *binary* (CDP process launch). \
                 Still prefer the native Rust binding for full API/stealth control."
                    .into(),
            );
        }
        DoctorReport {
            backend: "cdp".into(),
            available: self.available(config),
            details: json!({
                "cdp_url": config.cdp_url,
                "binary": binary.as_ref().map(|b| json!({
                    "path": b.path.display().to_string(),
                    "kind": format!("{:?}", b.kind),
                })),
            }),
            hints,
        }
    }

    fn create(
        &self,
        config: &BrowserRuntimeConfig,
        ctx: EngineContext,
    ) -> Result<Arc<dyn BrowserEngine>> {
        Ok(Arc::new(CdpEngine {
            config: config.clone(),
            ctx,
            state: Mutex::new(None),
        }))
    }
}

struct Live {
    _launched: Option<LaunchedBrowser>,
    page_id: String,
    cdp: CdpConnection,
    binary_label: String,
    current_url: String,
}

struct CdpEngine {
    config: BrowserRuntimeConfig,
    ctx: EngineContext,
    state: Mutex<Option<Live>>,
}

#[async_trait]
impl BrowserEngine for CdpEngine {
    fn backend_id(&self) -> &str {
        "cdp"
    }

    async fn open(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        *guard = Some(self.start().await?);
        Ok(())
    }

    async fn goto(&self, url: &str) -> Result<NavigateResult> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        live.cdp.navigate(url).await?;
        live.current_url = url.to_string();
        let title = live
            .cdp
            .evaluate("document.title")
            .await
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(NavigateResult {
            url: live.current_url.clone(),
            title,
        })
    }

    async fn snapshot(&self, max_chars: usize) -> Result<SnapshotResult> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let max_chars = max_chars.clamp(500, 80_000);
        let expression = format!(
            r#"
(() => {{
  const max = {max_chars};
  const lines = [];
  lines.push('title: ' + (document.title || ''));
  lines.push('url: ' + location.href);
  const push = (label, el) => {{
    if (!el) return;
    const text = (el.innerText || el.textContent || '').trim().replace(/\s+/g, ' ');
    if (!text) return;
    lines.push(label + ': ' + text.slice(0, 200));
  }};
  document.querySelectorAll('h1,h2,h3').forEach((el, i) => {{
    if (i < 30) push('heading', el);
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
        );
        let text = live
            .cdp
            .evaluate(&expression)
            .await?
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(SnapshotResult {
            url: live.current_url.clone(),
            snapshot: text,
        })
    }

    async fn screenshot_png(&self) -> Result<Vec<u8>> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let b64 = live.cdp.screenshot_png_base64().await?;
        B64.decode(b64.as_bytes()).context("decode screenshot")
    }

    async fn click(&self, selector: &str) -> Result<()> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let sel = serde_json::to_string(selector)?;
        let expr = format!(
            r#"
(() => {{
  const el = document.querySelector({sel});
  if (!el) throw new Error('selector not found');
  el.scrollIntoView({{ block: 'center', inline: 'center' }});
  el.click();
  return true;
}})()
"#
        );
        let _ = live.cdp.evaluate(&expr).await?;
        Ok(())
    }

    async fn type_text(&self, selector: &str, text: &str) -> Result<()> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let sel = serde_json::to_string(selector)?;
        let val = serde_json::to_string(text)?;
        let expr = format!(
            r#"
(() => {{
  const el = document.querySelector({sel});
  if (!el) throw new Error('selector not found');
  el.focus();
  if ('value' in el) {{
    el.value = {val};
    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  }} else {{
    el.textContent = {val};
  }}
  return true;
}})()
"#
        );
        let _ = live.cdp.evaluate(&expr).await?;
        Ok(())
    }

    async fn press(&self, key: &str) -> Result<()> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let key = key.trim();
        if key.is_empty() {
            bail!("key is required");
        }
        let _ = live
            .cdp
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyDown", "key": key }),
            )
            .await;
        let _ = live
            .cdp
            .call(
                "Input.dispatchKeyEvent",
                json!({ "type": "keyUp", "key": key }),
            )
            .await;
        Ok(())
    }

    async fn content(&self, kind: ContentKind, max_chars: usize) -> Result<String> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        let max_chars = max_chars.clamp(200, 100_000);
        let expr = match kind {
            ContentKind::Html => format!(
                "(document.documentElement && document.documentElement.outerHTML || '').slice(0, {max_chars})"
            ),
            ContentKind::Text => {
                format!("(document.body && document.body.innerText || '').slice(0, {max_chars})")
            }
        };
        Ok(live
            .cdp
            .evaluate(&expr)
            .await?
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn evaluate(&self, expression: &str) -> Result<Value> {
        self.open().await?;
        let mut guard = self.state.lock().await;
        let live = guard.as_mut().context("cdp not open")?;
        live.cdp.evaluate(expression).await
    }

    async fn close(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        *guard = None;
        Ok(())
    }

    fn status(&self) -> EngineStatus {
        // Try non-blocking status; if lock busy, report unknown.
        match self.state.try_lock() {
            Ok(guard) => match guard.as_ref() {
                Some(live) => EngineStatus {
                    open: true,
                    backend: "cdp".into(),
                    binary_label: Some(live.binary_label.clone()),
                    current_url: Some(live.current_url.clone()),
                    extra: json!({ "page_id": live.page_id }),
                },
                None => EngineStatus {
                    open: false,
                    backend: "cdp".into(),
                    ..Default::default()
                },
            },
            Err(_) => EngineStatus {
                open: false,
                backend: "cdp".into(),
                extra: json!({ "note": "busy" }),
                ..Default::default()
            },
        }
    }
}

impl CdpEngine {
    async fn start(&self) -> Result<Live> {
        std::fs::create_dir_all(&self.ctx.profile_dir)?;
        let (cdp_http_base, launched, binary_label) = if !self.config.cdp_url.trim().is_empty() {
            let base = self.config.cdp_url.trim().to_string();
            if !cdp_http_ready(&base).await {
                bail!("CDP endpoint not ready: {base}");
            }
            (base, None, "external-cdp".into())
        } else {
            let binary = discover_browser(
                non_empty_path(&self.config.binary_path),
                &self.config.backend,
            )
            .context(
                "no browser binary for CDP fallback — register CloakBrowser Rust binding \
                     via navi_browser::set_engine_factory, or install Chrome/Chromium",
            )?;
            let launched = launch_browser(
                &binary,
                LaunchOptions {
                    headless: self.config.headless,
                    user_data_dir: self.ctx.profile_dir.clone(),
                    proxy: non_empty(&self.config.proxy).map(|s| s.to_string()),
                    extra_args: Vec::new(),
                },
            )
            .await?;
            let base = format!("http://127.0.0.1:{}", launched.debug_port);
            let label = binary.path.display().to_string();
            (base, Some(launched), label)
        };

        let (page_id, page_ws) = new_page_ws(&cdp_http_base, "about:blank").await?;
        let cdp = CdpConnection::connect(&page_ws).await?;
        Ok(Live {
            _launched: launched,
            page_id,
            cdp,
            binary_label,
            current_url: "about:blank".into(),
        })
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}

fn non_empty_path(s: &str) -> Option<&Path> {
    non_empty(s).map(Path::new)
}
