use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextSource {
    File,
    Project,
    UserSelection,
    CanvasNode,
    StudyBlock,
    FocusThread,
    MaterialExcerpt,
    SessionSummary,
    Decision,
    MemorySearch,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    #[serde(default)]
    pub id: Option<String>,
    pub source: ContextSource,
    #[serde(default)]
    pub title: Option<String>,
    pub content: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_context_metadata")]
    pub metadata: Value,
}

fn default_context_metadata() -> Value {
    json!({})
}

pub fn render_context_packets(packets: &[ContextPacket]) -> Option<String> {
    if packets.is_empty() {
        return None;
    }

    let mut ordered = packets.to_vec();
    ordered.sort_by(|a, b| b.priority.cmp(&a.priority));

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
