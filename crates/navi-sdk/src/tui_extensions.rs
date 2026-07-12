//! Host-mediated TUI extension protocol for WASM plugin packages.
//!
//! Plugins declare UI via `tui.json` in their install directory (not native
//! ratatui widgets). The host loads these specs; the TUI decides how to render.

use std::fs;
use std::path::Path;

use navi_plugin_manifest::installed_plugins_dir;
use serde::{Deserialize, Serialize};

use crate::engine::NaviEngine;
use crate::types::NaviError;

type Result<T> = std::result::Result<T, NaviError>;

/// Declarative command shown in the palette / extensions hub.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiExtensionCommand {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub palette_group: Option<String>,
}

/// Declarative info panel (host renders as a simple modal/list entry).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TuiExtensionPanel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub body: String,
}

/// Full `tui.json` document for one installed plugin package.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TuiExtensionSpec {
    #[serde(default)]
    pub commands: Vec<TuiExtensionCommand>,
    #[serde(default)]
    pub panels: Vec<TuiExtensionPanel>,
    #[serde(default)]
    pub theme_tokens: std::collections::BTreeMap<String, String>,
}

/// Spec bound to its plugin install id and path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledTuiExtension {
    pub plugin_id: String,
    pub path: String,
    pub spec: TuiExtensionSpec,
}

/// Parse a `tui.json` document.
pub fn parse_tui_extension_spec(bytes: &[u8]) -> Result<TuiExtensionSpec> {
    serde_json::from_slice(bytes).map_err(|e| NaviError::Config(format!("invalid tui.json: {e}")))
}

/// Load `tui.json` from a single plugin directory if present.
pub fn load_tui_extension_from_dir(plugin_dir: &Path) -> Result<Option<TuiExtensionSpec>> {
    let path = plugin_dir.join("tui.json");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| NaviError::Config(e.to_string()))?;
    Ok(Some(parse_tui_extension_spec(&bytes)?))
}

/// Scan `{data_dir}/plugins/*/` for `tui.json` specs.
pub fn list_installed_tui_extensions(data_dir: &Path) -> Result<Vec<InstalledTuiExtension>> {
    let root = installed_plugins_dir(data_dir);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| NaviError::Config(e.to_string()))? {
        let entry = entry.map_err(|e| NaviError::Config(e.to_string()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(spec) = load_tui_extension_from_dir(&path)? {
            let plugin_id = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unknown".into());
            out.push(InstalledTuiExtension {
                plugin_id,
                path: path.display().to_string(),
                spec,
            });
        }
    }
    out.sort_by(|a, b| a.plugin_id.cmp(&b.plugin_id));
    Ok(out)
}

impl NaviEngine {
    /// List host-mediated TUI extensions declared by installed WASM packages.
    pub fn list_tui_extensions(&self) -> Result<Vec<InstalledTuiExtension>> {
        let loaded = self.loaded_config();
        list_installed_tui_extensions(&loaded.data_dir)
    }

    /// Flat list of palette commands from all installed `tui.json` files.
    pub fn list_tui_extension_commands(&self) -> Result<Vec<TuiExtensionCommand>> {
        Ok(self
            .list_tui_extensions()?
            .into_iter()
            .flat_map(|e| e.spec.commands)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin_dir(data_dir: &Path, id: &str) -> PathBuf {
        installed_plugins_dir(data_dir).join(id)
    }

    #[test]
    fn parse_and_list_tui_json() {
        let temp = tempfile::tempdir().unwrap();
        let dir = plugin_dir(temp.path(), "hello-echo");
        fs::create_dir_all(&dir).unwrap();
        let json = format!(
            "{{\"commands\":[{{\"id\":\"c1\",\"title\":\"Ping\",\"description\":\"d\",\"palette_group\":\"X\"}}],\"panels\":[{{\"id\":\"p1\",\"title\":\"Panel\",\"kind\":\"info\",\"body\":\"hi\"}}],\"theme_tokens\":{{\"accent\":\"{accent}\"}}}}",
            accent = "#fff"
        );
        fs::write(dir.join("tui.json"), json).unwrap();

        let list = list_installed_tui_extensions(temp.path()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].plugin_id, "hello-echo");
        assert_eq!(list[0].spec.commands[0].title, "Ping");
        assert_eq!(list[0].spec.panels[0].body, "hi");
        assert_eq!(
            list[0].spec.theme_tokens.get("accent").map(String::as_str),
            Some("#fff")
        );
    }
}
