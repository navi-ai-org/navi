use crate::memory::AutoMemoryStore;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::Result;

pub const CHECKPOINT_WRITER_SYSTEM: &str = r#"You are a checkpoint-writer subagent for NAVI.
Extract operational session state into a structured checkpoint and promote only stable,
verified, architectural project facts. Do NOT promote temporary debug details, one-off
task facts, or secrets.

Output ONLY these XML blocks (no extra prose):

<checkpoint_markdown>
# Session Checkpoint

## Current Intent
## Next Action
## Working Constraints
## Task Tree
## Current Work
## Involved Files
## Cross-Task Discoveries
## Errors and Fixes
## Runtime State
## Design Decisions
## Miscellaneous Notes
</checkpoint_markdown>

<promote_facts>
[Markdown list of new stable facts, or empty]
</promote_facts>

Be precise: filenames, commands, test status, next action, intent, constraints, decisions, errors.
Omit empty sections only if truly N/A; keep the heading structure."#;

pub const CHECKPOINT_WRITER_PROMPT: &str = r#"INPUTS:

1. Current notes:
{notes_content}

2. Existing project memory index:
{project_memory}

3. Previous Checkpoint:
{previous_checkpoint}

4. Current Conversation History:
{conversation_history}
"#;

pub(crate) fn sanitize_input(text: &str) -> String {
    text.replace("<checkpoint_markdown>", "[checkpoint_markdown]")
        .replace("</checkpoint_markdown>", "[/checkpoint_markdown]")
        .replace("<promote_facts>", "[promote_facts]")
        .replace("</promote_facts>", "[/promote_facts]")
}

/// Runs the checkpoint writer subagent.
///
/// All reads and writes go through the SQLite-backed `AutoMemoryStore`:
/// - Checkpoint is stored in the `session_checkpoint` table.
/// - Notes are read from and archived in the `session_notes` table.
/// - Promoted facts are upserted as `project`-type memories.
pub async fn run_checkpoint_writer(
    _session_id: &str,
    messages: &[ModelMessage],
    auto_memory: &AutoMemoryStore,
    model_provider: &dyn ModelProvider,
    model_name: &str,
) -> Result<()> {
    let prev_checkpoint_raw = sanitize_input(&auto_memory.read_checkpoint().unwrap_or_default());
    let notes_content = sanitize_input(&auto_memory.read_notes().unwrap_or_default());
    let project_memory = sanitize_input(&auto_memory.render_index());

    let conversation_history = sanitize_input(&format_conversation_history(messages));

    let prompt = CHECKPOINT_WRITER_PROMPT
        .replace("{notes_content}", &notes_content)
        .replace("{project_memory}", &project_memory)
        .replace("{previous_checkpoint}", &prev_checkpoint_raw)
        .replace("{conversation_history}", &conversation_history);

    let request = ModelRequest {
        model: model_name.to_string(),
        instructions: None,
        messages: vec![
            ModelMessage::system(CHECKPOINT_WRITER_SYSTEM),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = model_provider.complete(request).await?;
    let response_text = response.text;

    // Parse output blocks
    let checkpoint_text = extract_block(
        &response_text,
        "<checkpoint_markdown>",
        "</checkpoint_markdown>",
    )
    .unwrap_or_else(|| response_text.clone());
    let promote_facts = extract_block(&response_text, "<promote_facts>", "</promote_facts>");

    // Save checkpoint to SQLite
    if !checkpoint_text.trim().is_empty() {
        auto_memory.write_checkpoint(&checkpoint_text)?;
    }

    // Save promoted facts to SQLite memories table
    if let Some(facts) = promote_facts {
        auto_memory.promote_facts(&facts)?;
    }

    // Archive notes if notes were processed
    if !notes_content.trim().is_empty() {
        auto_memory.archive_notes()?;
    }

    Ok(())
}

fn format_conversation_history(messages: &[ModelMessage]) -> String {
    let mut out = String::new();
    for msg in messages {
        let role = match msg.role {
            crate::model::ModelRole::User => "User",
            crate::model::ModelRole::Assistant => "Assistant",
            crate::model::ModelRole::Tool => "Tool",
            crate::model::ModelRole::System => "System",
            crate::model::ModelRole::Developer => "Developer",
        };
        out.push_str(&format!("[{}]: {}\n", role, msg.content));
        if let Some(ref tool) = msg.tool_name {
            out.push_str(&format!("  Tool Name: {}\n", tool));
        }
    }
    out
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
