//! Runtime config shared by the `browser` tool and engine factories.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserRuntimeConfig {
    pub enabled: bool,
    /// Preferred backend id: `auto` | `cdp` | `chrome` | `chromium` | `cdp_url` | …
    ///
    /// - `auto` — first registered factory that reports `available`
    /// - explicit id — only that factory
    pub backend: String,
    /// Existing CDP HTTP base (e.g. `http://127.0.0.1:9222`).
    pub cdp_url: String,
    pub headless: bool,
    pub allow_private_network: bool,
    pub proxy: String,
    pub timeout_ms: u64,
    /// Optional absolute path to a Chrome/Chromium binary.
    pub binary_path: String,
}

impl Default for BrowserRuntimeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: "auto".into(),
            cdp_url: String::new(),
            headless: true,
            allow_private_network: true,
            proxy: String::new(),
            timeout_ms: 30_000,
            binary_path: String::new(),
        }
    }
}
