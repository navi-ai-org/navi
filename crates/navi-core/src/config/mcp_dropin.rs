//! Drop-in MCP server definitions (`mcp.d/`).
//!
//! Same ownership rule as [`super::providers::dropin`]: list-shaped
//! configuration that the UI/CLI adds and removes lives in its own file per
//! entry, so `config.toml` stays hand-authored and is never rewritten.
//!
//! ```text
//! ~/.config/navi/mcp.d/memory.toml
//! ```
//!
//! ```toml
//! id = "memory"
//! command = "memory-mcp-server"
//! args = ["--stdio"]
//! enabled = true
//! tool_prefix = "mem"
//! ```
//!
//! `[[mcp.servers]]` blocks in `config.toml` keep working and are merged by id;
//! a drop-in file wins over a `config.toml` entry with the same id. MCP servers
//! are deliberately global-only: project config may not introduce executable
//! surfaces (see `merge_from_file`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::types::{McpServerConfig, NaviConfig};

/// Directory (relative to the global config dir) holding MCP server files.
pub const MCP_DIR_NAME: &str = "mcp.d";

/// The directory scanned for MCP server files.
pub fn mcp_dir(global_config_dir: &Path) -> PathBuf {
    global_config_dir.join(MCP_DIR_NAME)
}

/// One `mcp.d/*.toml` file. `id` defaults to the file stem.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpServerFile {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    tool_prefix: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

/// A problem found while scanning `mcp.d/`.
#[derive(Debug, Clone)]
pub struct McpDropInDiagnostic {
    /// File the problem was found in.
    pub path: PathBuf,
    /// Description (includes the TOML error when parsing failed).
    pub message: String,
}

/// Result of scanning `mcp.d/`.
#[derive(Debug, Clone, Default)]
pub struct McpDropInSnapshot {
    /// Servers declared by files, in file order.
    pub servers: Vec<McpServerConfig>,
    /// Parse problems (never fatal).
    pub diagnostics: Vec<McpDropInDiagnostic>,
    /// Files that contributed a server.
    pub files: Vec<(PathBuf, String)>,
}

impl McpDropInSnapshot {
    /// Number of servers contributed by files.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }
}

/// Scans `mcp.d/` under a global config directory.
pub fn scan(global_config_dir: &Path) -> McpDropInSnapshot {
    let dir = mcp_dir(global_config_dir);
    let mut snapshot = McpDropInSnapshot::default();
    let Ok(entries) = fs::read_dir(&dir) else {
        return snapshot;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();

    for path in paths {
        let is_toml = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"));
        if !is_toml {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with('.') || name.ends_with('~') {
            continue;
        }
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) => {
                snapshot.diagnostics.push(McpDropInDiagnostic {
                    path,
                    message: format!("failed to read file: {err}"),
                });
                continue;
            }
        };
        if raw.trim().is_empty() {
            continue;
        }
        let parsed: McpServerFile = match toml::from_str(&raw) {
            Ok(parsed) => parsed,
            Err(err) => {
                snapshot.diagnostics.push(McpDropInDiagnostic {
                    path,
                    message: format!("invalid MCP server file: {err}"),
                });
                continue;
            }
        };
        let id = parsed
            .id
            .clone()
            .or_else(|| path.file_stem().map(|stem| stem.to_string_lossy().to_string()));
        let Some(id) = id.filter(|id| !id.trim().is_empty()) else {
            snapshot.diagnostics.push(McpDropInDiagnostic {
                path,
                message: "cannot derive a server id from the file name".to_string(),
            });
            continue;
        };
        if parsed.command.is_none() && parsed.url.is_none() {
            snapshot.diagnostics.push(McpDropInDiagnostic {
                path: path.clone(),
                message: format!("server `{id}` needs either `command` (stdio) or `url` (http)"),
            });
            continue;
        }
        snapshot.files.push((path, id.clone()));
        snapshot.servers.push(McpServerConfig {
            id,
            command: parsed.command,
            url: parsed.url,
            args: parsed.args,
            env: parsed.env,
            cwd: parsed.cwd,
            enabled: parsed.enabled,
            tool_prefix: parsed.tool_prefix,
            timeout_ms: parsed.timeout_ms,
        });
    }
    snapshot
}

/// Merges `mcp.d/` servers into `config.mcp.servers` (files win by id).
pub fn apply_to_config(config: &mut NaviConfig, global_config_dir: &Path) -> McpDropInSnapshot {
    let snapshot = scan(global_config_dir);
    for diagnostic in &snapshot.diagnostics {
        tracing::warn!(target: "navi::config", "{}: {}", diagnostic.path.display(), diagnostic.message);
    }
    for server in &snapshot.servers {
        config.mcp.servers.retain(|existing| existing.id != server.id);
        config.mcp.servers.push(server.clone());
    }
    snapshot
}

