use std::path::{Path, PathBuf};

use anyhow::Result;
use navi_plugin_broker::{
    ChangeType, InstallApproval, ReconsentAction, Severity, UpdateReconsent,
    check_update_reconsent, prepare_install_approval,
};
use navi_plugin_manifest::{
    Lockfile, PluginManifest, aggregate_lockfile_path, compute_wasm_hash, installed_plugins_dir,
    lock_entry_from_manifest, parse_manifest, upsert_aggregate_lock_entry, validate,
};
use std::collections::BTreeSet;

use crate::runtime::spawn_runtime_task;
use crate::state::{PluginApprovalKind, PluginApprovalRequest};

/// Outcome of a TUI approval decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginApprovalDecision {
    Approved,
    Denied,
}

/// Build an `InstallApproval` from a manifest (used by both the modal and the CLI).
fn build_install_approval(manifest: &PluginManifest) -> InstallApproval {
    prepare_install_approval(manifest)
}

/// Load and validate a manifest from a path, returning the parsed manifest or an error.
pub(crate) fn load_and_validate_manifest(path: &Path) -> Result<PluginManifest> {
    if !path.exists() {
        anyhow::bail!("plugin directory not found: {}", path.display());
    }
    let manifest_path = path.join("plugin.toml");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.toml found in {}", path.display());
    }
    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow::anyhow!("failed to read plugin.toml: {}", e))?;
    let manifest = parse_manifest(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse plugin.toml: {}", e))?;
    validate(&manifest, navi_plugin_manifest::TrustLevel::Community)
        .map_err(|e| anyhow::anyhow!("manifest validation failed: {}", e))?;
    let wasm_path = path.join(&manifest.plugin.entry);
    if !wasm_path.exists() {
        anyhow::bail!("WASM binary not found: {}", wasm_path.display());
    }
    let wasm_bytes = std::fs::read(&wasm_path)
        .map_err(|e| anyhow::anyhow!("failed to read WASM binary: {}", e))?;
    let actual_hash = compute_wasm_hash(&wasm_bytes);
    if actual_hash != manifest.plugin.wasm_hash {
        anyhow::bail!(
            "WASM hash mismatch:\n  declared: {}\n  actual:   {}",
            manifest.plugin.wasm_hash,
            actual_hash
        );
    }
    Ok(manifest)
}

/// Pre-format strings for the TUI modal.
fn build_approval_request(
    id: String,
    source_path: String,
    manifest: &PluginManifest,
    kind: PluginApprovalKind,
    reconsent: Option<&UpdateReconsent>,
    install_on_approve: bool,
) -> PluginApprovalRequest {
    let approval = build_install_approval(manifest);
    let mut capabilities_text = String::new();
    for cap in &approval.capabilities {
        let icon = match cap.severity {
            Severity::Low => "  ",
            Severity::Medium => "  ",
            Severity::High => "! ",
            Severity::Critical => "!!",
        };
        capabilities_text.push_str(&format!(
            "{} [{}] {}: {}\n",
            icon, cap.severity, cap.kind, cap.description
        ));
    }
    let mut tools_text = String::new();
    for tool in &approval.tools {
        tools_text.push_str(&format!("  {} [{}] {}\n", tool.id, tool.risk, tool.summary));
    }
    let warnings_text = approval.warnings.join("\n");

    let reconsent_action = reconsent.map(|r| match r.action {
        ReconsentAction::Allow => "ALLOW".to_string(),
        ReconsentAction::RequireReconsent => "REQUIRE RECONSENT".to_string(),
        ReconsentAction::Block => "BLOCKED".to_string(),
    });
    let changes_text = reconsent
        .map(|r| {
            let mut s = String::new();
            for c in &r.changes {
                s.push_str(&format!(
                    "  {} {}\n",
                    change_type_glyph(&c.change_type),
                    c.description
                ));
            }
            s
        })
        .unwrap_or_default();

    PluginApprovalRequest {
        id,
        source_path,
        plugin_id: manifest.plugin.id.clone(),
        version: manifest.plugin.version.clone(),
        publisher: manifest.plugin.publisher.clone(),
        overall_risk: approval.overall_risk.to_string(),
        capabilities_text,
        tools_text,
        warnings_text,
        kind,
        changes_text,
        reconsent_action,
        install_on_approve,
    }
}

