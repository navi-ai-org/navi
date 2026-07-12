use anyhow::{Context, Result};
#[cfg(feature = "wasm-runtime")]
use navi_core::Tool;
use navi_core::ToolExecutor;
#[cfg(feature = "wasm-runtime")]
use navi_plugin_manifest::PluginManifest;
use navi_plugin_manifest::{self, Lockfile, SecurityDefaults};
use std::path::{Path, PathBuf};
#[cfg(feature = "wasm-runtime")]
use std::sync::Arc;

#[cfg(feature = "wasm-runtime")]
use crate::tool_adapter::WasmPluginTool;

/// Plugin orchestrator that manages the full plugin lifecycle:
/// discovery → manifest parsing → validation → risk classification → tool registration.
pub struct PluginOrchestrator {
    /// Project root directory.
    #[cfg(feature = "wasm-runtime")]
    project_root: PathBuf,
    /// Plugin directory (where .wasm files and plugin.toml live).
    plugin_dir: PathBuf,
    /// Lockfile path.
    lockfile_path: PathBuf,
    /// Security defaults.
    #[cfg(feature = "wasm-runtime")]
    defaults: SecurityDefaults,
    /// Loaded lockfile.
    lockfile: Lockfile,
    /// Warnings from loading.
    warnings: Vec<String>,
}

/// Report from loading plugins.
#[derive(Debug)]
pub struct PluginLoadReport {
    pub loaded: Vec<LoadedPluginInfo>,
    pub warnings: Vec<String>,
    pub tool_count: usize,
}

/// Info about a loaded plugin.
#[derive(Debug)]
pub struct LoadedPluginInfo {
    pub plugin_id: String,
    pub version: String,
    pub tool_count: usize,
    pub risk_level: String,
}

impl PluginOrchestrator {
    /// Create a new orchestrator.
    pub fn new(
        project_root: PathBuf,
        plugin_dir: PathBuf,
        lockfile_path: PathBuf,
        defaults: SecurityDefaults,
    ) -> Self {
        let _ = navi_plugin_manifest::migrate_legacy_per_plugin_lockfiles(&plugin_dir);
        let lockfile = Lockfile::load(&lockfile_path).unwrap_or_default();
        #[cfg(not(feature = "wasm-runtime"))]
        let _ = (&project_root, &defaults);

        Self {
            #[cfg(feature = "wasm-runtime")]
            project_root,
            plugin_dir,
            lockfile_path,
            #[cfg(feature = "wasm-runtime")]
            defaults,
            lockfile,
            warnings: Vec::new(),
        }
    }

    /// Discover and load all plugins from the plugin directory.
    ///
    /// For each plugin directory:
    /// 1. Parse plugin.toml manifest
    /// 2. Validate manifest (community trust level)
    /// 3. Verify WASM hash
    /// 4. Check lockfile for reconsent
    /// 5. Create WasmPluginTool for each tool
    /// 6. Register tools with ToolExecutor
    pub fn load_plugins(&mut self, executor: &mut ToolExecutor) -> Result<PluginLoadReport> {
        let mut report = PluginLoadReport {
            loaded: Vec::new(),
            warnings: Vec::new(),
            tool_count: 0,
        };

        // Discover plugin directories
        let plugin_dirs = self.discover_plugins()?;

        for plugin_dir in plugin_dirs {
            match self.load_single_plugin(&plugin_dir, executor) {
                Ok(info) => {
                    report.tool_count += info.tool_count;
                    report.loaded.push(info);
                }
                Err(err) => {
                    let warning = format!(
                        "failed to load plugin at {}: {:#}",
                        plugin_dir.display(),
                        err
                    );
                    report.warnings.push(warning);
                }
            }
        }

        // Save lockfile after loading
        if let Some(parent) = self.lockfile_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = self.lockfile.save(&self.lockfile_path) {
            report
                .warnings
                .push(format!("failed to save lockfile: {}", e));
        }

        Ok(report)
    }

