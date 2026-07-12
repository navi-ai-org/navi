//! Runtime config shared by the `browser` tool and engine factories.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserRuntimeConfig {
    pub enabled: bool,
    /// Preferred backend id: `auto` | `cloakbrowser` | `cdp` | `cdp_url` | …
    ///
    /// - `auto` — first registered factory that reports `available`
    /// - explicit id — only that factory
    pub backend: String,
    /// Existing CDP HTTP base when using the CDP fallback / cloakserve.
    pub cdp_url: String,
    pub headless: bool,
    pub allow_private_network: bool,
    pub proxy: String,
    pub timeout_ms: u64,
    /// Optional absolute path to a browser binary (CloakBrowser / Chrome).
    pub binary_path: String,
    /// Enable CloakBrowser `HumanPage` motion (Bezier mouse / typing) when using
    /// the Rust binding.
    pub humanize: bool,
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Prefer CloakBrowser Rust binding when registered; else CDP fallback.
            backend: "auto".into(),
            cdp_url: String::new(),
            headless: true,
            allow_private_network: true,
            proxy: String::new(),
            timeout_ms: 30_000,
            binary_path: String::new(),
            humanize: false,
        }
    }
}
