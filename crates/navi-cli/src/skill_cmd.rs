use anyhow::{Context, Result};
use navi_core::{
    LoadedConfig, SkillSource, SkillWriteRequest, SkillWriteScope, list_installed_skills,
    parse_skill_file, write_skill,
};
use std::fs;
use std::path::Path;

use crate::SkillAction;

pub fn handle_skill_command(
    action: SkillAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    match action {
        SkillAction::Install { path, id, scope } => {
            install_skill(&path, id.as_deref(), &scope, loaded_config, cwd)
        }
        SkillAction::List => list_skills(loaded_config, cwd),
    }
}

fn install_skill(
    path: &Path,
    id_override: Option<&str>,
    scope_raw: &str,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("failed to read skill file {}", path.display()))?;

    let fallback_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("skill")
        .to_string();

    let parsed = parse_skill_file(&path, &raw, &fallback_name)
        .with_context(|| format!("failed to parse skill file {}", path.display()))?;

    let scope = parse_scope(scope_raw)?;
    let id = id_override
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(parsed.id)
        .unwrap_or_default();

    let request = SkillWriteRequest {
        id,
        name: parsed.name,
        description: parsed.description,
        version: parsed.version,
        author: parsed.author,
        tags: parsed.tags,
        requires: vec![],
        allow_tools: parsed.allow_tools,
        deny_tools: parsed.deny_tools,
        harness: parsed.harness,
        pool: None,
        instructions: parsed.instructions,
        scope,
    };

    let result = write_skill(&request, cwd, &loaded_config.data_dir)
        .with_context(|| format!("failed to install skill from {}", path.display()))?;

    let action = if result.created { "created" } else { "updated" };
    let scope_label = match scope {
        SkillWriteScope::User => "user",
        SkillWriteScope::Project => "project",
    };
    println!(
        "Skill {action}: id={} name=\"{}\" scope={scope_label}",
        result.skill.id, result.skill.name
    );
    println!("  store: {}", result.path.display());
    if let Some(desc) = &result.skill.description {
        println!("  description: {desc}");
    }
    if let Some(version) = &result.skill.version {
        println!("  version: {version}");
    }

    // Deterministic harness pack materialize (best-effort; install still succeeds).
    if let Err(err) =
        crate::harness_cmd::materialize_skill_id_after_install(loaded_config, cwd, &result.skill.id)
    {
        tracing::warn!(error = %err, skill = %result.skill.id, "harness materialize after install failed");
        println!("  harness: materialize skipped ({err})");
    }
    Ok(())
}

fn list_skills(loaded_config: &LoadedConfig, cwd: &Path) -> Result<()> {
    let skills = list_installed_skills(cwd, &loaded_config.data_dir)?;
    if skills.is_empty() {
        println!("No skills found.");
        println!(
            "  store: {}",
            loaded_config.data_dir.join("skills").display()
        );
        return Ok(());
    }

    println!(
        "{:<24} {:<28} {:<10} {:<8}",
        "ID", "NAME", "SOURCE", "SCOPE"
    );
    println!("{}", "-".repeat(72));
    for skill in &skills {
        let source = match skill.source {
            SkillSource::Builtin => "builtin",
            SkillSource::Store => "store",
        };
        let scope = match skill.scope {
            SkillWriteScope::User => "user",
            SkillWriteScope::Project => "project",
        };
        println!(
            "{:<24} {:<28} {:<10} {:<8}",
            truncate(&skill.id, 24),
            truncate(&skill.name, 28),
            source,
            scope
        );
    }
    println!();
    println!(
        "{} skill(s). store: {}",
        skills.len(),
        loaded_config.data_dir.join("skills").display()
    );
    Ok(())
}

fn parse_scope(raw: &str) -> Result<SkillWriteScope> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "user" => Ok(SkillWriteScope::User),
        "project" => Ok(SkillWriteScope::Project),
        other => anyhow::bail!("invalid --scope '{other}' (expected 'user' or 'project')"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
