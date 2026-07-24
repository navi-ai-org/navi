use anyhow::Result;
use navi_core::LoadedConfig;
use navi_sdk::NaviEngineBuilder;

use crate::RegistryAction;

pub async fn handle_registry_command(
    action: RegistryAction,
    loaded_config: &LoadedConfig,
    cwd: &std::path::Path,
) -> Result<()> {
    match action {
        RegistryAction::Sync => sync_registry(loaded_config, cwd).await,
        RegistryAction::List => list_registry(loaded_config, cwd).await,
    }
}

async fn sync_registry(loaded_config: &LoadedConfig, cwd: &std::path::Path) -> Result<()> {
    let engine = NaviEngineBuilder::from_project(cwd)
        .loaded_config(loaded_config.clone())
        .build()?;

    println!("Syncing provider registry from remote database...");
    let updated = engine.sync_registry(true).await?;

    if updated {
        println!("Registry updated successfully.");
    } else {
        println!("Registry is already up to date.");
    }

    Ok(())
}

async fn list_registry(loaded_config: &LoadedConfig, cwd: &std::path::Path) -> Result<()> {
    let engine = NaviEngineBuilder::from_project(cwd)
        .loaded_config(loaded_config.clone())
        .build()?;

    // Force a non-forced sync to ensure the cache is fresh if possible,
    // but don't fail if the network is unavailable.
    let _ = engine.sync_registry(false).await;

    let providers = navi_core::provider_catalog(&loaded_config.config);

    if providers.is_empty() {
        println!("No providers available in the registry.");
        return Ok(());
    }

    let total_models: usize = providers.iter().map(|p| p.models.len()).sum();

    println!(
        "Registry: {} providers, {} models\n",
        providers.len(),
        total_models
    );
    println!("{:<25} {:<20} {:>6} KIND", "ID", "LABEL", "MODELS");
    println!("{}", "-".repeat(70));

    for provider in &providers {
        println!(
            "{:<25} {:<20} {:>6} {}",
            provider.id,
            provider.label,
            provider.models.len(),
            format!("{:?}", provider.kind).to_lowercase(),
        );
    }

    Ok(())
}
