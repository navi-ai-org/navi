use crate::config::HarnessConfig;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ModelRole, ThinkingConfig};
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

const READ_ONLY_TOOLS: &[&str] = &["read_file", "fs_browser", "grep", "bash", "git_ops"];

/// Removes read-only tool results from older messages when idle time exceeds
/// the gap threshold. Returns the number of messages cleared.
pub fn micro_compact(messages: &mut [ModelMessage], gap_threshold_minutes: u64) -> usize {
    let now = current_unix_millis();
    let gap_threshold_ms = gap_threshold_minutes * 60 * 1000;

    let last_assistant_ts = messages
        .iter()
        .rev()
        .find(|m| m.role == ModelRole::Assistant)
        .and_then(|m| m.created_at);

    let Some(last_ts) = last_assistant_ts else {
        return 0;
    };

    if now.saturating_sub(last_ts) < gap_threshold_ms {
        return 0;
    }

    let mut cleared = 0;
    for msg in messages.iter_mut() {
        if msg.role == ModelRole::Tool
            && let Some(ref tool_name) = msg.tool_name
            && READ_ONLY_TOOLS.contains(&tool_name.as_str())
            && !msg.content.contains("[Old tool result content cleared]")
        {
            msg.content = "[Old tool result content cleared]".to_string();
            cleared += 1;
        }
    }
    cleared
}

pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// Context usage severity level used to trigger compact warnings and errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactThreshold {
    /// Context usage is within normal bounds.
    Normal,
    /// Context usage is approaching the limit; a warning should be shown.
    Warning,
    /// Context usage is critically close to the limit.
    Error,
    /// Compact has failed too many times; further attempts are blocked.
    CircuitOpen,
}

impl std::fmt::Display for CompactThreshold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompactThreshold::Normal => write!(f, "ok"),
            CompactThreshold::Warning => write!(f, "warning"),
            CompactThreshold::Error => write!(f, "error"),
            CompactThreshold::CircuitOpen => write!(f, "circuit-open"),
        }
    }
}

/// Tracks token usage and compact failure state for autocompact decisions.
#[derive(Debug, Clone, Default)]
pub struct CompactState {
    /// Token count from the last model response, if available.
    pub last_input_tokens: Option<u64>,
    /// Estimated bytes of new messages not yet sent to the model.
    pub estimated_unsent_bytes: usize,
    /// Context window size in tokens for the current model.
    pub context_window: u64,
    /// Number of consecutive compact failures.
    pub consecutive_failures: u32,
    pub summary: Option<String>,
    pub summary_message_count: usize,
}

impl CompactState {
    pub fn new(context_window: u64) -> Self {
        Self {
            context_window,
            ..Default::default()
        }
    }

    pub fn add_unsent_bytes(&mut self, bytes: usize) {
        self.estimated_unsent_bytes += bytes;
    }

    pub fn clear_unsent_bytes(&mut self) {
        self.estimated_unsent_bytes = 0;
    }

    pub fn total_estimated_tokens(&self, pending_input_bytes: usize) -> u64 {
        let server_tokens = self.last_input_tokens.unwrap_or(0);
        let client_bytes = self.estimated_unsent_bytes + pending_input_bytes;
        let client_tokens = (client_bytes.saturating_add(3) / 4) as u64;
        server_tokens + client_tokens
    }

