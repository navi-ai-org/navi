use crate::memory::memory_store::MemoryStore;
use crate::model::ModelMessage;

#[derive(Debug, Clone)]
pub struct RebuildBudgets {
    pub total: usize,
    pub task_list: usize,
    pub checkpoint: usize,
    pub recent_user_messages: usize,
    pub project_memory: usize,
    pub global_memory: usize,
    pub notes: usize,
    pub memory_index: usize,
    pub tail_reminder: usize,
}

impl RebuildBudgets {
    pub fn new(total_budget: usize, context_window: u64) -> Self {
        // Scale down if the context window is smaller than total_budget
        let scale = if context_window < total_budget as u64 {
            context_window as f64 / total_budget as f64
        } else {
            1.0
        };

        let scale_budget =
            |default_val: usize| -> usize { ((default_val as f64 * scale) as usize).max(100) };

        Self {
            total: scale_budget(total_budget),
            task_list: scale_budget(4000),
            checkpoint: scale_budget(16000),
            recent_user_messages: scale_budget(8000),
            project_memory: scale_budget(16000),
            global_memory: scale_budget(4000),
            notes: scale_budget(4000),
            memory_index: scale_budget(4000),
            tail_reminder: scale_budget(1000),
        }
    }
}

pub fn truncate_to_tokens(text: &str, token_budget: usize) -> String {
    let char_budget = token_budget * 4;
    if text.len() <= char_budget {
        text.to_string()
    } else {
        let mut idx = char_budget;
        while idx > 0 && !text.is_char_boundary(idx) {
            idx -= 1;
        }
        format!("{}... [Truncated due to token budget]", &text[..idx])
    }
}

/// Assembles the boot context for a rebuilt context window.
pub fn build_rebuild_context(
    messages: &[ModelMessage],
    memory_store: &MemoryStore,
    context_window: u64,
    total_budget: usize,
) -> String {
    let budgets = RebuildBudgets::new(total_budget, context_window);
    let mut parts = Vec::new();

    // 1. Task list / objective: extract from user task or user messages
    let mut initial_task = "No explicit objective recorded.".to_string();
    for msg in messages {
        if msg.role == crate::model::ModelRole::User {
            initial_task = msg.content.clone();
            break;
        }
    }
    parts.push(format!(
        "=== OBJECTIVE ===\n{}",
        truncate_to_tokens(&initial_task, budgets.task_list)
    ));

    // 2. Session checkpoint
    let checkpoint = memory_store.read_checkpoint().unwrap_or_default();
    parts.push(format!(
        "=== SESSION CHECKPOINT ===\n{}",
        truncate_to_tokens(&checkpoint, budgets.checkpoint)
    ));

    // 3. Verbatim slices of recent user messages
    let mut user_msgs = Vec::new();
    for msg in messages.iter().rev() {
        if msg.role == crate::model::ModelRole::User {
            user_msgs.push(msg.content.clone());
        }
    }
    user_msgs.reverse();
    let recent_user = user_msgs.join("\n---\n");
    parts.push(format!(
        "=== RECENT USER MESSAGES ===\n{}",
        truncate_to_tokens(&recent_user, budgets.recent_user_messages)
    ));

    // 4. Project memory
    let project_mem = memory_store.read_project_memory().unwrap_or_default();
    parts.push(format!(
        "=== PROJECT MEMORY ===\n{}",
        truncate_to_tokens(&project_mem, budgets.project_memory)
    ));

    // 5. Global memory
    let global_mem = memory_store.read_global_memory().unwrap_or_default();
    parts.push(format!(
        "=== GLOBAL MEMORY ===\n{}",
        truncate_to_tokens(&global_mem, budgets.global_memory)
    ));

    // 6. Current notes
    let notes = memory_store.read_notes().unwrap_or_default();
    if !notes.trim().is_empty() {
        parts.push(format!(
            "=== CURRENT NOTES ===\n{}",
            truncate_to_tokens(&notes, budgets.notes)
        ));
    }

    // 7. Memory file index
    let mut index_str = String::new();
    index_str.push_str(&format!(
        "- checkpoint.md: {}\n",
        memory_store.checkpoint_path().display()
    ));
    index_str.push_str(&format!(
        "- notes.md: {}\n",
        memory_store.notes_path().display()
    ));
    index_str.push_str(&format!(
        "- MEMORY.md: {}\n",
        memory_store.project_memory_path().display()
    ));
    index_str.push_str(&format!(
        "- global-memory.md: {}\n",
        memory_store.global_memory_path().display()
    ));
    parts.push(format!(
        "=== MEMORY INDEX ===\n{}",
        truncate_to_tokens(&index_str, budgets.memory_index)
    ));

    // 8. Tail reminder / instructions
    let tail_reminder = r#"=== SYSTEM INSTRUCTIONS FOR CONTINUATION ===
You are continuing an existing logical coding session after a context rebuild.
Do not ask the user to restate the goal.
Trust the structured checkpoint unless contradicted by verbatim user messages or current repository state.
Use the history tool only when structured memory is insufficient.
Your immediate next action is listed in the checkpoint."#;
    parts.push(truncate_to_tokens(tail_reminder, budgets.tail_reminder));

    parts.join("\n\n")
}
