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
            ModelMessage::system("You are a memory extraction bot. Return only a JSON array."),
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

    #[test]
    fn test_extracted_memory_deserialization() {
        let json = r#"[{"id":"test_mem","type":"feedback","name":"Test","description":"A test memory","body":"This is a test"}]"#;
        let memories: Vec<ExtractedMemory> = serde_json::from_str(json).unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].id, "test_mem");
        assert_eq!(memories[0].memory_type, "feedback");
    }

    #[test]
    fn test_extracted_memory_with_invalid_type() {
        let json =
            r#"[{"id":"bad","type":"invalid_type","name":"Bad","description":"Bad","body":"Bad"}]"#;
        let memories: Vec<ExtractedMemory> = serde_json::from_str(json).unwrap();
        assert_eq!(memories.len(), 1);
        // from_str on memory_type will return None, so it should be skipped
        assert!(MemoryType::from_str(&memories[0].memory_type).is_none());
    }

    #[test]
    fn test_extracted_memory_empty_array() {
        let json = "[]";
        let memories: Vec<ExtractedMemory> = serde_json::from_str(json).unwrap();
        assert!(memories.is_empty());
    }

    /// Mock model provider for testing extract_memories without a real API.
    struct MockProvider {
        response: String,
    }

    impl crate::model::ModelProvider for MockProvider {
        fn stream(&self, _request: ModelRequest) -> crate::model::ModelStream {
            let text = self.response.clone();
            Box::pin(futures_util::stream::iter(vec![
                Ok(crate::model::ModelStreamEvent::TextDelta { text }),
                Ok(crate::model::ModelStreamEvent::Done),
            ]))
        }
    }

    #[tokio::test]
    async fn test_extract_memories_with_mock_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("memories.db");
        let store = AutoMemoryStore::open(&db_path).unwrap();

        // Mock provider returns one memory
        let provider = MockProvider {
            response: r#"[{"id":"redis_tests","type":"feedback","name":"Redis for Tests","description":"Need Redis running","body":"Start Redis before running tests"}]"#.to_string(),
        };

        let result = extract_memories(
            "User: fix the tests\n\nAssistant: I found that Redis needs to be running",
            &provider,
            "test-model",
            &store,
        )
        .await
        .unwrap();

        assert_eq!(result, 1);

        // Verify the memory was saved
        let entry = store.get("redis_tests").unwrap().unwrap();
        assert_eq!(entry.name, "Redis for Tests");
        assert_eq!(entry.memory_type, MemoryType::Feedback);
    }

    #[tokio::test]
    async fn test_extract_memories_skips_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("memories.db");
        let store = AutoMemoryStore::open(&db_path).unwrap();

        // Pre-populate with an existing memory
        let entry = new_entry(
            "redis_tests",
            MemoryType::Feedback,
            "Existing",
            "Old",
            "Old body",
        );
        store.upsert(&entry).unwrap();

        // Mock provider returns the same id
        let provider = MockProvider {
            response: r#"[{"id":"redis_tests","type":"feedback","name":"New","description":"New","body":"New body"}]"#.to_string(),
        };

        let result = extract_memories("test", &provider, "model", &store)
            .await
            .unwrap();

        // Should skip because it already exists
        assert_eq!(result, 0);

        // Verify the existing memory was NOT overwritten
        let existing = store.get("redis_tests").unwrap().unwrap();
        assert_eq!(existing.name, "Existing");
    }

    #[tokio::test]
    async fn test_extract_memories_empty_response() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("memories.db");
        let store = AutoMemoryStore::open(&db_path).unwrap();

        let provider = MockProvider {
            response: "[]".to_string(),
        };

        let result = extract_memories("nothing useful", &provider, "model", &store)
            .await
            .unwrap();

        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn test_extract_memories_garbage_response() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("memories.db");
        let store = AutoMemoryStore::open(&db_path).unwrap();

        let provider = MockProvider {
            response: "This is not JSON at all".to_string(),
        };

        let result = extract_memories("test", &provider, "model", &store)
            .await
            .unwrap();

        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn test_extract_memories_json_in_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("memories.db");
        let store = AutoMemoryStore::open(&db_path).unwrap();

        let provider = MockProvider {
            response: r#"Here are the memories:
```json
[{"id":"pref_dark","type":"user","name":"Dark Mode","description":"User prefers dark mode","body":"The user prefers dark mode themes"}]
```
"#.to_string(),
        };

        let result = extract_memories("User: I like dark mode", &provider, "model", &store)
            .await
            .unwrap();

        assert_eq!(result, 1);
        let entry = store.get("pref_dark").unwrap().unwrap();
        assert_eq!(entry.name, "Dark Mode");
        assert_eq!(entry.memory_type, MemoryType::User);
    }
}
