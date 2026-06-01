use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Identifies the origin or category of a context packet.
///
/// Clients (TUI, Tutor, editors) use these variants so the engine can
/// prioritize and format injected context without knowing the client's UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextSource {
    /// Content from a file on disk.
    File,
    /// Project-level metadata or state.
    Project,
    /// A user's text selection in an editor or UI.
    UserSelection,
    /// A node from a visual canvas (e.g. NAVI Tutor).
    CanvasNode,
    /// A study block from a learning workspace.
    StudyBlock,
    /// A focus thread tracking the user's current area of work.
    FocusThread,
    /// An excerpt from study material or documentation.
    MaterialExcerpt,
    /// A summary from a previous session.
    SessionSummary,
    /// A recorded decision or rationale.
    Decision,
    /// Results from a memory or knowledge-base search.
    MemorySearch,
    /// A custom source identified by an arbitrary string tag.
    Other(String),
}

/// A unit of external context injected into the agent's conversation.
///
/// Context packets let clients supply information from files, canvas nodes,
/// study blocks, memory searches, and other sources without the engine
/// needing to know about the client's data model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    /// Optional client-assigned identifier for deduplication or reference.
    #[serde(default)]
    pub id: Option<String>,
    /// The origin category of this packet.
    pub source: ContextSource,
    /// Optional short title for display or logging.
    #[serde(default)]
    pub title: Option<String>,
    /// The text content to inject into the conversation.
    pub content: String,
    /// Ordering priority; higher values are rendered first in the context block.
    #[serde(default)]
    pub priority: i32,
    /// Arbitrary metadata the client wants to attach (ignored by the engine).
    #[serde(default = "default_context_metadata")]
    pub metadata: Value,
}

fn default_context_metadata() -> Value {
    json!({})
}

/// Renders context packets into a text block for injection into the system
/// prompt, sorted by descending priority.
///
/// Returns `None` if the slice is empty.
pub fn render_context_packets(packets: &[ContextPacket]) -> Option<String> {
    if packets.is_empty() {
        return None;
    }

    let mut ordered = packets.to_vec();
    ordered.sort_by_key(|b| std::cmp::Reverse(b.priority));

    let mut rendered = String::from("=== External Context Packets ===\n");
    for packet in ordered {
        let title = packet.title.as_deref().unwrap_or("untitled");
        rendered.push_str(&format!(
            "- source: {:?}; priority: {}; title: {}\n",
            packet.source, packet.priority, title
        ));
        rendered.push_str(packet.content.trim());
        rendered.push_str("\n\n");
    }

    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_context_packets_by_priority() {
        let low = ContextPacket {
            id: None,
            source: ContextSource::StudyBlock,
            title: Some("low".to_string()),
            content: "later".to_string(),
            priority: 1,
            metadata: json!({}),
        };
        let high = ContextPacket {
            id: None,
            source: ContextSource::FocusThread,
            title: Some("high".to_string()),
            content: "now".to_string(),
            priority: 10,
            metadata: json!({}),
        };

        let rendered = render_context_packets(&[low, high]).expect("rendered");
        assert!(rendered.find("high").unwrap() < rendered.find("low").unwrap());
        assert!(rendered.contains("now"));
        assert!(rendered.contains("later"));
    }
}