fn change_type_glyph(ct: &ChangeType) -> &'static str {
    match ct {
        ChangeType::CapabilityAdded => "+",
        ChangeType::CapabilityRemoved => "-",
        ChangeType::ToolAdded => "+",
        ChangeType::ToolRemoved => "-",
        ChangeType::ToolRiskIncreased => "!",
        ChangeType::ToolSchemaChanged => "~",
        ChangeType::PublisherChanged => "X",
        ChangeType::SigningKeyChanged => "X",
        ChangeType::CodeChanged => "~",
        ChangeType::MinimumNaviIncreased => "!",
    }
}

/// Build a request to install a new plugin and return the corresponding `PluginApprovalRequest`.
pub(crate) fn build_install_request(
    id: String,
    source_path: &Path,
    manifest: &PluginManifest,
) -> PluginApprovalRequest {
    build_approval_request(
        id,
        source_path.display().to_string(),
        manifest,
        PluginApprovalKind::Install,
        None,
        true,
    )
}

/// Build a request to update an existing plugin; performs reconsent check.
pub(crate) fn build_update_request(
    id: String,
    source_path: &Path,
    new_manifest: &PluginManifest,
    installed_dir: &Path,
) -> Result<PluginApprovalRequest> {
    // Load installed manifest + lockfile
    let installed_manifest_path = installed_dir.join("plugin.toml");
    if !installed_manifest_path.exists() {
        anyhow::bail!(
            "installed plugin has no manifest at {}",
            installed_manifest_path.display()
        );
    }
    let plugins_root = installed_dir.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "invalid installed plugin path (expected .../plugins/<id>): {}",
            installed_dir.display()
        )
    })?;
    let installed_lockfile_path = aggregate_lockfile_path(plugins_root);
    let old_manifest = parse_manifest(&std::fs::read_to_string(&installed_manifest_path)?)
        .map_err(|e| anyhow::anyhow!("failed to parse installed manifest: {}", e))?;
    let lockfile = Lockfile::load(&installed_lockfile_path).unwrap_or_default();
    let old_entry = lockfile.find(&new_manifest.plugin.id).ok_or_else(|| {
        anyhow::anyhow!("no lockfile entry for plugin '{}'", new_manifest.plugin.id)
    })?;

    let reconsent = check_update_reconsent(old_entry, new_manifest, &old_manifest);
    if reconsent.action == ReconsentAction::Block {
        anyhow::bail!(
            "update blocked (publisher or signing key changed); use CLI with --force to override"
        );
    }

    Ok(build_approval_request(
        id,
        source_path.display().to_string(),
        new_manifest,
        PluginApprovalKind::Update,
        Some(&reconsent),
        true,
    ))
}

/// Format a request into a human-readable summary (used by `info_plugin` and notifications).
pub(crate) fn format_request_for_log(req: &PluginApprovalRequest) -> String {
    match req.kind {
        PluginApprovalKind::Install => format_install_approval_for_request(req),
        PluginApprovalKind::Update => format_update_reconsent_for_request(req),
    }
}

fn format_install_approval_for_request(req: &PluginApprovalRequest) -> String {
    let mut out = String::new();
    out.push_str(&format!("Plugin: {} v{}\n", req.plugin_id, req.version));
    out.push_str(&format!("Publisher: {}\n", req.publisher));
    out.push_str(&format!("Overall risk: {}\n\n", req.overall_risk));
    if !req.capabilities_text.is_empty() {
        out.push_str("Capabilities:\n");
        out.push_str(&req.capabilities_text);
        out.push('\n');
    }
    if !req.tools_text.is_empty() {
        out.push_str("Tools:\n");
        out.push_str(&req.tools_text);
        out.push('\n');
    }
    if !req.warnings_text.is_empty() {
        out.push_str("Warnings:\n");
        out.push_str(&req.warnings_text);
        out.push('\n');
    }
    out
}

