//! Per-turn memory extraction.
//!
//! After each completed turn, a background task analyzes the conversation
//! and extracts durable memories. This mirrors Claude Code's extractMemories:
//! fire-and-forget, only when the model hasn't already written memories
//! during the turn (mutual exclusion with the `memory` tool).

use anyhow::Result;
use serde::Deserialize;

use crate::memory::{AutoMemoryStore, MemoryType, new_entry, sanitize_id};
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};

/// Prompt sent to the model to extract memories from a turn.
const EXTRACT_PROMPT: &str = r#"You are a memory extraction subagent for NAVI.

Analyze the following conversation turn and extract durable memories that
will be useful in future sessions. Only extract facts that are:
- Durable (not temporary debugging state)
- Non-obvious (not derivable from the code itself)
- Specific (not vague preferences)

Memory types:
- user: preferences, identity, working style
- feedback: behaviors to repeat or avoid
- project: non-derivable project context (deadlines, decisions)
- reference: links to dashboards, external docs

DO NOT extract:
- Secrets, tokens, passwords
- Temporary errors or one-off debugging notes
- Facts that are obvious from reading the code

Conversation:
{conversation}

Return a JSON array of memories. Each memory has:
  - id: short snake_case identifier (e.g. "redis_tests")
  - type: one of "user", "feedback", "project", "reference"
  - name: human-readable title
  - description: one-line summary
  - body: detailed explanation (1-3 sentences)

If nothing worth remembering, return an empty array [].

Output ONLY the JSON array, no markdown fences or explanation."#;

/// A memory extracted by the model.
#[derive(Debug, Clone, Deserialize)]
struct ExtractedMemory {
    id: String,
    #[serde(rename = "type")]
    memory_type: String,
    name: String,
    description: String,
    body: String,
}

/// Runs memory extraction on a completed turn in background.
///
/// `conversation` is the user task + assistant response text.
/// `model_provider` and `model_name` are used for the extraction call.
/// `store` is the auto-memory SQLite store to write to.
///
/// This function is designed to be called via `tokio::spawn` — it should
/// never panic or propagate errors to the caller.
pub async fn extract_memories(
    conversation: &str,
    model_provider: &dyn ModelProvider,
    model_name: &str,
    store: &AutoMemoryStore,
) -> Result<usize> {
    // Sanitize conversation to prevent prompt injection in the template
    let sanitized = conversation.replace("{conversation}", "[conversation]");

    let prompt = EXTRACT_PROMPT.replace("{conversation}", &sanitized);

    let request = ModelRequest {
        model: model_name.to_string(),
        messages: vec![
            ModelMessage::system(
                "You are a memory extraction bot. Return only a JSON array.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = model_provider.complete(request).await?;
    let text = response.text.trim();

    // Parse JSON array
    let memories: Vec<ExtractedMemory> = if text.starts_with('[') {
        serde_json::from_str(text).unwrap_or_default()
    } else {
        // Try to extract JSON from markdown fences
        if let Some(start) = text.find('[') {
            if let Some(end) = text.rfind(']') {
                serde_json::from_str(&text[start..=end]).unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    };

    let mut saved = 0;
    for m in &memories {
        let memory_type = match MemoryType::from_str(&m.memory_type) {
            Some(t) => t,
            None => continue,
        };

        let id = sanitize_id(&m.id);

        // Skip if memory already exists with same id
        if store.get(&id).ok().flatten().is_some() {
            continue;
        }

        let entry = new_entry(&id, memory_type, &m.name, &m.description, &m.body);
        if store.upsert(&entry).is_ok() {
            saved += 1;
        }
    }

    if saved > 0 {
        tracing::info!("extracted {} memories from turn", saved);
    }

    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_prompt_has_placeholder() {
        assert!(EXTRACT_PROMPT.contains("{conversation}"));
    }
}