/// Writes a server definition to `mcp.d/<id>.toml`, replacing any existing file.
pub fn write_server_file(dir: &Path, server: &McpServerConfig) -> Result<PathBuf> {
    if server.id.trim().is_empty() {
        anyhow::bail!("MCP server id must not be empty");
    }
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{}.toml", file_stem_for(&server.id)));

    // Serialize by hand so the file stays readable and only carries real values.
    let mut content = String::from(
        "# MCP server definition written by `navi mcp add`.\n\
         # Hand edits are kept: NAVI only rewrites this file on an explicit command.\n\n",
    );
    content.push_str(&format!("id = {}\n", toml_string(&server.id)));
    if let Some(command) = &server.command {
        content.push_str(&format!("command = {}\n", toml_string(command)));
    }
    if let Some(url) = &server.url {
        content.push_str(&format!("url = {}\n", toml_string(url)));
    }
    if !server.args.is_empty() {
        let args: Vec<String> = server.args.iter().map(|arg| toml_string(arg)).collect();
        content.push_str(&format!("args = [{}]\n", args.join(", ")));
    }
    if let Some(cwd) = &server.cwd {
        content.push_str(&format!(
            "cwd = {}\n",
            toml_string(&cwd.to_string_lossy())
        ));
    }
    if let Some(prefix) = &server.tool_prefix {
        content.push_str(&format!("tool_prefix = {}\n", toml_string(prefix)));
    }
    if let Some(timeout) = server.timeout_ms {
        content.push_str(&format!("timeout_ms = {timeout}\n"));
    }
    content.push_str(&format!("enabled = {}\n", server.enabled));
    // Tables last: anything written after `[env]` would land inside it.
    if !server.env.is_empty() {
        content.push_str("\n[env]\n");
        let mut keys: Vec<&String> = server.env.keys().collect();
        keys.sort();
        for key in keys {
            content.push_str(&format!("{} = {}\n", key, toml_string(&server.env[key])));
        }
    }
    crate::config::state::write_atomic(&path, &content)?;
    Ok(path)
}

/// Deletes `mcp.d/<id>.toml`. Returns the removed path when it existed.
pub fn remove_server_file(dir: &Path, server_id: &str) -> Result<Option<PathBuf>> {
    let path = dir.join(format!("{}.toml", file_stem_for(server_id)));
    if !path.exists() {
        return Ok(None);
    }
    fs::remove_file(&path).with_context(|| format!("failed to delete {}", path.display()))?;
    Ok(Some(path))
}

/// Server id → defining file.
pub fn server_file_map(dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    for (path, id) in scan(dir.parent().unwrap_or(dir)).files {
        map.entry(id).or_insert(path);
    }
    map
}

fn file_stem_for(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('-');
        }
    }
    if out.is_empty() {
        out.push_str("server");
    }
    out
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_server_files_and_derives_ids_from_file_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp = dir.path().join(MCP_DIR_NAME);
        fs::create_dir_all(&mcp).expect("mkdir");
        fs::write(
            mcp.join("memory.toml"),
            "command = \"memory-mcp\"\nargs = [\"--stdio\"]\ntool_prefix = \"mem\"\n",
        )
        .expect("write");
        fs::write(mcp.join("notes.toml"), "url = \"http://127.0.0.1:9/mcp\"\n").expect("write");

        let snapshot = scan(dir.path());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        assert_eq!(snapshot.server_count(), 2);
        let memory = snapshot
            .servers
            .iter()
            .find(|server| server.id == "memory")
            .expect("memory server");
        assert_eq!(memory.command.as_deref(), Some("memory-mcp"));
        assert_eq!(memory.args, vec!["--stdio".to_string()]);
        assert_eq!(memory.tool_prefix.as_deref(), Some("mem"));
        assert!(memory.enabled, "enabled defaults to true");

        let mut config = NaviConfig::default();
        apply_to_config(&mut config, dir.path());
        assert_eq!(config.mcp.servers.len(), 2);
    }

    #[test]
    fn drop_in_servers_win_over_config_toml_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp = dir.path().join(MCP_DIR_NAME);
        fs::create_dir_all(&mcp).expect("mkdir");
        fs::write(mcp.join("memory.toml"), "command = \"from-file\"\n").expect("write");

        let mut config = NaviConfig::default();
        config.mcp.servers.push(McpServerConfig {
            id: "memory".to_string(),
            command: Some("from-config".to_string()),
            url: None,
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            enabled: true,
            tool_prefix: None,
            timeout_ms: None,
        });
        apply_to_config(&mut config, dir.path());
        assert_eq!(config.mcp.servers.len(), 1);
        assert_eq!(
            config.mcp.servers[0].command.as_deref(),
            Some("from-file")
        );
    }

    #[test]
    fn a_server_without_command_or_url_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp = dir.path().join(MCP_DIR_NAME);
        fs::create_dir_all(&mcp).expect("mkdir");
        fs::write(mcp.join("broken.toml"), "args = [\"--stdio\"]\n").expect("write");

        let snapshot = scan(dir.path());
        assert_eq!(snapshot.server_count(), 0);
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert!(snapshot.diagnostics[0].message.contains("command"));
    }

    #[test]
    fn write_and_remove_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mcp = dir.path().join(MCP_DIR_NAME);
        let server = McpServerConfig {
            id: "osint".to_string(),
            command: Some("osint-mcp".to_string()),
            url: None,
            args: vec!["--stdio".to_string()],
            env: [("TOKEN".to_string(), "abc".to_string())].into_iter().collect(),
            cwd: Some(PathBuf::from("/tmp")),
            enabled: true,
            tool_prefix: Some("osint".to_string()),
            timeout_ms: Some(5_000),
        };
        let path = write_server_file(&mcp, &server).expect("write");
        assert_eq!(path.file_name().unwrap(), "osint.toml");

        let snapshot = scan(dir.path());
        assert!(snapshot.diagnostics.is_empty(), "{:?}", snapshot.diagnostics);
        let parsed = &snapshot.servers[0];
        assert_eq!(parsed.command.as_deref(), Some("osint-mcp"));
        assert_eq!(parsed.env.get("TOKEN").map(String::as_str), Some("abc"));
        assert_eq!(parsed.timeout_ms, Some(5_000));

        let removed = remove_server_file(&mcp, "osint").expect("remove");
        assert!(removed.is_some());
        assert!(remove_server_file(&mcp, "osint").expect("remove again").is_none());
    }
}
