use anyhow::{Context, Result};
use navi_core::LoadedConfig;
use navi_plugin_broker::{ReconsentAction, check_update_reconsent, prepare_install_approval};
use navi_plugin_manifest::{
    Lockfile, TrustLevel, aggregate_lockfile_path, compute_wasm_hash, installed_plugins_dir,
    lock_entry_from_manifest, parse_manifest, registry_url, remove_aggregate_lock_entry,
    search_catalog, stage_plugin_by_id, upsert_aggregate_lock_entry, validate,
};
use std::fs;
use std::path::Path;

use crate::PluginAction;

pub fn handle_plugin_command(
    action: PluginAction,
    config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    match action {
        PluginAction::Install { path, yes } => install_plugin(&path, yes, config, cwd),
        PluginAction::InstallMarketplace { plugin_id, yes } => {
            install_plugin_marketplace(&plugin_id, yes, config)
        }
        PluginAction::Update { path, force } => update_plugin(&path, force, config, cwd),
        PluginAction::UpdateMarketplace { plugin_id, force } => {
            update_plugin_marketplace(&plugin_id, force, config)
        }
        PluginAction::Search { query } => search_marketplace(query.as_deref(), config),
        PluginAction::List => list_plugins(config, cwd),
        PluginAction::Remove { plugin_id } => remove_plugin(&plugin_id, config, cwd),
        PluginAction::Info { plugin_id } => show_plugin_info(&plugin_id, config, cwd),
    }
}

fn registry_for_config(config: &LoadedConfig) -> &str {
    registry_url(
        config
            .config
            .plugin_marketplace
            .registry_url
            .as_deref(),
    )
}

fn search_marketplace(query: Option<&str>, config: &LoadedConfig) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to start async runtime")?;
    let catalog = rt
        .block_on(navi_plugin_manifest::fetch_catalog(registry_for_config(config)))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let q = query.unwrap_or("");
    let hits = search_catalog(&catalog, q);
    if hits.is_empty() {
        println!("No marketplace plugins match '{}'.", q);
        return Ok(());
    }
    for entry in hits {
        println!(
            "  {} v{} — {} ({})",
            entry.id, entry.version, entry.name, entry.publisher
        );
        if !entry.description.is_empty() {
            println!("    {}", entry.description);
        }
    }
    Ok(())
}

