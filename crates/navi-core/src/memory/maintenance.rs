use crate::memory::history_store::HistoryStore;
use crate::memory::memory_store::MemoryStore;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DREAM_PROMPT: &str = r#"You are a memory maintenance subagent named NAVI Dream.
Your task is to reflect on NAVI's persistent memory and recent session transcripts, then produce a separate consolidated memory store.

This is an offline synthesis pass:
- Do not merely append a transcript summary.
- Merge duplicates.
- Resolve contradictions by preferring the newest verified session evidence.
- Drop stale, temporary, speculative, or one-off debugging notes.
- Preserve stable project architecture, commands, conventions, user preferences, and reusable lessons.
- Surface new durable insights that should help future sessions.

Existing MEMORY.md:
{project_memory}

Existing global-memory.md:
{global_memory}

Current checkpoint.md:
{checkpoint}

Current notes.md:
{notes}

Recent sessions:
{recent_sessions}

Additional dream instructions:
{instructions}

INSTRUCTIONS:
Output the dream result inside distinct XML blocks:
<updated_project_memory>...</updated_project_memory>
<updated_global_memory>...</updated_global_memory>
<dream_report>Briefly list what changed, what was removed, and notable unresolved contradictions.</dream_report>
"#;

pub const DISTILL_PROMPT: &str = r#"You are a process distillation subagent named NAVI Distill.
Your task is to analyze the recent conversation histories and extract reusable processes (SOPs, skills, checklists).

Recent Session History:
{recent_history}

INSTRUCTIONS:
Identify any repeated successful patterns, workflows, checklists, or setups.
Generate a reusable process artifact (Standard Operating Procedure - SOP) in Markdown.
Output your generated SOP inside a `<sop_artifact filename="name.md">...</sop_artifact>` block.
"#;

fn sanitize_input(text: &str) -> String {
    text.replace("<updated_project_memory>", "[updated_project_memory]")
        .replace("</updated_project_memory>", "[/updated_project_memory]")
        .replace("<updated_global_memory>", "[updated_global_memory]")
        .replace("</updated_global_memory>", "[/updated_global_memory]")
        .replace("<dream_report>", "[dream_report]")
        .replace("</dream_report>", "[/dream_report]")
        .replace("<sop_artifact", "[sop_artifact")
        .replace("</sop_artifact>", "[/sop_artifact]")
}

#[derive(Debug, Clone)]
pub struct DreamOptions {
    /// Number of recent sessions to mine. Claude dreams accept up to 100 sessions.
    pub session_limit: usize,
    /// High-level synthesis guidance.
    pub instructions: Option<String>,
    /// Replace active memory files with the dream output after writing it.
    pub apply: bool,
}