fn format_update_reconsent_for_request(req: &PluginApprovalRequest) -> String {
    let mut out = String::new();
    out.push_str(&format!("Plugin: {} (update)\n", req.plugin_id));
    if let Some(action) = &req.reconsent_action {
        out.push_str(&format!("Action: {}\n\n", action));
    }
    if !req.changes_text.is_empty() {
        out.push_str("Changes:\n");
        out.push_str(&req.changes_text);
        out.push('\n');
    }
    if !req.warnings_text.is_empty() {
        out.push_str("Warnings:\n");
        out.push_str(&req.warnings_text);
        out.push('\n');
    }
    out
}

/// Apply a plugin install/update to disk (copy files + update lockfile).
/// Used after a TUI approval to actually install the plugin.
pub(crate) fn apply_plugin_install(
    source_path: &Path,
    manifest: &PluginManifest,
    data_dir: &Path,
    kind: PluginApprovalKind,
) -> Result<PathBuf> {
    let plugin_dir = data_dir.join("plugins").join(&manifest.plugin.id);
    if plugin_dir.exists() {
        std::fs::remove_dir_all(&plugin_dir)
            .map_err(|e| anyhow::anyhow!("failed to remove existing plugin: {}", e))?;
    }
    copy_dir_recursive(source_path, &plugin_dir)
        .map_err(|e| anyhow::anyhow!("failed to copy plugin: {}", e))?;

    let plugins_root = installed_plugins_dir(data_dir);
    let approved_capabilities = approved_capabilities_for_apply(data_dir, manifest, kind)?;
    let entry = lock_entry_from_manifest(manifest, approved_capabilities);
    upsert_aggregate_lock_entry(&plugins_root, entry)
        .map_err(|e| anyhow::anyhow!("failed to save lockfile: {}", e))?;
    Ok(plugin_dir)
}