    pub fn threshold_level(&self, pending_input_bytes: usize) -> CompactThreshold {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return CompactThreshold::CircuitOpen;
        }
        let total_tokens = self.total_estimated_tokens(pending_input_bytes);
        if total_tokens == 0 {
            return CompactThreshold::Normal;
        }
        let remaining = self.context_window.saturating_sub(total_tokens);
        if remaining <= ERROR_THRESHOLD_BUFFER_TOKENS {
            CompactThreshold::Error
        } else if remaining <= WARNING_THRESHOLD_BUFFER_TOKENS + AUTOCOMPACT_BUFFER_TOKENS {
            CompactThreshold::Warning
        } else {
            CompactThreshold::Normal
        }
    }

    pub fn should_autocompact(&self, buffer_tokens: u64) -> bool {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return false;
        }
        let Some(input_tokens) = self.last_input_tokens else {
            return false;
        };
        input_tokens + buffer_tokens >= self.context_window
    }

    pub fn context_percentage(&self, pending_input_bytes: usize) -> u8 {
        if self.context_window == 0 {
            return 0;
        }
        let total_tokens = self.total_estimated_tokens(pending_input_bytes);
        let percentage = (total_tokens as f64 / self.context_window as f64) * 100.0;
        percentage.clamp(0.0, 100.0) as u8
    }

    pub fn usage_label(&self, pending_input_bytes: usize) -> String {
        let pct = self.context_percentage(pending_input_bytes);
        let format_tokens = |t: u64| {
            if t >= 1_000_000 {
                format!("{:.1}M", t as f64 / 1_000_000.0)
            } else if t >= 1_000 {
                format!("{}k", t / 1_000)
            } else {
                t.to_string()
            }
        };

        let total_tokens = self.total_estimated_tokens(pending_input_bytes);

        format!(
            "{} / {} ({}%)",
            format_tokens(total_tokens),
            format_tokens(self.context_window),
            pct
        )
    }

    pub fn update_usage(&mut self, input_tokens: u64) {
        self.last_input_tokens = Some(input_tokens);
        self.clear_unsent_bytes();
    }

    pub async fn auto_compact(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        model_provider: &dyn ModelProvider,
        model_name: &str,
        harness_config: &HarnessConfig,
    ) -> Result<Option<u64>> {
        if !self.should_autocompact(harness_config.autocompact_buffer_tokens) {
            return Ok(None);
        }

        // Split: system message(s) first, then conversation messages.
        let system_msgs: Vec<ModelMessage> = messages
            .iter()
            .filter(|m| m.role == ModelRole::System)
            .cloned()
            .collect();
        let conversation_msgs: Vec<ModelMessage> = messages
            .iter()
            .filter(|m| m.role != ModelRole::System)
            .cloned()
            .collect();

        if conversation_msgs.is_empty() {
            return Ok(None);
        }

        // KeepRatio: keep the last N% of conversation turns intact.
        let keep_ratio = harness_config.autocompact_keep_ratio.clamp(0.0, 0.9);
        let total = conversation_msgs.len();
        let keep_count = (total as f64 * keep_ratio).round() as usize;
        // Always keep at least 2 messages (1 user + 1 assistant) and at most
        // total - 2 (so there's something to summarize).
        let keep_count = keep_count.clamp(2.min(total), total.saturating_sub(2).max(2.min(total)));
        let split_at = total.saturating_sub(keep_count);

        // Old messages → summarize. Recent messages → keep intact.
        let (old_msgs, recent_msgs) = conversation_msgs.split_at(split_at);
        let old_text = build_conversation_text(old_msgs);

        if old_text.trim().is_empty() {
            // Nothing old to summarize; just keep everything.
            return Ok(None);
        }

        let prompt = if let Some(ref prev_summary) = self.summary {
            PARTIAL_COMPACT_PROMPT
                .replace("{previous_summary}", prev_summary)
                .replace("{new_conversation}", &old_text)
        } else {
            format!(
                "{}\n\nConversation to summarize:\n{}",
                COMPACT_PROMPT, old_text
            )
        };

        let request = ModelRequest {
            model: model_name.to_string(),
            messages: vec![
                ModelMessage::system("You are a precise conversation summarizer."),
                ModelMessage::user(prompt),
            ],
            thinking: ThinkingConfig::Off,
            tools: vec![],
        };

        match model_provider.complete(request).await {
            Ok(response) => {
                let summary = response.text;
                let previous_tokens = self.last_input_tokens.unwrap_or(0);

                // Reassemble: system + summary + recent turns kept intact.
                messages.clear();
                messages.extend(system_msgs);
                messages.push(ModelMessage::user(format!(
                    "Here is a summary of the conversation so far:\n\n{}",
                    summary
                )));
                messages.extend(recent_msgs.iter().cloned());

                self.summary = Some(summary);
                self.summary_message_count = messages.len();
                self.consecutive_failures = 0;
                self.last_input_tokens = None;

                let tokens_saved =
                    previous_tokens.saturating_sub(harness_config.autocompact_max_output_tokens);
                tracing::info!(
                    tokens_saved,
                    old_turns = old_msgs.len(),
                    kept_turns = recent_msgs.len(),
                    "auto-compact completed"
                );

                Ok(Some(tokens_saved))
            }
            Err(e) => {
                self.consecutive_failures += 1;
                tracing::warn!(
                    failures = self.consecutive_failures,
                    error = %e,
                    "auto-compact failed"
                );
                Err(e)
            }
        }
    }
}

fn build_conversation_text(messages: &[ModelMessage]) -> String {
    let mut text = String::new();
    for msg in messages {
        if msg.role == ModelRole::System {
            continue;
        }
        let role_label = match msg.role {
            ModelRole::User => "User",
            ModelRole::Assistant => "Assistant",
            ModelRole::Tool => "Tool",
            ModelRole::System => continue,
        };
        if msg.role == ModelRole::Tool {
            if let Some(ref tool_name) = msg.tool_name {
                text.push_str(&format!("[Tool({})]: {}\n", tool_name, msg.content));
            } else {
                text.push_str(&format!("[Tool]: {}\n", msg.content));
            }
        } else {
            text.push_str(&format!("[{}]: {}\n", role_label, msg.content));
        }
    }
    text
}

