//! `navi provider` — manage custom providers and models in `providers.d/`.
//!
//! NAVI never rewrites a hand-authored `config.toml`, and provider definitions
//! no longer live there: every provider is a file under
//! `~/.config/navi/providers.d/` (or `<project>/.navi/providers.d/`). These
//! subcommands are the "without much effort" path — add, extend, edit, remove —
//! while hand-editing the same files works just as well and is picked up at
//! runtime.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use navi_core::LoadedConfig;
use navi_core::config::providers::dropin::{self, DropInRoots};
use navi_core::{ProviderConfig, ProviderKind, ProviderModelConfig};

use crate::ProviderAction;

/// Resolves the drop-in roots for the current process.
fn roots_for(loaded_config: &LoadedConfig, cwd: &Path) -> DropInRoots {
    let global_dir = loaded_config
        .global_config_path
        .as_ref()
        .and_then(|path| path.parent());
    dropin::roots_for(global_dir, cwd)
}

/// Directory a write should land in (`--project` picks the project root).
fn write_dir(roots: &DropInRoots, project: bool) -> Result<PathBuf> {
    if project {
        roots
            .project
            .clone()
            .ok_or_else(|| anyhow::anyhow!("project providers.d directory is unavailable"))
    } else {
        roots
            .global
            .clone()
            .ok_or_else(|| anyhow::anyhow!("global providers.d directory is unavailable"))
    }
}

pub fn handle_provider_command(
    action: ProviderAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    let roots = roots_for(loaded_config, cwd);
    match action {
        ProviderAction::List { json } => list(&roots, loaded_config, json),
        ProviderAction::Path { project } => {
            println!("{}", write_dir(&roots, project)?.display());
            Ok(())
        }
        ProviderAction::Check { json } => check(&roots, json),
        ProviderAction::Add {
            id,
            label,
            kind,
            base_url,
            api_key_env,
            model,
            project,
        } => add(
            &roots,
            project,
            AddArgs {
                id,
                label,
                kind,
                base_url,
                api_key_env,
                models: model,
            },
        ),
        ProviderAction::AddModel {
            provider,
            name,
            context_window,
            max_output,
            thinking,
            images,
            temperature,
            project,
        } => add_model(
            &roots,
            project,
            &provider,
            ProviderModelConfig {
                name,
                context_window_tokens: context_window,
                max_output_tokens: max_output,
                supports_thinking: thinking.then_some(true),
                supports_images: images.then_some(true),
                recommended_temperature: temperature,
                ..Default::default()
            },
        ),
        ProviderAction::Edit { id, project } => edit(&roots, loaded_config, project, &id),
        ProviderAction::Remove { id, project, yes } => remove(&roots, project, &id, yes),
        ProviderAction::Migrate { dry_run } => migrate(loaded_config, &roots, dry_run),
    }
}

