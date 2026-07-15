//! Global engine factory registry.

use crate::config::BrowserRuntimeConfig;
use crate::engine::{BrowserEngine, BrowserEngineFactory, DoctorReport, EngineContext};
use anyhow::{Result, bail};
use serde_json::json;
use std::sync::{Arc, RwLock};

static FACTORY: RwLock<Option<Arc<dyn BrowserEngineFactory>>> = RwLock::new(None);
static FALLBACKS: RwLock<Vec<Arc<dyn BrowserEngineFactory>>> = RwLock::new(Vec::new());

/// Install the primary browser engine factory (typically CloakBrowser Rust binding).
///
/// Call once during process startup from the host that links the binding, e.g.:
/// ```ignore
/// navi_browser::set_engine_factory(Arc::new(cloakbrowser_navi::Factory::default()));
/// ```
pub fn set_engine_factory(factory: Arc<dyn BrowserEngineFactory>) {
    if let Ok(mut slot) = FACTORY.write() {
        *slot = Some(factory);
    }
}

/// Clear the primary factory (tests).
pub fn clear_engine_factory() {
    if let Ok(mut slot) = FACTORY.write() {
        *slot = None;
    }
}

/// Register an additional fallback factory (e.g. CDP). Safe to call multiple times.
pub fn register_fallback_factory(factory: Arc<dyn BrowserEngineFactory>) {
    if let Ok(mut list) = FALLBACKS.write() {
        // Replace same id if re-registered.
        list.retain(|f| f.id() != factory.id());
        list.push(factory);
    }
}

/// Primary factory, if any.
pub fn primary_factory() -> Option<Arc<dyn BrowserEngineFactory>> {
    FACTORY.read().ok().and_then(|g| g.clone())
}

fn all_factories() -> Vec<Arc<dyn BrowserEngineFactory>> {
    let mut out = Vec::new();
    // Primary: host-registered factory (set_engine_factory), else built-in CloakBrowser.
    if let Some(p) = primary_factory() {
        out.push(p);
    }
    #[cfg(feature = "cloakbrowser")]
    {
        if !out
            .iter()
            .any(|f| f.id() == crate::engines::cloakbrowser::FACTORY_ID)
        {
            out.push(Arc::new(
                crate::engines::cloakbrowser::CloakBrowserEngineFactory,
            ));
        }
    }
    if let Ok(list) = FALLBACKS.read() {
        out.extend(list.iter().cloned());
    }
    // Built-in CDP fallback when feature is on and nothing else registered it yet.
    #[cfg(feature = "cdp-fallback")]
    {
        let has_cdp = out.iter().any(|f| f.id() == "cdp");
        if !has_cdp {
            out.push(Arc::new(crate::engines::cdp::CdpEngineFactory));
        }
    }
    out
}

/// Resolve factory for config.backend (`auto` or explicit id).
pub fn resolve_factory(config: &BrowserRuntimeConfig) -> Result<Arc<dyn BrowserEngineFactory>> {
    let factories = all_factories();
    if factories.is_empty() {
        bail!(
            "no browser engine factory registered — link the CloakBrowser Rust binding \
             and call navi_browser::set_engine_factory(...), or enable feature `cdp-fallback`"
        );
    }

    let want = config.backend.trim().to_ascii_lowercase();
    if want.is_empty() || want == "auto" {
        if let Some(f) = factories.iter().find(|f| f.available(config)) {
            return Ok(f.clone());
        }
        // Return the first factory so doctor/create can explain why unavailable.
        return Ok(factories[0].clone());
    }

    // Aliases for CDP backends.
    let want = match want.as_str() {
        "chrome" | "chromium" | "cdp_url" | "cdp-url" => "cdp".to_string(),
        other => other.to_string(),
    };

    factories
        .into_iter()
        .find(|f| f.id() == want)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "browser backend `{want}` is not registered (available: {})",
                available_ids().join(", ")
            )
        })
}

fn available_ids() -> Vec<String> {
    all_factories()
        .into_iter()
        .map(|f| f.id().to_string())
        .collect()
}