pub const COMPACT_PROMPT: &str = r#"You are summarizing a conversation between a user and an AI coding assistant (NAVI). Create a detailed summary with these exact sections:

## 1. Primary Request and Intent
## 2. Key Technical Concepts
## 3. Files and Code Snippets
## 4. Errors and Fixes
## 5. Problem Resolution
## 6. All User Messages
## 7. Pending Tasks
## 8. Current Work
## 9. Next Step (Optional)
List the next step you would take on the current task.

Be thorough and specific. The summary must contain enough detail to continue the conversation seamlessly."#;

pub const PARTIAL_COMPACT_PROMPT: &str = r#"You are extending an existing conversation summary with new content. Preserve the existing summary sections and update them with new information. Add any new user messages to section 6. Update sections 8 and 9 based on the most recent work.

Existing summary:
{previous_summary}

New conversation to summarize:
{new_conversation}

Return the complete updated summary with all 9 sections."#;

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelMessage;

    #[test]
    fn micro_compact_clears_read_only_tools_after_gap() {
        let now = current_unix_millis();
        let gap_ms: u64 = 61 * 60 * 1000;

        let mut messages = vec![
            ModelMessage::system("system"),
            ModelMessage::user("task"),
            {
                let mut m = ModelMessage::assistant("response");
                m.created_at = Some(now.saturating_sub(gap_ms));
                m
            },
            ModelMessage::tool_result("call-1", "read_file", "file content here".to_string()),
            ModelMessage::tool_result("call-2", "write_file", "written content".to_string()),
            ModelMessage::tool_result("call-3", "grep", "match results".to_string()),
            ModelMessage::tool_result("call-4", "bash", "command output".to_string()),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 3);
        assert!(
            messages[3]
                .content
                .contains("[Old tool result content cleared]")
        );
        assert_eq!(messages[4].content, "written content");
        assert!(
            messages[5]
                .content
                .contains("[Old tool result content cleared]")
        );
        assert!(
            messages[6]
                .content
                .contains("[Old tool result content cleared]")
        );
    }

    #[test]
    fn micro_compact_no_gap_returns_zero() {
        let mut messages = vec![
            ModelMessage::system("system"),
            ModelMessage::user("task"),
            ModelMessage::assistant("response"),
            ModelMessage::tool_result("call-1", "read_file", "content".to_string()),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0);
    }

    #[test]
    fn micro_compact_no_double_clear() {
        let now = current_unix_millis();
        let gap_ms: u64 = 61 * 60 * 1000;

        let mut messages = vec![
            ModelMessage::system("system"),
            {
                let mut m = ModelMessage::assistant("response");
                m.created_at = Some(now.saturating_sub(gap_ms));
                m
            },
            ModelMessage::tool_result(
                "call-1",
                "read_file",
                "[Old tool result content cleared]".to_string(),
            ),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0);
    }

    #[test]
    fn compact_state_threshold_normal() {
        let state = CompactState {
            last_input_tokens: Some(50_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(0), CompactThreshold::Normal);
    }

    #[test]
    fn compact_state_threshold_warning() {
        let state = CompactState {
            last_input_tokens: Some(170_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(0), CompactThreshold::Warning);
    }

    #[test]
    fn compact_state_threshold_error() {
        let state = CompactState {
            last_input_tokens: Some(181_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(0), CompactThreshold::Error);
    }

    #[test]
    fn compact_state_circuit_breaker() {
        let state = CompactState {
            last_input_tokens: Some(50_000),
            context_window: 200_000,
            consecutive_failures: 3,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(0), CompactThreshold::CircuitOpen);
        assert!(!state.should_autocompact(AUTOCOMPACT_BUFFER_TOKENS));
    }

    #[test]
    fn compact_state_should_autocompact() {
        let state = CompactState {
            last_input_tokens: Some(190_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert!(state.should_autocompact(AUTOCOMPACT_BUFFER_TOKENS));
    }

    #[test]
    fn compact_state_context_percentage() {
        let state = CompactState {
            last_input_tokens: Some(100_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.context_percentage(0), 50);
    }

    #[test]
    fn compact_state_no_usage_returns_zero_percent() {
        let state = CompactState {
            last_input_tokens: None,
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.context_percentage(0), 0);
    }

    #[test]
    fn compact_state_usage_label_shows_real_context_usage() {
        let state = CompactState {
            last_input_tokens: Some(2_000),
            context_window: 128_000,
            ..Default::default()
        };
        assert_eq!(state.usage_label(0), "2k / 128k (1%)");
    }

    #[test]
    fn write_tool_preserved_in_micro_compact() {
        let now = current_unix_millis();
        let gap_ms: u64 = 61 * 60 * 1000;

        let mut messages = vec![
            ModelMessage::system("system"),
            {
                let mut m = ModelMessage::assistant("response");
                m.created_at = Some(now.saturating_sub(gap_ms));
                m
            },
            ModelMessage::tool_result("call-1", "write_file", "content written".to_string()),
            ModelMessage::tool_result("call-2", "apply_patch", "patch applied".to_string()),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0);
        assert_eq!(messages[2].content, "content written");
        assert_eq!(messages[3].content, "patch applied");
    }

    // ── Regression tests ──────────────────────────────────────────────────────

    #[test]
    fn regression_micro_compact_no_assistant_messages_returns_zero() {
        let mut messages = vec![
            ModelMessage::system("system"),
            ModelMessage::user("hello"),
            ModelMessage::tool_result("c1", "read_file", "content"),
        ];
        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0);
    }

    #[test]
    fn regression_micro_compact_preserves_non_readonly_tools() {
        let now = current_unix_millis();
        let gap_ms: u64 = 61 * 60 * 1000;

        let mut messages = vec![
            ModelMessage::system("system"),
            {
                let mut m = ModelMessage::assistant("response");
                m.created_at = Some(now.saturating_sub(gap_ms));
                m
            },
            ModelMessage::tool_result("c1", "test_runner", "tests passed"),
            ModelMessage::tool_result("c2", "build_runner", "build ok"),
            ModelMessage::tool_result("c3", "package_manager", "deps ok"),
            ModelMessage::tool_result("c4", "apply_patch", "patch applied"),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0, "non-read-only tools must not be cleared");
        assert_eq!(messages[2].content, "tests passed");
        assert_eq!(messages[3].content, "build ok");
        assert_eq!(messages[4].content, "deps ok");
        assert_eq!(messages[5].content, "patch applied");
    }

    #[test]
    fn regression_compact_state_percentage_clamps_to_100() {
        let mut state = CompactState::new(1000);
        // Simulate more tokens than window
        state.last_input_tokens = Some(2000);
        let pct = state.context_percentage(0);
        assert!(pct <= 100, "percentage must clamp to 100, got {pct}");
    }

    #[test]
    fn regression_compact_state_usage_label_formats_millions() {
        let state = CompactState {
            last_input_tokens: Some(1_500_000),
            context_window: 2_000_000,
            ..Default::default()
        };
        let label = state.usage_label(0);
        assert!(label.contains("M"), "should use M format for millions");
    }

    #[test]
    fn regression_build_conversation_text_excludes_system() {
        let messages = vec![
            ModelMessage::system("you are a helpful assistant"),
            ModelMessage::user("hello"),
            ModelMessage::assistant("hi there"),
        ];
        let text = build_conversation_text(&messages);
        assert!(
            !text.contains("you are a helpful assistant"),
            "system message must be excluded"
        );
        assert!(text.contains("hello"));
        assert!(text.contains("hi there"));
    }

    #[test]
    fn regression_build_conversation_text_includes_tool_name() {
        let messages = vec![
            ModelMessage::user("read file"),
            ModelMessage::tool_result("c1", "read_file", "file content"),
        ];
        let text = build_conversation_text(&messages);
        assert!(
            text.contains("read_file"),
            "tool name must be included in conversation text"
        );
    }

    #[test]
    fn keep_ratio_clamps_valid_range() {
        let config = HarnessConfig {
            autocompact_keep_ratio: 0.25,
            ..Default::default()
        };
        assert_eq!(config.autocompact_keep_ratio, 0.25);
    }

    #[test]
    fn keep_ratio_default_is_25_percent() {
        let config = HarnessConfig::default();
        assert_eq!(config.autocompact_keep_ratio, 0.25);
    }

    #[test]
    fn build_conversation_text_preserves_order() {
        let messages = vec![
            ModelMessage::user("first"),
            ModelMessage::assistant("second"),
            ModelMessage::user("third"),
            ModelMessage::assistant("fourth"),
        ];
        let text = build_conversation_text(&messages);
        let first_pos = text.find("first").unwrap();
        let second_pos = text.find("second").unwrap();
        let third_pos = text.find("third").unwrap();
        let fourth_pos = text.find("fourth").unwrap();
        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
        assert!(third_pos < fourth_pos);
    }
}