fn approved_capabilities_for_apply(
    data_dir: &Path,
    manifest: &PluginManifest,
    kind: PluginApprovalKind,
) -> Result<Vec<String>> {
    match kind {
        PluginApprovalKind::Install => Ok(manifest
            .capabilities
            .iter()
            .map(|c| c.id().to_string())
            .collect()),
        PluginApprovalKind::Update => {
            let lockfile =
                Lockfile::load(&aggregate_lockfile_path(&installed_plugins_dir(data_dir)))
                    .unwrap_or_default();
            let mut approved: BTreeSet<String> = lockfile
                .find(&manifest.plugin.id)
                .map(|entry| entry.approved_capabilities.iter().cloned().collect())
                .unwrap_or_default();
            for cap in &manifest.capabilities {
                approved.insert(cap.id().to_string());
            }
            Ok(approved.into_iter().collect())
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            std::fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}

/// Submit an install/update to disk after a TUI approval.
/// Performs the file copy and lockfile update.
pub(crate) fn approve_plugin_install(app: &mut crate::TuiApp, req: PluginApprovalRequest) {
    use crate::notifications::show_notification;

    if !req.install_on_approve {
        return;
    }
    if req.reconsent_action.as_deref() == Some("BLOCKED") {
        notify_plugin_decision(app, &req, PluginApprovalDecision::Denied);
        return;
    }

    tracing::info!(
        plugin = %req.plugin_id,
        summary = %format_request_for_log(&req),
        "plugin approval granted"
    );

    let data_dir = app.loaded_config.data_dir.clone();
    let source_path = PathBuf::from(&req.source_path);
    let source_path_for_async = source_path.clone();
    let plugin_id = req.plugin_id.clone();
    let kind = req.kind;

    // Run on background thread to keep TUI responsive.
    let req_id = req.id.clone();
    let tx = app.async_sender();
    spawn_runtime_task(async move {
        let result = (|| -> Result<PathBuf> {
            let manifest = load_and_validate_manifest(&source_path_for_async)?;
            apply_plugin_install(&source_path_for_async, &manifest, &data_dir, kind)
        })();
        match result {
            Ok(path) => {
                tracing::info!(
                    request_id = %req_id,
                    path = %path.display(),
                    "plugin installed/updated"
                );
                let _ = tx.send(crate::dispatch::AsyncEvent::PluginsReloadNeeded);
            }
            Err(e) => {
                tracing::warn!(
                    request_id = %req_id,
                    "plugin install failed: {e:#}"
                );
            }
        }
    });

    // Best-effort sync install for small plugins. If it completes synchronously,
    // show a notification right away; otherwise defer to the async task.
    if let Ok(manifest) = load_and_validate_manifest(&source_path)
        && let Ok(installed) = apply_plugin_install(
            &source_path,
            &manifest,
            &app.loaded_config.data_dir,
            req.kind,
        )
    {
        show_notification(
            app,
            match req.kind {
                PluginApprovalKind::Install => "Plugin",
                PluginApprovalKind::Update => "Plugin update",
            },
            format!("Installed {plugin_id} → {}", installed.display()),
        );
        notify_plugin_decision(app, &req, PluginApprovalDecision::Approved);
        crate::plugins::reload_engine_plugins(app);
        return;
    }

    show_notification(
        app,
        match req.kind {
            PluginApprovalKind::Install => "Plugin",
            PluginApprovalKind::Update => "Plugin update",
        },
        format!("Installing {plugin_id} (async)..."),
    );
}

/// Notify (no-op) when a plugin install is denied.
pub(crate) fn notify_plugin_decision(
    app: &mut crate::TuiApp,
    req: &PluginApprovalRequest,
    decision: PluginApprovalDecision,
) {
    use crate::notifications::show_notification;
    let title = match req.kind {
        PluginApprovalKind::Install => "Plugin",
        PluginApprovalKind::Update => "Plugin update",
    };
    let verb = match decision {
        PluginApprovalDecision::Approved => "approved",
        PluginApprovalDecision::Denied => "denied",
    };
    show_notification(app, title, format!("Install {} {verb}.", req.plugin_id));
}

/// Count the number of installed plugins on disk.
pub(crate) fn count_installed_plugins(app: &crate::TuiApp) -> usize {
    let plugin_dir = app.loaded_config.data_dir.join("plugins");
    if !plugin_dir.exists() {
        return 0;
    }
    std::fs::read_dir(&plugin_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir() && e.path().join("plugin.toml").exists())
                .count()
        })
        .unwrap_or(0)
}

/// Request a TUI approval for installing a plugin from a path.
/// Loads the manifest, builds the approval request, and pushes it onto the queue.
pub(crate) fn request_install_approval(app: &mut crate::TuiApp, source_path: &Path) -> Result<()> {
    use crate::keybindings::open_modal;
    use crate::state::ModalKind;

    let manifest = load_and_validate_manifest(source_path)?;
    let req = build_install_request(
        format!("install:{}", manifest.plugin.id),
        source_path,
        &manifest,
    );
    let path_for_log = source_path.display().to_string();
    tracing::info!(
        plugin = %manifest.plugin.id,
        source = %path_for_log,
        summary = %format_request_for_log(&req),
        "requesting plugin install approval"
    );
    app.pending_plugin_approvals.push(req);
    app.plugin_approval_scroll = 0;
    open_modal(app, ModalKind::PluginApproval);
    Ok(())
}

