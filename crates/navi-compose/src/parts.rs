//! Concrete implementations of the swappable engine parts.
//!
//! These are the values the host installs into the typed slots when a component
//! activates. They are plain Rust implementations of `navi_core` traits — the
//! point is that *which one the engine uses* is decided by the composition
//! (kernel generations), not by the build.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use navi_core::{
    CompactOutcome, CompactState, CompactionStrategy, ModelMessage, ModelProvider, ModelRole,
    PromptBuilder, PromptCache, RenderedPrompt, SystemPromptInput, config::HarnessConfig,
};

/// The values a component provides to the engine when it activates.
#[derive(Default, Clone)]
pub struct ComponentValues {
    /// `navi.compaction@1`
    pub compaction: Option<Arc<dyn CompactionStrategy>>,
    /// `navi.prompt@1`
    pub prompt: Option<Arc<dyn PromptBuilder>>,
}

impl ComponentValues {
    /// True when the component provides nothing typed (a pure JSON capability).
    pub fn is_empty(&self) -> bool {
        self.compaction.is_none() && self.prompt.is_none()
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Aggressive micro-compaction: clears **every** tool result older than the gap.
///
/// The default strategy only clears read-only tool results; this one trades
/// detail for context, which is exactly the kind of policy a user may want to
/// swap in mid-session.
#[derive(Debug, Default)]
pub struct TerseCompaction;

#[async_trait::async_trait]
impl CompactionStrategy for TerseCompaction {
    fn micro_compact(&self, messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
        let gap_ms = gap_threshold_minutes.saturating_mul(60_000);
        let last_assistant = messages
            .iter()
            .rev()
            .find(|message| message.role == ModelRole::Assistant)
            .and_then(|message| message.created_at);
        let Some(last_ts) = last_assistant else {
            return 0;
        };
        if now_millis().saturating_sub(last_ts) < gap_ms {
            return 0;
        }
        let mut cleared = 0;
        for message in messages.iter_mut() {
            if message.role == ModelRole::Tool && !message.content.contains("[terse compaction]") {
                message.content = "[terse compaction] old tool result cleared".to_string();
                message.content_parts.clear();
                cleared += 1;
            }
        }
        cleared
    }

    async fn auto_compact(
        &self,
        state: &mut CompactState,
        messages: &mut Vec<ModelMessage>,
        provider: &dyn ModelProvider,
        model: &str,
        config: &HarnessConfig,
    ) -> anyhow::Result<Option<CompactOutcome>> {
        // Reuses NAVI's summarisation path; only micro-compaction is specialised.
        state.auto_compact(messages, provider, model, config).await
    }
}

/// Wraps another prompt builder and stamps a visible marker into the base
/// instructions, so a swap is observable in the system prompt.
pub struct BrandedPrompt {
    inner: Arc<dyn PromptBuilder>,
    marker: String,
}

impl BrandedPrompt {
    /// Builds a branded prompt over `inner`.
    pub fn new(inner: Arc<dyn PromptBuilder>, marker: impl Into<String>) -> Self {
        Self {
            inner,
            marker: marker.into(),
        }
    }
}

impl PromptBuilder for BrandedPrompt {
    fn build(&self, input: SystemPromptInput, cache: Arc<PromptCache>) -> RenderedPrompt {
        let mut rendered = self.inner.build(input, cache);
        if !rendered.instructions.contains(&self.marker) {
            rendered.instructions.push('\n');
            rendered.instructions.push_str(&self.marker);
            rendered.instructions.push('\n');
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use navi_core::DefaultCompactionStrategy;

    fn old_tool_result(tool: &str) -> ModelMessage {
        ModelMessage {
            role: ModelRole::Tool,
            content: "file contents".to_string(),
            content_parts: Vec::new(),
            tool_call_id: Some(format!("call-{tool}")),
            tool_name: Some(tool.to_string()),
            tool_calls: Vec::new(),
            created_at: Some(now_millis().saturating_sub(3_600_000)),
            thinking_content: None,
        }
    }

    fn old_assistant() -> ModelMessage {
        ModelMessage {
            role: ModelRole::Assistant,
            content: "done".to_string(),
            content_parts: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: Vec::new(),
            created_at: Some(now_millis().saturating_sub(3_600_000)),
            thinking_content: None,
        }
    }

    #[test]
    fn terse_compaction_clears_more_than_the_default() {
        let build = || {
            vec![
                old_tool_result("read_file"),
                old_tool_result("edit"),
                old_assistant(),
            ]
        };

        let mut default_messages = build();
        let default_cleared =
            DefaultCompactionStrategy.micro_compact(&mut default_messages, 60);
        let mut terse_messages = build();
        let terse_cleared = TerseCompaction.micro_compact(&mut terse_messages, 60);

        assert_eq!(default_cleared, 1, "the default only clears read-only results");
        assert_eq!(terse_cleared, 2, "the terse strategy clears every tool result");
        assert!(terse_messages[1].content.contains("[terse compaction]"));
        assert_eq!(
            default_messages[1].content, "file contents",
            "the default leaves the write-tool result alone"
        );
    }

    #[test]
    fn terse_compaction_respects_the_gap_threshold() {
        let mut messages = vec![
            ModelMessage {
                role: ModelRole::Tool,
                content: "fresh".to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: Some("read_file".to_string()),
                tool_calls: Vec::new(),
                created_at: Some(now_millis()),
                thinking_content: None,
            },
            ModelMessage {
                role: ModelRole::Assistant,
                content: "now".to_string(),
                content_parts: Vec::new(),
                tool_call_id: None,
                tool_name: None,
                tool_calls: Vec::new(),
                created_at: Some(now_millis()),
                thinking_content: None,
            },
        ];
        assert_eq!(TerseCompaction.micro_compact(&mut messages, 60), 0);
        assert_eq!(messages[0].content, "fresh");
    }
}