fn install_plugin_marketplace(plugin_id: &str, yes: bool, config: &LoadedConfig) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to start async runtime")?;
    let (_, staging) = rt
        .block_on(stage_plugin_by_id(
            registry_for_config(config),
            plugin_id,
            &config.data_dir,
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    install_plugin(&staging, yes, config, staging.parent().unwrap_or(&staging))
}

fn update_plugin_marketplace(plugin_id: &str, force: bool, config: &LoadedConfig) -> Result<()> {
    let rt = tokio::runtime::Runtime::new().context("failed to start async runtime")?;
    let (_, staging) = rt
        .block_on(stage_plugin_by_id(
            registry_for_config(config),
            plugin_id,
            &config.data_dir,
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    update_plugin(&staging, force, config, staging.parent().unwrap_or(&staging))
}

fn install_plugin(path: &Path, yes: bool, config: &LoadedConfig, _cwd: &Path) -> Result<()> {
    let manifest = load_and_validate_manifest(path)?;

    // Display install approval
    let approval = prepare_install_approval(&manifest);
    println!("{}", navi_plugin_broker::format_install_approval(&approval));

    if !yes && !prompt_yes_no("Install this plugin? [y/N] ")? {
        println!("Install cancelled.");
        return Ok(());
    }

    println!(
        "Installing plugin '{}' v{}...",
        manifest.plugin.id, manifest.plugin.version
    );

    install_files(path, &manifest, &config.data_dir)?;
    write_lockfile(&config.data_dir, &manifest)?;

    println!("Plugin '{}' installed successfully.", manifest.plugin.id);
    println!("  Tools: {}", manifest.tools.len());
    println!("  Capabilities: {}", manifest.capabilities.len());

    Ok(())
}

fn update_plugin(path: &Path, force: bool, config: &LoadedConfig, _cwd: &Path) -> Result<()> {
    let new_manifest = load_and_validate_manifest(path)?;
    let plugin_id = new_manifest.plugin.id.clone();
    let installed_dir = config.data_dir.join("plugins").join(&plugin_id);
    if !installed_dir.exists() {
        anyhow::bail!(
            "plugin '{}' is not installed; use `navi plugin install` to install it",
            plugin_id
        );
    }

    // Load installed manifest + lockfile entry.
    let old_manifest_path = installed_dir.join("plugin.toml");
    let old_manifest = parse_manifest(&fs::read_to_string(&old_manifest_path).context(format!(
        "failed to read installed manifest at {}",
        old_manifest_path.display()
    ))?)
    .context("failed to parse installed manifest")?;
    let plugins_root = installed_plugins_dir(&config.data_dir);
    let lockfile = Lockfile::load(&aggregate_lockfile_path(&plugins_root)).unwrap_or_default();
    let old_entry = lockfile
        .find(&plugin_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "plugin '{}' has no lockfile entry; reinstall with `navi plugin install`",
                plugin_id
            )
        })?
        .clone();

    // Run reconsent check.
    let reconsent = check_update_reconsent(&old_entry, &new_manifest, &old_manifest);
    println!(
        "{}",
        navi_plugin_broker::format_update_reconsent(&reconsent)
    );

    match reconsent.action {
        ReconsentAction::Block => {
            if !force {
                anyhow::bail!("update blocked (publisher change); re-run with --force to override");
            }
            println!("--force: applying update despite publisher change");
        }
        ReconsentAction::RequireReconsent => {
            if !prompt_yes_no("Re-consent to the new capabilities/changes? [y/N] ")? {
                println!("Update cancelled.");
                return Ok(());
            }
        }
        ReconsentAction::Allow => {
            // proceed silently
        }
    }

    println!(
        "Updating plugin '{}' from v{} to v{}...",
        plugin_id, old_manifest.plugin.version, new_manifest.plugin.version
    );

    install_files(path, &new_manifest, &config.data_dir)?;

    // Update lockfile: union of approved capabilities (preserve old + add new).
    let mut approved_caps: std::collections::BTreeSet<String> =
        old_entry.approved_capabilities.into_iter().collect();
    for cap in &new_manifest.capabilities {
        approved_caps.insert(cap.id().to_string());
    }
    write_lockfile_with_approved(
        &config.data_dir,
        &new_manifest,
        approved_caps.into_iter().collect(),
    )?;

    println!("Plugin '{}' updated successfully.", plugin_id);
    println!("  Tools: {}", new_manifest.tools.len());
    println!("  Capabilities: {}", new_manifest.capabilities.len());
    Ok(())
}

fn load_and_validate_manifest(path: &Path) -> Result<navi_plugin_manifest::PluginManifest> {
    if !path.exists() {
        anyhow::bail!("plugin directory not found: {}", path.display());
    }
    let manifest_path = path.join("plugin.toml");
    if !manifest_path.exists() {
        anyhow::bail!("no plugin.toml found in {}", path.display());
    }
    let manifest_content =
        fs::read_to_string(&manifest_path).context("failed to read plugin.toml")?;
    let manifest = parse_manifest(&manifest_content).context("failed to parse plugin.toml")?;
    validate(&manifest, TrustLevel::Community).context("manifest validation failed")?;

    let wasm_path = path.join(&manifest.plugin.entry);
    if !wasm_path.exists() {
        anyhow::bail!("WASM binary not found: {}", wasm_path.display());
    }
    let wasm_bytes = fs::read(&wasm_path).context("failed to read WASM binary")?;
    let actual_hash = compute_wasm_hash(&wasm_bytes);
    if actual_hash != manifest.plugin.wasm_hash {
        anyhow::bail!(
            "WASM hash mismatch:\n  declared: {}\n  actual:   {}",
            manifest.plugin.wasm_hash,
            actual_hash
        );
    }
    navi_plugin_manifest::verify_plugin_signature(&manifest, &wasm_bytes, TrustLevel::Community)
        .map_err(|reason| anyhow::anyhow!("signature verification failed: {reason}"))?;
    Ok(manifest)
}

fn install_files(
    source_path: &Path,
    _manifest: &navi_plugin_manifest::PluginManifest,
    data_dir: &Path,
) -> Result<std::path::PathBuf> {
    let plugin_dir = data_dir.join("plugins").join(&_manifest.plugin.id);
    if plugin_dir.exists() {
        fs::remove_dir_all(&plugin_dir).context("failed to remove existing plugin")?;
    }
    copy_dir_recursive(source_path, &plugin_dir).context("failed to copy plugin")?;
    Ok(plugin_dir)
}

fn write_lockfile(data_dir: &Path, manifest: &navi_plugin_manifest::PluginManifest) -> Result<()> {
    let approved = manifest
        .capabilities
        .iter()
        .map(|c| c.id().to_string())
        .collect();
    write_lockfile_with_approved(data_dir, manifest, approved)
}

fn write_lockfile_with_approved(
    data_dir: &Path,
    manifest: &navi_plugin_manifest::PluginManifest,
    approved_capabilities: Vec<String>,
) -> Result<()> {
    let plugins_root = installed_plugins_dir(data_dir);
    let entry = lock_entry_from_manifest(manifest, approved_capabilities);
    upsert_aggregate_lock_entry(&plugins_root, entry)
        .map_err(|e| anyhow::anyhow!("failed to save lockfile: {}", e))?;
    Ok(())
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout()
        .flush()
        .context("failed to flush stdout")?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn list_plugins(config: &LoadedConfig, _cwd: &Path) -> Result<()> {
    let plugin_dir = config.data_dir.join("plugins");

    if !plugin_dir.exists() {
        println!("No plugins installed.");
        return Ok(());
    }

    let mut found = false;
    for entry in fs::read_dir(&plugin_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("plugin.toml");
        if !manifest_path.exists() {
            continue;
        }

        let manifest_content = fs::read_to_string(&manifest_path)?;
        match parse_manifest(&manifest_content) {
            Ok(manifest) => {
                found = true;
                let tool_names: Vec<&str> = manifest.tools.iter().map(|t| t.id.as_str()).collect();
                println!(
                    "  {} v{} ({}) — tools: [{}]",
                    manifest.plugin.id,
                    manifest.plugin.version,
                    manifest.plugin.publisher,
                    tool_names.join(", ")
                );
            }
            Err(e) => {
                println!("  {} (invalid manifest: {})", path.display(), e);
            }
        }
    }

    if !found {
        println!("No plugins installed.");
    }

    Ok(())
}

fn remove_plugin(plugin_id: &str, config: &LoadedConfig, _cwd: &Path) -> Result<()> {
    let plugin_dir = config.data_dir.join("plugins").join(plugin_id);

    if !plugin_dir.exists() {
        anyhow::bail!("plugin '{}' not found", plugin_id);
    }

    // Confirm removal
    println!("Removing plugin '{}'...", plugin_id);
    fs::remove_dir_all(&plugin_dir).context("failed to remove plugin directory")?;

    let plugins_root = installed_plugins_dir(&config.data_dir);
    if let Err(e) = remove_aggregate_lock_entry(&plugins_root, plugin_id) {
        tracing::warn!(plugin = plugin_id, error = %e, "failed to update aggregate lockfile on remove");
    }

    println!("Plugin '{}' removed.", plugin_id);
    Ok(())
}

fn show_plugin_info(plugin_id: &str, config: &LoadedConfig, _cwd: &Path) -> Result<()> {
    let plugin_dir = config.data_dir.join("plugins").join(plugin_id);

    if !plugin_dir.exists() {
        // Try as a path
        let path = Path::new(plugin_id);
        if path.exists() && path.join("plugin.toml").exists() {
            let manifest_content = fs::read_to_string(path.join("plugin.toml"))?;
            let manifest = parse_manifest(&manifest_content)?;
            let approval = navi_plugin_broker::prepare_install_approval(&manifest);
            println!("{}", navi_plugin_broker::format_install_approval(&approval));
            return Ok(());
        }
        anyhow::bail!("plugin '{}' not found", plugin_id);
    }

    let manifest_path = plugin_dir.join("plugin.toml");
    let manifest_content = fs::read_to_string(&manifest_path)?;
    let manifest = parse_manifest(&manifest_content)?;

    let approval = navi_plugin_broker::prepare_install_approval(&manifest);
    println!("{}", navi_plugin_broker::format_install_approval(&approval));

    // Show lockfile info
    let plugins_root = installed_plugins_dir(&config.data_dir);
    let lockfile_path = aggregate_lockfile_path(&plugins_root);
    if let Ok(lockfile) = navi_plugin_manifest::Lockfile::load(&lockfile_path) {
        if let Some(entry) = lockfile.find(plugin_id) {
            println!("Installed: {}", entry.approved_at);
            println!("WASM hash: {}", entry.wasm_hash);
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest_path = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &dest_path)?;
        } else {
            fs::copy(&path, &dest_path)?;
        }
    }
    Ok(())
}