pub fn create_engine(
    config: &BrowserRuntimeConfig,
    ctx: EngineContext,
) -> Result<Arc<dyn BrowserEngine>> {
    let factory = resolve_factory(config)?;
    factory.create(config, ctx)
}

pub fn doctor_report(config: &BrowserRuntimeConfig) -> serde_json::Value {
    let factories = all_factories();
    let primary = primary_factory().map(|f| f.id().to_string());
    let reports: Vec<DoctorReport> = factories.iter().map(|f| f.doctor(config)).collect();
    let selected = resolve_factory(config).ok().map(|f| f.id().to_string());

    let mut hints = Vec::new();
    if primary.is_none() {
        hints.push(
            "No CloakBrowser Rust binding registered. When your binding is ready, call \
             navi_browser::set_engine_factory(Arc::new(...)) at process startup."
                .into(),
        );
    }
    if !factories.iter().any(|f| f.available(config)) {
        hints.push(
            "No available browser backend. Install CloakBrowser (via the Rust binding) \
             or enable/configure the CDP fallback."
                .into(),
        );
    }
    for r in &reports {
        hints.extend(r.hints.iter().cloned());
    }

    json!({
        "enabled": config.enabled,
        "backend": config.backend,
        "selected_factory": selected,
        "primary_factory": primary,
        "factories": reports,
        "hints": hints,
        "contract": {
            "trait": "BrowserEngine",
            "factory": "BrowserEngineFactory",
            "register": "navi_browser::set_engine_factory",
            "docs": "crates/navi-browser/INTEGRATION.md",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ContentKind, EngineStatus, NavigateResult, SnapshotResult};
    use async_trait::async_trait;
    use serde_json::Value;

    struct MockEngine;

    #[async_trait]
    impl BrowserEngine for MockEngine {
        fn backend_id(&self) -> &str {
            "mock"
        }
        async fn open(&self) -> Result<()> {
            Ok(())
        }
        async fn goto(&self, url: &str) -> Result<NavigateResult> {
            Ok(NavigateResult {
                url: url.into(),
                title: "t".into(),
            })
        }
        async fn snapshot(&self, _max_chars: usize) -> Result<SnapshotResult> {
            Ok(SnapshotResult {
                url: "about:blank".into(),
                snapshot: "ok".into(),
            })
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>> {
            Ok(vec![1, 2, 3])
        }
        async fn click(&self, _selector: &str) -> Result<()> {
            Ok(())
        }
        async fn type_text(&self, _selector: &str, _text: &str) -> Result<()> {
            Ok(())
        }
        async fn press(&self, _key: &str) -> Result<()> {
            Ok(())
        }
        async fn content(&self, _kind: ContentKind, _max_chars: usize) -> Result<String> {
            Ok(String::new())
        }
        async fn evaluate(&self, _expression: &str) -> Result<Value> {
            Ok(Value::Null)
        }
        async fn close(&self) -> Result<()> {
            Ok(())
        }
        fn status(&self) -> EngineStatus {
            EngineStatus {
                open: true,
                backend: "mock".into(),
                ..Default::default()
            }
        }
    }

    struct MockFactory;

    impl BrowserEngineFactory for MockFactory {
        fn id(&self) -> &str {
            "mock"
        }
        fn available(&self, _config: &BrowserRuntimeConfig) -> bool {
            true
        }
        fn doctor(&self, _config: &BrowserRuntimeConfig) -> DoctorReport {
            DoctorReport {
                backend: "mock".into(),
                available: true,
                details: json!({}),
                hints: vec![],
            }
        }
        fn create(
            &self,
            _config: &BrowserRuntimeConfig,
            _ctx: EngineContext,
        ) -> Result<Arc<dyn BrowserEngine>> {
            Ok(Arc::new(MockEngine))
        }
    }

    #[test]
    fn set_primary_factory_is_selected() {
        clear_engine_factory();
        set_engine_factory(Arc::new(MockFactory));
        let f = resolve_factory(&BrowserRuntimeConfig::default()).expect("factory");
        assert_eq!(f.id(), "mock");
        clear_engine_factory();
    }
}