    /// Load a single plugin from its directory.
    fn load_single_plugin(
        &mut self,
        plugin_dir: &Path,
        executor: &mut ToolExecutor,
    ) -> Result<LoadedPluginInfo> {
        // 1. Parse manifest
        let manifest_path = plugin_dir.join("plugin.toml");
        let manifest_content =
            std::fs::read_to_string(&manifest_path).context("failed to read plugin.toml")?;
        let manifest = navi_plugin_manifest::parse_manifest(&manifest_content)
            .context("failed to parse plugin.toml")?;

        // 2. Resolve trust from lockfile (path installs = LocalDev, marketplace = Community).
        let plugin_id = &manifest.plugin.id;
        let lock_entry = self
            .lockfile
            .find(plugin_id)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "plugin '{}' is not in the lockfile; install with `navi plugin install` first",
                    plugin_id
                )
            })?;
        let trust_level = lock_entry.trust_level;

        // 3. Validate manifest under that trust level
        navi_plugin_manifest::validate(&manifest, trust_level)
            .context("manifest validation failed")?;

        // 4. Load WASM binary
        let wasm_path = plugin_dir.join(&manifest.plugin.entry);
        let wasm_bytes = std::fs::read(&wasm_path).context("failed to read WASM binary")?;

        navi_plugin_manifest::verify_plugin_signature(&manifest, &wasm_bytes, trust_level)
            .map_err(|reason| anyhow::anyhow!("signature verification failed: {reason}"))?;

        // 4a. Detect WASM component kind
        #[cfg(feature = "wasm-runtime")]
        {
            let component_kind = navi_plugin_runtime::detect_component_kind(&wasm_bytes);
            tracing::debug!(
                plugin = manifest.plugin.id,
                kind = ?component_kind,
                "detected WASM component kind"
            );
        }

        // 5. Verify WASM hash
        let expected_hash = &manifest.plugin.wasm_hash;
        if !navi_plugin_manifest::verify_wasm_hash(&wasm_bytes, expected_hash) {
            anyhow::bail!(
                "WASM hash mismatch: expected {}, got {}",
                expected_hash,
                navi_plugin_manifest::compute_wasm_hash(&wasm_bytes)
            );
        }

        // 6. Require lockfile capability approval before loading tools
        navi_plugin_manifest::verify_approved_capabilities(&manifest, &lock_entry)
            .map_err(|reason| anyhow::anyhow!("{reason}"))?;

        #[cfg(not(feature = "wasm-runtime"))]
        {
            let _ = executor;
            return Err(anyhow::anyhow!(
                "WASM plugin runtime is disabled in this build; rebuild with feature `wasm-runtime`"
            ));
        }

        #[cfg(feature = "wasm-runtime")]
        {
            // 6. Create and register tools
            let mut tool_count = 0;
            let mut risk_labels = Vec::new();

            for (i, _tool_def) in manifest.tools.iter().enumerate() {
                let wasm_tool = WasmPluginTool::new(
                    &manifest,
                    i,
                    wasm_bytes.clone(),
                    self.project_root.clone(),
                    self.defaults.clone(),
                )?;

                risk_labels.push(format!("{:?}", wasm_tool.risk_level()));
                let tool_name = wasm_tool.definition().name.clone();
                executor.register_tool(Arc::new(wasm_tool));
                tool_count += 1;

                tracing::info!(
                    plugin = plugin_id,
                    tool = tool_name,
                    "registered WASM plugin tool"
                );
            }

            // 7. Refresh lockfile metadata without expanding approved capabilities
            let mut entry = lock_entry.clone();
            entry.version = manifest.plugin.version.clone();
            entry.publisher = manifest.plugin.publisher.clone();
            entry.wasm_hash = manifest.plugin.wasm_hash.clone();
            entry.capabilities_hash = compute_capabilities_hash(&manifest);
            entry.tools_hash = compute_tools_hash(&manifest);
            self.lockfile.upsert(entry);

            let risk_level = risk_labels.join(", ");

            Ok(LoadedPluginInfo {
                plugin_id: plugin_id.clone(),
                version: manifest.plugin.version.clone(),
                tool_count,
                risk_level,
            })
        }
    }

    /// Discover plugin directories in the plugin directory.
    fn discover_plugins(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();

        if !self.plugin_dir.exists() {
            return Ok(dirs);
        }

        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && path.join("plugin.toml").exists() {
                dirs.push(path);
            }
        }

        Ok(dirs)
    }

    /// Get the current lockfile.
    pub fn lockfile(&self) -> &Lockfile {
        &self.lockfile
    }

    /// Get warnings from loading.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// Compute a hash of the capabilities section.
#[cfg(feature = "wasm-runtime")]
fn compute_capabilities_hash(manifest: &PluginManifest) -> String {
    let caps: Vec<&str> = manifest.capabilities.iter().map(|c| c.id()).collect();
    let content = caps.join(",");
    navi_plugin_manifest::compute_content_hash(&content)
}

