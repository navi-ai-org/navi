use crate::memory::history_store::HistoryStore;
use crate::memory::memory_store::MemoryStore;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::Result;

pub const DREAM_PROMPT: &str = r#"You are a memory maintenance subagent named NAVI Dream.
Your task is to review the existing MEMORY.md and global-memory.md, clean up duplicates, resolve contradictions, and output a consolidated, compact version of these files.

Existing MEMORY.md:
{project_memory}

Existing global-memory.md:
{global_memory}

Recent Checkpoints:
{recent_checkpoints}

INSTRUCTIONS:
1. Merge duplicate rules or observations.
2. Remove outdated or temporary entries that are no longer true.
3. Keep verified conventions, architecture notes, commands, and rules.
4. Output the updated files inside distinct XML blocks: `<updated_project_memory>...</updated_project_memory>` and `<updated_global_memory>...</updated_global_memory>`.
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
        .replace("<sop_artifact", "[sop_artifact")
        .replace("</sop_artifact>", "[/sop_artifact]")
}

pub async fn run_dream_maintenance(
    memory_store: &MemoryStore,
    history_store: &HistoryStore,
    model_provider: &dyn ModelProvider,
    model_name: &str,
) -> Result<()> {
    let project_memory = sanitize_input(&memory_store.read_project_memory().unwrap_or_default());
    let global_memory = sanitize_input(&memory_store.read_global_memory().unwrap_or_default());

    // Fetch checkpoints from DB
    let conn_guard = history_store.search_history("Checkpoint", None, Some(5))?;
    let mut recent_checkpoints = String::new();
    for event in conn_guard {
        if let Some(ref content) = event.content {
            recent_checkpoints.push_str(content);
            recent_checkpoints.push_str("\n---\n");
        }
    }
    let recent_checkpoints = sanitize_input(&recent_checkpoints);

    let prompt = DREAM_PROMPT
        .replace("{project_memory}", &project_memory)
        .replace("{global_memory}", &global_memory)
        .replace("{recent_checkpoints}", &recent_checkpoints);

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

    if let Some(updated_pm) = extract_block(
        &text,
        "<updated_project_memory>",
        "</updated_project_memory>",
    ) {
        if !updated_pm.trim().is_empty() {
            memory_store.write_project_memory(&updated_pm)?;
        }
    }

    if let Some(updated_gm) =
        extract_block(&text, "<updated_global_memory>", "</updated_global_memory>")
    {
        if !updated_gm.trim().is_empty() {
            memory_store.write_global_memory(&updated_gm)?;
        }
    }

    Ok(())
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