impl Default for DreamOptions {
    fn default() -> Self {
        Self {
            session_limit: 10,
            instructions: None,
            apply: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DreamResult {
    pub output_dir: PathBuf,
    pub project_memory_path: PathBuf,
    pub global_memory_path: PathBuf,
    pub report_path: PathBuf,
    pub applied: bool,
    pub auto_memory_report: Option<crate::memory::ConsolidationReport>,
}

pub async fn run_dream_maintenance(
    memory_store: &MemoryStore,
    history_store: &HistoryStore,
    model_provider: &dyn ModelProvider,
    model_name: &str,
) -> Result<DreamResult> {
    run_dream_maintenance_with_options(
        memory_store,
        history_store,
        model_provider,
        model_name,
        DreamOptions::default(),
    )
    .await
}

pub async fn run_dream_maintenance_with_options(
    memory_store: &MemoryStore,
    history_store: &HistoryStore,
    model_provider: &dyn ModelProvider,
    model_name: &str,
    options: DreamOptions,
) -> Result<DreamResult> {
    let project_memory = sanitize_input(&memory_store.read_project_memory().unwrap_or_default());
    let global_memory = sanitize_input(&memory_store.read_global_memory().unwrap_or_default());
    let checkpoint = sanitize_input(&memory_store.read_checkpoint().unwrap_or_default());
    let notes = sanitize_input(&memory_store.read_notes().unwrap_or_default());
    let recent_sessions = sanitize_input(&format_recent_sessions(
        history_store,
        options.session_limit.clamp(1, 100),
    )?);
    let instructions = sanitize_input(options.instructions.as_deref().unwrap_or(
        "Focus on stable coding workflow, project architecture, commands, and user preferences.",
    ));

    let prompt = DREAM_PROMPT
        .replace("{project_memory}", &project_memory)
        .replace("{global_memory}", &global_memory)
        .replace("{checkpoint}", &checkpoint)
        .replace("{notes}", &notes)
        .replace("{recent_sessions}", &recent_sessions)
        .replace("{instructions}", &instructions);

    let request = ModelRequest {
        model: model_name.to_string(),
        messages: vec![
            ModelMessage::system(
                "You are a helpful memory maintenance bot. Return only the requested XML tags.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = model_provider.complete(request).await?;
    let text = response.text;

    let updated_pm = extract_block(
        &text,
        "<updated_project_memory>",
        "</updated_project_memory>",
    )
    .context("dream response did not include <updated_project_memory>")?;
    let updated_gm = extract_block(&text, "<updated_global_memory>", "</updated_global_memory>")
        .context("dream response did not include <updated_global_memory>")?;
    let dream_report = extract_block(&text, "<dream_report>", "</dream_report>")
        .unwrap_or_else(|| "Dream completed without a report.".to_string());

    if updated_pm.trim().is_empty() {
        anyhow::bail!("dream response produced empty project memory");
    }
    if updated_gm.trim().is_empty() {
        anyhow::bail!("dream response produced empty global memory");
    }

    let output_dir = dream_output_dir(memory_store)?;
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    let project_memory_path = output_dir.join("MEMORY.md");
    let global_memory_path = output_dir.join("global-memory.md");
    let report_path = output_dir.join("dream-report.md");
    crate::memory::memory_store::write_atomic(&project_memory_path, updated_pm.trim())?;
    crate::memory::memory_store::write_atomic(&global_memory_path, updated_gm.trim())?;
    crate::memory::memory_store::write_atomic(&report_path, dream_report.trim())?;

    if options.apply {
        memory_store.write_project_memory(updated_pm.trim())?;
        memory_store.write_global_memory(updated_gm.trim())?;
    }

    // Consolidate auto-memory SQLite store (mark stale, deduplicate, backfill embeddings)
    let auto_memory_report = {
        let db_path = memory_store.memory_root.join("memories.db");
        match crate::memory::AutoMemoryStore::open(&db_path) {
            Ok(store) => {
                // Consolidation pass (stale + dedup)
                match store.consolidate(30) {
                    Ok(report) => {
                        tracing::info!(
                            "auto-memory consolidation: {} stale, {} duplicates, {} active",
                            report.marked_stale,
                            report.duplicates_merged,
                            report.remaining_active
                        );

                        // Backfill embeddings for memories without them
                        if crate::memory::embeddings_available() {
                            let models_dir = memory_store.memory_root.join("models");
                            let model_path = models_dir.join(crate::memory::DEFAULT_MODEL_FILE);
                            let tokenizer_path = models_dir.join(crate::memory::DEFAULT_TOKENIZER_FILE);

                            if model_path.exists() && tokenizer_path.exists() {
                                let embed_config = crate::memory::EmbeddingConfig {
                                    model_path,
                                    tokenizer_path,
                                    ..Default::default()
                                };
                                let embedder = crate::memory::create_embedder(embed_config);

                                let missing = store.list_without_embeddings()
                                    .unwrap_or_default();

                                if !missing.is_empty() {
                                    tracing::info!(
                                        "backfilling embeddings for {} memories",
                                        missing.len()
                                    );
                                    for m in &missing {
                                        if let Some(text) = store.get_memory_text(&m.id).unwrap_or(None) {
                                            match embedder.embed(&text) {
                                                Ok(emb) => {
                                                    let _ = store.set_embedding(&m.id, &emb);
                                                }
                                                Err(e) => {
                                                    tracing::debug!(
                                                        "embedding backfill failed for {}: {}",
                                                        m.id, e
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        Some(report)
                    }
                    Err(e) => {
                        tracing::warn!("auto-memory consolidation failed: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::debug!("auto-memory store not available for consolidation: {}", e);
                None
            }
        }
    };

    Ok(DreamResult {
        output_dir,
        project_memory_path,
        global_memory_path,
        report_path,
        applied: options.apply,
        auto_memory_report,
    })
}

pub async fn run_distill_maintenance(
    memory_store: &MemoryStore,
    history_store: &HistoryStore,
    model_provider: &dyn ModelProvider,
    model_name: &str,
) -> Result<()> {
    // Retrieve recent events for context
    let sessions = history_store.list_sessions()?;
    let mut history_text = String::new();
    for session in sessions.iter().take(3) {
        let events = history_store.get_recent_events(&session.id, Some(50))?;
        for e in events {
            if let Some(ref content) = e.content {
                history_text.push_str(&format!("[Session {}]: {}\n", session.id, content));
            }
        }
    }
    let history_text = sanitize_input(&history_text);

    let prompt = DISTILL_PROMPT.replace("{recent_history}", &history_text);

    let request = ModelRequest {
        model: model_name.to_string(),
        messages: vec![
            ModelMessage::system(
                "You are a process distillation bot. Return only the requested XML tags.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = model_provider.complete(request).await?;
    let text = response.text;

    if let Some(sop_block) = extract_block_with_attr(&text, "<sop_artifact", "</sop_artifact>") {
        let (filename, content) = sop_block;
        if !content.trim().is_empty() {
            let sops_dir = memory_store.memory_root.join("sops");
            if !sops_dir.exists() {
                std::fs::create_dir_all(&sops_dir)?;
            }
            let sop_path = sops_dir.join(filename);
            crate::memory::memory_store::write_atomic(&sop_path, &content)?;
        }
    }

    Ok(())
}

fn extract_block(text: &str, start_tag: &str, end_tag: &str) -> Option<String> {
    let start_idx = text.find(start_tag)?;
    let end_idx = text.find(end_tag)?;
    if start_idx < end_idx {
        Some(text[start_idx + start_tag.len()..end_idx].to_string())
    } else {
        None
    }
}

fn dream_output_dir(memory_store: &MemoryStore) -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let dreams_dir = memory_store.memory_root.join("dreams");
    let mut candidate = dreams_dir.join(format!("dream-{timestamp}"));
    let mut suffix = 1;
    while candidate.exists() {
        candidate = dreams_dir.join(format!("dream-{timestamp}-{suffix}"));
        suffix += 1;
    }
    Ok(candidate)
}

fn format_recent_sessions(history_store: &HistoryStore, session_limit: usize) -> Result<String> {
    let sessions = history_store.list_sessions()?;
    if sessions.is_empty() {
        return Ok("No recorded sessions yet.".to_string());
    }

    let mut rendered = String::new();
    for session in sessions.iter().take(session_limit) {
        rendered.push_str(&format!(
            "## Session {}\nStarted: {}\nProject: {}\n",
            session.id, session.started_at, session.project_id
        ));
        let events = history_store.get_recent_events(&session.id, Some(80))?;
        for event in events {
            if event.event_type != "message" {
                continue;
            }
            let role = event.role.as_deref().unwrap_or("unknown");
            if let Some(content) = event.content {
                rendered.push_str(&format!(
                    "[{}] {}\n",
                    role,
                    truncate_chars(content.trim(), 2_000)
                ));
            }
            if let Some(tool_output) = event.tool_output {
                rendered.push_str(&format!(
                    "[tool-output] {}\n",
                    truncate_chars(tool_output.trim(), 1_000)
                ));
            }
        }
        rendered.push_str("\n---\n");
    }

    Ok(truncate_chars(&rendered, 80_000))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut truncated: String = text.chars().take(max_chars).collect();
    truncated.push_str("\n[truncated]");
    truncated
}

fn extract_block_with_attr(
    text: &str,
    start_tag_prefix: &str,
    end_tag: &str,
) -> Option<(String, String)> {
    let start_idx = text.find(start_tag_prefix)?;
    let end_tag_start = text[start_idx..].find('>')?;
    let start_tag_full_len = end_tag_start + 1;
    let start_tag_content = &text[start_idx..start_idx + start_tag_full_len];

    // Extract filename attribute: filename="some_name.md"
    let filename = if let Some(fn_start) = start_tag_content.find("filename=\"") {
        let fn_sub = &start_tag_content[fn_start + "filename=\"".len()..];
        if let Some(fn_end) = fn_sub.find('"') {
            fn_sub[..fn_end].to_string()
        } else {
            "distilled_sop.md".to_string()
        }
    } else {
        "distilled_sop.md".to_string()
    };

    let content_start = start_idx + start_tag_full_len;
    let end_idx = text[content_start..].find(end_tag)?;
    let content = text[content_start..content_start + end_idx].to_string();

    Some((filename, content))
}