fn list(roots: &DropInRoots, loaded_config: &LoadedConfig, json: bool) -> Result<()> {
    let catalog = navi_core::provider_catalog(&loaded_config.config);
    let files = dropin::provider_file_map(roots);
    let config_ids: HashSet<String> = loaded_config
        .config
        .providers
        .iter()
        .map(|provider| navi_core::canonical_provider_id(&provider.id).to_string())
        .collect();

    if json {
        let rows: Vec<serde_json::Value> = catalog
            .iter()
            .map(|provider| {
                serde_json::json!({
                    "id": provider.id,
                    "label": provider.label,
                    "kind": format!("{:?}", provider.kind).to_lowercase(),
                    "base_url": provider.base_url,
                    "api_key_env": provider.api_key_env,
                    "models": provider.models.len(),
                    "source": source_for(&provider.id, &files, &config_ids),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    println!(
        "{:<22} {:<26} {:>6}  {:<8} {}",
        "PROVIDER", "KIND", "MODELS", "SOURCE", "BASE URL"
    );
    println!("{}", "-".repeat(110));
    for provider in &catalog {
        println!(
            "{:<22} {:<26} {:>6}  {:<8} {}",
            provider.id,
            format!("{:?}", provider.kind).to_lowercase(),
            provider.models.len(),
            source_for(&provider.id, &files, &config_ids),
            provider.base_url.as_deref().unwrap_or("-")
        );
    }
    if catalog.is_empty() {
        println!("(no providers)");
    }
    println!();
    for (label, dir) in [
        ("global ", roots.global.as_ref()),
        ("project", roots.project.as_ref()),
    ] {
        let Some(dir) = dir else { continue };
        let exists = if dir.is_dir() { "" } else { " (not created yet)" };
        println!("{label}: {}{exists}", dir.display());
    }
    Ok(())
}

fn source_for(
    provider_id: &str,
    files: &std::collections::BTreeMap<String, PathBuf>,
    config_ids: &HashSet<String>,
) -> String {
    let id = navi_core::canonical_provider_id(provider_id);
    if let Some(path) = files.get(id) {
        return match path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => "providers.d".to_string(),
        };
    }
    if config_ids.contains(id) {
        return "config".to_string();
    }
    "registry".to_string()
}

fn check(roots: &DropInRoots, json: bool) -> Result<()> {
    let snapshot = dropin::scan(roots);
    if json {
        let diagnostics: Vec<serde_json::Value> = snapshot
            .diagnostics
            .iter()
            .map(|diagnostic| {
                serde_json::json!({
                    "path": diagnostic.path,
                    "level": match diagnostic.level {
                        dropin::DiagnosticLevel::Error => "error",
                        dropin::DiagnosticLevel::Warning => "warning",
                    },
                    "message": diagnostic.message,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "files": snapshot.files.len(),
                "providers": snapshot.provider_count(),
                "models": snapshot.model_count(),
                "diagnostics": diagnostics,
            }))?
        );
    } else if snapshot.files.is_empty() {
        println!("No provider files found.");
        for dir in roots.dirs() {
            println!("  (looked in {})", dir.display());
        }
    } else {
        println!(
            "{} file(s): {} provider(s), {} model(s) declared",
            snapshot.files.len(),
            snapshot.provider_count(),
            snapshot.model_count()
        );
        for file in &snapshot.files {
            println!(
                "  {:<52} {}{}",
                file.path.display(),
                file.provider_id,
                if file.defines_provider {
                    String::new()
                } else {
                    format!(" (+{} model(s))", file.model_count)
                }
            );
        }
        if !snapshot.diagnostics.is_empty() {
            println!();
            for diagnostic in &snapshot.diagnostics {
                println!("  {diagnostic}");
            }
        }
    }

    let errors = snapshot
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.level == dropin::DiagnosticLevel::Error)
        .count();
    if errors > 0 {
        bail!("{errors} provider file(s) failed to load");
    }
    Ok(())
}

struct AddArgs {
    id: String,
    label: Option<String>,
    kind: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    models: Vec<String>,
}

fn add(roots: &DropInRoots, project: bool, args: AddArgs) -> Result<()> {
    let id = args.id.trim().to_string();
    if id.is_empty() {
        bail!("provider id must not be empty");
    }
    if id.chars().any(char::is_whitespace) {
        bail!("provider id must not contain whitespace");
    }
    let dir = write_dir(roots, project)?;
    let kind = match args.kind.as_deref() {
        Some(raw) => parse_kind(raw)?,
        None => ProviderKind::OpenAiChatCompletions,
    };
    let provider = ProviderConfig {
        id: id.clone(),
        label: args.label.unwrap_or_else(|| id.clone()),
        description: String::new(),
        kind,
        api_key_env: args
            .api_key_env
            .unwrap_or_else(|| dropin::default_api_key_env(&id)),
        base_url: args.base_url,
        models: args
            .models
            .iter()
            .map(|name| ProviderModelConfig {
                name: name.clone(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    let path = dropin::write_provider_file(&dir, &provider)?;
    println!("Wrote {}", path.display());
    println!(
        "  provider: {id}  kind: {}  models: {}",
        format!("{kind:?}").to_lowercase(),
        provider.models.len()
    );
    if provider.models.is_empty() {
        println!(
            "  next: `navi provider add-model {id} <model>` or `navi registry sync` to \
             fetch the endpoint's model list"
        );
    }
    if provider.api_key_env.is_empty() {
        println!("  set api_key_env before use (e.g. MY_API_KEY)");
    } else {
        println!(
            "  credential: export {}=... (or `navi setup` to store it)",
            provider.api_key_env
        );
    }
    Ok(())
}

fn add_model(
    roots: &DropInRoots,
    project: bool,
    provider_id: &str,
    model: ProviderModelConfig,
) -> Result<()> {
    if provider_id.trim().is_empty() {
        bail!("provider id must not be empty");
    }
    if model.name.trim().is_empty() {
        bail!("model name must not be empty");
    }
    let dir = write_dir(roots, project)?;
    // Extend the provider's own file when it exists so the provider and its
    // models stay together; otherwise create a fragment file.
    let path = match dropin::provider_file(roots, provider_id) {
        Some(path) if path.starts_with(&dir) => {
            dropin::append_models_to_file(&path, std::slice::from_ref(&model))?;
            path
        }
        _ => dropin::write_model_fragment(&dir, provider_id, std::slice::from_ref(&model))?,
    };
    println!("Added model \"{}\" to {provider_id}", model.name);
    println!("  file: {}", path.display());
    Ok(())
}

fn edit(roots: &DropInRoots, loaded_config: &LoadedConfig, project: bool, id: &str) -> Result<()> {
    let dir = write_dir(roots, project)?;
    let path = match dropin::provider_file(roots, id) {
        Some(path) => path,
        None => {
            // Materialize the effective provider (registry or config.toml entry)
            // so editing a built-in override starts from the real definition.
            let provider = navi_core::resolve_provider_config(&loaded_config.config, id)
                .with_context(|| format!("unknown provider `{id}`"))?;
            let path = dropin::write_provider_file(&dir, &provider)?;
            println!("Created {} from the effective {id} definition.", path.display());
            path
        }
    };

    if std::env::var_os("EDITOR").is_none() && std::env::var_os("VISUAL").is_none() {
        println!("{}", path.display());
        bail!("set $EDITOR (or $VISUAL) to edit the file, or open the path above");
    }
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default();
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("failed to launch editor `{editor}`"))?;
    if !status.success() {
        bail!("editor `{editor}` exited with {status}");
    }
    println!("Saved {}", path.display());
    Ok(())
}

fn remove(roots: &DropInRoots, project: bool, id: &str, yes: bool) -> Result<()> {
    let files = dropin::provider_files(roots, id);
    if files.is_empty() {
        bail!(
            "no providers.d file defines `{id}`; if it comes from config.toml, remove that \
             [[providers]] block (or run `navi provider migrate`)"
        );
    }
    if !yes {
        println!("About to delete:");
        for file in &files {
            println!("  {}", file.display());
        }
        bail!("re-run with --yes to confirm");
    }
    let _ = project;
    let removed = dropin::remove_provider_files(roots, id)?;
    for path in removed {
        println!("Removed {}", path.display());
    }
    Ok(())
}

fn migrate(loaded_config: &LoadedConfig, roots: &DropInRoots, dry_run: bool) -> Result<()> {
    let Some(config_path) = loaded_config.global_config_path.clone() else {
        bail!("global config path is unavailable");
    };
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let providers = loaded_config
        .config
        .providers
        .iter()
        .filter(|provider| {
            // Drop-in providers are already in files; only migrate the ones that
            // came from the `[[providers]]` blocks.
            dropin::provider_file(roots, &provider.id).is_none()
        })
        .cloned()
        .collect::<Vec<_>>();

    if providers.is_empty() {
        println!("Nothing to migrate: no `[[providers]]` blocks in {}", config_path.display());
        return Ok(());
    }

    let dir = write_dir(roots, false)?;
    for provider in &providers {
        let target = dir.join(format!("{}.toml", dropin::file_stem_for(&provider.id)));
        println!(
            "{} {} → {}",
            if dry_run { "would move" } else { "move" },
            provider.id,
            target.display()
        );
    }

    let (stripped, removed_blocks) = dropin::strip_providers_section(&raw);
    println!(
        "{} {} `[[providers]]` block(s) from {}",
        if dry_run { "would remove" } else { "remove" },
        removed_blocks,
        config_path.display()
    );
    if dry_run {
        return Ok(());
    }

    for provider in &providers {
        let path = dropin::write_provider_file(&dir, provider)?;
        println!("Wrote {}", path.display());
    }

    let backup = config_path.with_extension("toml.bak-providers-migrate");
    fs::copy(&config_path, &backup)
        .with_context(|| format!("failed to back up {}", config_path.display()))?;
    fs::write(&config_path, stripped)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    println!("Backup: {}", backup.display());
    println!(
        "Done. {} now only holds hand-authored settings; NAVI will not rewrite it.",
        config_path.display()
    );
    Ok(())
}

fn parse_kind(raw: &str) -> Result<ProviderKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "openai-chat-completions" | "chat" | "chat-completions" => {
            Ok(ProviderKind::OpenAiChatCompletions)
        }
        "openai-responses" | "responses" => Ok(ProviderKind::OpenAiResponses),
        "anthropic-messages" | "anthropic" => Ok(ProviderKind::AnthropicMessages),
        "gemini-generate-content" | "gemini" => Ok(ProviderKind::GeminiGenerateContent),
        other => bail!(
            "unknown provider kind `{other}` (expected openai-chat-completions, \
             openai-responses, anthropic-messages or gemini-generate-content)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_accepts_aliases_and_rejects_typos() {
        assert_eq!(
            parse_kind("chat").expect("chat alias"),
            ProviderKind::OpenAiChatCompletions
        );
        assert_eq!(
            parse_kind("anthropic-messages").expect("anthropic"),
            ProviderKind::AnthropicMessages
        );
        assert!(parse_kind("openaichat").is_err());
    }
}
