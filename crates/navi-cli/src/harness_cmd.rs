//! CLI: `navi harness list|show|materialize`

use anyhow::{Context, Result};
use navi_core::tool::ToolExposure;
use navi_core::{
    LoadedConfig, MaterializeOptions, build_capability_inventory, list_harness_ids, load_pack,
    load_skill_by_id, materialize_from_skill,
};
use std::path::Path;

use crate::HarnessAction;

/// Default direct/deferred tools used when building inventory without a live executor.
fn default_tool_meta() -> Vec<(String, ToolExposure)> {
    let direct = [
        "search",
        "read_file",
        "edit",
        "write_file",
        "bash",
        "plan",
        "question",
        "tool_search",
        "memory",
        "set_session_title",
        "get_goal",
        "create_goal",
        "update_goal",
    ];
    let deferred = [
        "browser",
        "code",
        "code_edit",
        "code_exec",
        "repo_explore",
        "subagent",
        "workflow",
        "package_manager",
        "ast_search",
        "symbol_goto",
        "view_image",
        "apply_patch",
        "sandbox",
    ];
    let mut out = Vec::new();
    for n in direct {
        out.push((n.into(), ToolExposure::Direct));
    }
    for n in deferred {
        out.push((n.into(), ToolExposure::Deferred));
    }
    out
}

pub fn handle_harness_command(
    action: HarnessAction,
    loaded_config: &LoadedConfig,
    cwd: &Path,
) -> Result<()> {
    match action {
        HarnessAction::List => list_harnesses(loaded_config),
        HarnessAction::Show { id } => show_harness(loaded_config, &id),
        HarnessAction::Materialize { skill, id } => {
            materialize(loaded_config, cwd, &skill, id.as_deref())
        }
    }
}

fn list_harnesses(loaded_config: &LoadedConfig) -> Result<()> {
    let ids = list_harness_ids(&loaded_config.data_dir)?;
    if ids.is_empty() {
        println!("No harness packs found.");
        println!(
            "  root: {}",
            loaded_config.data_dir.join("harnesses").display()
        );
        println!("  hint: navi harness materialize <skill-id>");
        return Ok(());
    }
    println!(
        "{:<24} {:<12} {:<14} {}",
        "ID", "MAX_TURNS", "TOKEN_BUDGET", "PATH"
    );
    println!("{}", "-".repeat(72));
    for id in &ids {
        match load_pack(&loaded_config.data_dir, id)? {
            Some(pack) => {
                let budget = pack
                    .loop_spec
                    .token_budget
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "-".into());
                println!(
                    "{:<24} {:<12} {:<14} {}",
                    pack.id,
                    pack.loop_spec.max_turns,
                    budget,
                    pack.root.display()
                );
            }
            None => println!("{id:<24} (incomplete pack)"),
        }
    }
    println!();
    println!(
        "{} pack(s). root: {}",
        ids.len(),
        loaded_config.data_dir.join("harnesses").display()
    );
    Ok(())
}

fn show_harness(loaded_config: &LoadedConfig, id: &str) -> Result<()> {
    let pack = load_pack(&loaded_config.data_dir, id)?
        .with_context(|| format!("harness pack `{id}` not found"))?;
    println!("id: {}", pack.id);
    println!("path: {}", pack.root.display());
    println!("loop.max_turns: {}", pack.loop_spec.max_turns);
    println!(
        "loop.token_budget: {}",
        pack.loop_spec
            .token_budget
            .map(|b| b.to_string())
            .unwrap_or_else(|| "none".into())
    );
    println!("loop.stop: {:?}", pack.loop_spec.stop);
    if let Some(graph) = &pack.graph {
        println!("graph.entry: {}", graph.entry);
        println!(
            "graph.nodes: {:?}",
            graph.nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
        );
        for node in &graph.nodes {
            if !node.allow_tools.is_empty() {
                println!("  node.{} allow_tools: {:?}", node.id, node.allow_tools);
            }
        }
    } else {
        println!("graph: (none)");
    }
    if !pack.verifiers.is_empty() {
        println!("verifiers:");
        for v in &pack.verifiers {
            println!("  - {} ({:?}): {}", v.id, v.kind, v.recipe);
        }
    }
    Ok(())
}

fn materialize(
    loaded_config: &LoadedConfig,
    cwd: &Path,
    skill_id: &str,
    id_override: Option<&str>,
) -> Result<()> {
    // Force discovery so install→materialize works even when skills.enabled=false.
    let mut skills_cfg = loaded_config.config.skills.clone();
    skills_cfg.enabled = true;
    let skill = load_skill_by_id(&skills_cfg, cwd, &loaded_config.data_dir, skill_id)
        .with_context(|| format!("skill `{skill_id}` not found"))?;

    let mut config = loaded_config.config.clone();
    config.skills.enabled = true;
    let inventory = build_capability_inventory(
        &loaded_config.data_dir,
        &config,
        &default_tool_meta(),
        true,
        &[],
        &[],
    );

    let mut opts = MaterializeOptions::default();
    if let Some(id) = id_override {
        opts.id_override = Some(id.to_string());
    }

    let pack = materialize_from_skill(&loaded_config.data_dir, &skill, &inventory, opts)?;

    println!(
        "Harness pack materialized: id={} path={}",
        pack.id,
        pack.root.display()
    );
    println!(
        "  loop.max_turns={} token_budget={:?}",
        pack.loop_spec.max_turns, pack.loop_spec.token_budget
    );
    if let Some(g) = &pack.graph {
        println!("  graph.entry={} nodes={}", g.entry, g.nodes.len());
    }
    Ok(())
}

/// Materialize harness after skill install (best-effort).
pub fn materialize_skill_id_after_install(
    loaded_config: &LoadedConfig,
    cwd: &Path,
    skill_id: &str,
) -> Result<()> {
    materialize(loaded_config, cwd, skill_id, None)
}