/// Compute a hash of the tools section.
#[cfg(feature = "wasm-runtime")]
fn compute_tools_hash(manifest: &PluginManifest) -> String {
    let tools: Vec<String> = manifest
        .tools
        .iter()
        .map(|t| format!("{}:{:?}:{}", t.id, t.risk, t.summary))
        .collect();
    let content = tools.join("\n");
    navi_plugin_manifest::compute_content_hash(&content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_plugin_manifest::LockEntry;
    use std::fs;

    fn setup() -> (tempfile::TempDir, SecurityDefaults) {
        let tmp = tempfile::tempdir().unwrap();
        let defaults = SecurityDefaults::default();
        (tmp, defaults)
    }

    fn write_lockfile_entry(plugins_root: &Path, plugin_id: &str, approved: Vec<&str>) {
        let lockfile_path = plugins_root.join("navi-plugins.lock");
        let mut lockfile = Lockfile::load(&lockfile_path).unwrap_or_default();
        lockfile.upsert(LockEntry {
            id: plugin_id.to_string(),
            version: "1.0.0".to_string(),
            publisher: "gh:test".to_string(),
            wasm_hash: format!("sha256:{}", "0".repeat(64)),
            capabilities_hash: String::new(),
            tools_hash: String::new(),
            approved_capabilities: approved.into_iter().map(str::to_string).collect(),
            approved_at: "0".to_string(),
            trust_level: TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        });
        lockfile.save(&lockfile_path).unwrap();
    }

    fn write_plugin(tmp: &Path, id: &str, wasm_content: &[u8]) {
        use navi_plugin_manifest::{
            PluginManifest, PluginMeta, RuntimeKind, ToolDef, ToolRisk,
            sign_plugin_manifest_for_tests,
        };

        let plugin_dir = tmp.join("plugins").join(id);
        fs::create_dir_all(&plugin_dir).unwrap();

        let mut manifest = PluginManifest {
            plugin: PluginMeta {
                id: id.to_string(),
                name: "Test Plugin".to_string(),
                version: "1.0.0".to_string(),
                publisher: "gh:test".to_string(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".to_string(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".to_string(),
            },
            capabilities: vec![],
            tools: vec![ToolDef {
                id: "echo".to_string(),
                summary: "Echo input.".to_string(),
                risk: ToolRisk::ReadOnly,
                input_schema: None,
                capabilities: vec![],
            }],
        };
        sign_plugin_manifest_for_tests(&mut manifest, wasm_content);
        fs::write(
            plugin_dir.join("plugin.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(plugin_dir.join("plugin.wasm"), wasm_content).unwrap();
    }

    #[test]
    fn discover_plugins() {
        let (tmp, defaults) = setup();
        write_plugin(tmp.path(), "test-plugin", b"fake wasm");

        let plugin_dir = tmp.path().join("plugins");
        let lockfile_path = tmp.path().join("navi-plugins.lock");
        let orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugin_dir,
            lockfile_path,
            defaults,
        );

        let dirs = orchestrator.discover_plugins().unwrap();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].join("plugin.toml").exists());
    }

    #[test]
    fn discover_empty_dir() {
        let (tmp, defaults) = setup();
        let plugin_dir = tmp.path().join("plugins");
        fs::create_dir_all(&plugin_dir).unwrap();

        let lockfile_path = tmp.path().join("navi-plugins.lock");
        let orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugin_dir,
            lockfile_path,
            defaults,
        );

        let dirs = orchestrator.discover_plugins().unwrap();
        assert!(dirs.is_empty());
    }

    #[test]
    fn discover_nonexistent_dir() {
        let (tmp, defaults) = setup();
        let plugin_dir = tmp.path().join("nonexistent");
        let lockfile_path = tmp.path().join("navi-plugins.lock");
        let orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugin_dir,
            lockfile_path,
            defaults,
        );

        let dirs = orchestrator.discover_plugins().unwrap();
        assert!(dirs.is_empty());
    }

    #[test]
    fn wasm_hash_mismatch_rejected() {
        let (tmp, defaults) = setup();
        write_plugin(tmp.path(), "bad-hash", b"fake wasm");
        let plugins_root = tmp.path().join("plugins");
        let plugin_dir = plugins_root.join("bad-hash");
        fs::write(plugin_dir.join("plugin.wasm"), b"altered wasm bytes").unwrap();
        write_lockfile_entry(&plugins_root, "bad-hash", vec![]);

        let mut orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugins_root.clone(),
            plugins_root.join("navi-plugins.lock"),
            defaults,
        );

        let mut executor = ToolExecutor::new(
            navi_core::SecurityPolicy::new(
                tmp.path().to_path_buf(),
                tmp.path().join("data"),
                navi_core::SecurityConfig::default(),
            )
            .unwrap(),
        );

        let report = orchestrator.load_plugins(&mut executor).unwrap();
        assert!(!report.warnings.is_empty());
        assert!(
            report.warnings.iter().any(|w| w.contains("hash")),
            "expected hash mismatch, got {:?}",
            report.warnings
        );
    }

    #[test]
    fn load_requires_lockfile_entry() {
        let (tmp, defaults) = setup();
        write_plugin(tmp.path(), "locked", b"fake wasm");

        let plugins_root = tmp.path().join("plugins");
        let lockfile_path = plugins_root.join("navi-plugins.lock");
        let mut orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugins_root,
            lockfile_path,
            defaults,
        );

        let mut executor = ToolExecutor::new(
            navi_core::SecurityPolicy::new(
                tmp.path().to_path_buf(),
                tmp.path().join("data"),
                navi_core::SecurityConfig::default(),
            )
            .unwrap(),
        );

        let report = orchestrator.load_plugins(&mut executor).unwrap();
        assert_eq!(report.loaded.len(), 0);
        assert!(
            report.warnings.iter().any(|w| w.contains("lockfile")),
            "expected lockfile warning, got: {:?}",
            report.warnings
        );
    }

    #[test]
    fn invalid_signature_rejected() {
        let (tmp, defaults) = setup();
        write_plugin(tmp.path(), "bad-sig", b"fake wasm");
        let plugin_dir = tmp.path().join("plugins").join("bad-sig");
        let mut manifest = navi_plugin_manifest::parse_manifest(
            &fs::read_to_string(plugin_dir.join("plugin.toml")).unwrap(),
        )
        .unwrap();
        manifest.plugin.signature =
            "ed25519:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
                .into();
        fs::write(
            plugin_dir.join("plugin.toml"),
            toml::to_string(&manifest).unwrap(),
        )
        .unwrap();

        let plugins_root = tmp.path().join("plugins");
        write_lockfile_entry(&plugins_root, "bad-sig", vec![]);

        let mut orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugins_root.clone(),
            plugins_root.join("navi-plugins.lock"),
            defaults,
        );
        let mut executor = ToolExecutor::new(
            navi_core::SecurityPolicy::new(
                tmp.path().to_path_buf(),
                tmp.path().join("data"),
                navi_core::SecurityConfig::default(),
            )
            .unwrap(),
        );
        let report = orchestrator.load_plugins(&mut executor).unwrap();
        assert!(report.loaded.is_empty());
        assert!(
            report.warnings.iter().any(|w| w.contains("signature")),
            "expected signature failure, got {:?}",
            report.warnings
        );
    }

    // Requires a real wasm module; "fake wasm" bytes fail runtime load (loaded=0).
    #[test]
    #[ignore = "needs real wasm fixture, not placeholder bytes"]
    fn load_succeeds_with_approved_lockfile_entry() {
        let (tmp, defaults) = setup();
        write_plugin(tmp.path(), "ok", b"fake wasm");
        let plugins_root = tmp.path().join("plugins");
        write_lockfile_entry(&plugins_root, "ok", vec![]);

        let lockfile_path = plugins_root.join("navi-plugins.lock");
        let mut orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            plugins_root,
            lockfile_path,
            defaults,
        );

        let mut executor = ToolExecutor::new(
            navi_core::SecurityPolicy::new(
                tmp.path().to_path_buf(),
                tmp.path().join("data"),
                navi_core::SecurityConfig::default(),
            )
            .unwrap(),
        );

        let report = orchestrator.load_plugins(&mut executor).unwrap();
        assert_eq!(report.loaded.len(), 1);
        assert_eq!(report.loaded[0].plugin_id, "ok");
    }

    #[test]
    fn invalid_manifest_rejected() {
        let (tmp, defaults) = setup();
        let plugin_dir = tmp.path().join("plugins").join("bad-manifest");
        fs::create_dir_all(&plugin_dir).unwrap();
        fs::write(plugin_dir.join("plugin.toml"), "not valid toml [[[").unwrap();
        fs::write(plugin_dir.join("plugin.wasm"), b"fake").unwrap();

        let lockfile_path = tmp.path().join("navi-plugins.lock");
        let mut orchestrator = PluginOrchestrator::new(
            tmp.path().to_path_buf(),
            tmp.path().join("plugins"),
            lockfile_path,
            defaults,
        );

        let mut executor = ToolExecutor::new(
            navi_core::SecurityPolicy::new(
                tmp.path().to_path_buf(),
                tmp.path().join("data"),
                navi_core::SecurityConfig::default(),
            )
            .unwrap(),
        );

        let report = orchestrator.load_plugins(&mut executor).unwrap();
        assert!(!report.warnings.is_empty());
        assert!(report.warnings[0].contains("failed to parse"));
    }
}