/// Request a TUI approval for updating an installed plugin from a path.
pub(crate) fn request_update_approval(app: &mut crate::TuiApp, source_path: &Path) -> Result<()> {
    use crate::keybindings::open_modal;
    use crate::state::ModalKind;

    let manifest = load_and_validate_manifest(source_path)?;
    let installed_dir = app
        .loaded_config
        .data_dir
        .join("plugins")
        .join(&manifest.plugin.id);
    if !installed_dir.exists() {
        anyhow::bail!(
            "plugin '{}' is not installed; use install instead of update",
            manifest.plugin.id
        );
    }
    let req = build_update_request(
        format!("update:{}", manifest.plugin.id),
        source_path,
        &manifest,
        &installed_dir,
    )?;
    let path_for_log = source_path.display().to_string();
    tracing::info!(
        plugin = %manifest.plugin.id,
        source = %path_for_log,
        summary = %format_request_for_log(&req),
        "requesting plugin update approval"
    );
    app.pending_plugin_approvals.push(req);
    app.plugin_approval_scroll = 0;
    open_modal(app, ModalKind::PluginApproval);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_plugin_manifest::{PluginMeta, RuntimeKind};
    use tempfile::tempdir;

    fn make_manifest(id: &str, version: &str, publisher: &str) -> PluginManifest {
        use navi_plugin_manifest::sign_plugin_manifest_for_tests;

        let wasm = b"test-wasm-bytes";
        let mut manifest = PluginManifest {
            plugin: PluginMeta {
                id: id.to_string(),
                name: id.to_string(),
                version: version.to_string(),
                publisher: publisher.to_string(),
                runtime: RuntimeKind::WasmComponent,
                entry: "plugin.wasm".to_string(),
                wasm_hash: String::new(),
                signature: String::new(),
                public_key: None,
                minimum_navi: "0.1.0".to_string(),
            },
            capabilities: vec![],
            tools: vec![],
        };
        sign_plugin_manifest_for_tests(&mut manifest, wasm);
        manifest
    }

    #[test]
    fn install_request_kind_is_install() {
        let m = make_manifest("test", "1.0.0", "gh:test");
        let req = build_install_request("id-1".into(), Path::new("/tmp/x"), &m);
        assert_eq!(req.kind, PluginApprovalKind::Install);
        assert_eq!(req.plugin_id, "test");
    }

    #[test]
    fn update_request_kind_is_update() {
        let tmp = tempdir().unwrap();
        let plugins_root = tmp.path().join("plugins");
        let installed_dir = plugins_root.join("u");
        std::fs::create_dir_all(&installed_dir).unwrap();

        let old = make_manifest("u", "1.0.0", "gh:test");
        let installed_manifest = toml::to_string(&old).unwrap();
        std::fs::write(installed_dir.join("plugin.toml"), &installed_manifest).unwrap();

        // Pre-populate aggregate lockfile with an entry for the old manifest.
        let mut lockfile = Lockfile::default();
        lockfile.upsert(navi_plugin_manifest::LockEntry {
            id: "u".to_string(),
            version: "1.0.0".to_string(),
            publisher: "gh:test".to_string(),
            wasm_hash: format!("sha256:{}", "0".repeat(64)),
            capabilities_hash: navi_plugin_manifest::compute_content_hash(""),
            tools_hash: navi_plugin_manifest::compute_content_hash(""),
            approved_capabilities: vec![],
            approved_at: "0".to_string(),
            trust_level: navi_plugin_manifest::TrustLevel::Community,
            kind: navi_plugin_manifest::PluginCatalogKind::Plugin,
        });
        lockfile
            .save(&aggregate_lockfile_path(&plugins_root))
            .unwrap();

        let mut new = old.clone();
        new.plugin.version = "1.1.0".to_string();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        let req = build_update_request("u-id".into(), &src, &new, &installed_dir).unwrap();
        assert_eq!(req.kind, PluginApprovalKind::Update);
    }

    #[test]
    fn apply_install_copies_files_and_lockfile() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("plugin.toml"), "x = 1").unwrap();
        std::fs::write(src.join("plugin.wasm"), b"wasm").unwrap();

        let manifest = make_manifest("p1", "1.0.0", "gh:t");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let dst =
            apply_plugin_install(&src, &manifest, &data, PluginApprovalKind::Install).unwrap();
        assert!(dst.join("plugin.toml").exists());
        assert!(dst.join("plugin.wasm").exists());
        let aggregate = installed_plugins_dir(&data).join("navi-plugins.lock");
        assert!(
            aggregate.exists(),
            "aggregate lockfile at {}",
            aggregate.display()
        );
        assert!(
            !dst.join("navi-plugins.lock").exists(),
            "per-plugin lockfile should not be created"
        );
    }
}
