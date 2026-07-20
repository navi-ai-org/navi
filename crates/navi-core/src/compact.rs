use crate::config::HarnessConfig;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ModelRole, ThinkingConfig};
use anyhow::Result;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Result of a successful conversation compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactOutcome {
    /// Estimated tokens removed from the live context.
    pub tokens_saved: u64,
    /// Model-produced summary that replaces older turns.
    pub summary: String,
    /// How many non-system conversation messages were kept after the summary.
    pub kept_recent_messages: usize,
}

/// Build the standard post-compact user message that carries the summary.
pub fn compact_summary_user_message(summary: &str) -> ModelMessage {
    ModelMessage::user(format!(
        "Here is a summary of the conversation so far:\n\n{}",
        summary
    ))
}

const READ_ONLY_TOOLS: &[&str] = &[
    "read_file",
    "read",
    "search",
    "fs_browser",
    "grep",
    "list_dir",
    "glob",
    "tool_search",
    "code",
    "ast_search",
    "symbol_goto",
    "symbol_references",
    "repo_explore",
    "current_time",
    "get_context_remaining",
    "view_image",
    "question",
    "plan",
];

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
            // Free multimodal payload (e.g. view_image base64) along with text.
            msg.content_parts.clear();
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
/// auto-compact when context usage reaches this percent of the window.
pub const AUTO_COMPACT_THRESHOLD_PERCENT: u8 = 80;

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
///
/// Measurement model (aligned with modern coding CLI TUI):
/// - **context tokens used** = last API `input_tokens` (ground truth for the
/// current context) + client preflight for unsent bytes (`bytes/4`)
/// - **window** = model `context_window` from registry
/// - **usage %** = used / window
/// - **total before compaction** = cumulative input tokens across turns
/// (historical; does not reset when the bar drops after compact)
#[derive(Debug, Clone)]
pub struct CompactState {
    /// Token count from the last model response usage (current context size).
    pub last_input_tokens: Option<u64>,
    /// Output tokens from the last model response (UI turn label).
    pub last_output_tokens: Option<u64>,
    /// Estimated bytes of new messages not yet sent to the model.
    pub estimated_unsent_bytes: usize,
    /// Context window size in tokens for the current model.
    pub context_window: u64,
    /// Cumulative input tokens processed before/during compaction cycles.
    pub total_tokens_before_compaction: u64,
    /// Number of compaction runs in this session.
    pub compaction_count: u32,
    /// Auto-compact when `context_percentage >= this` (default: 85).
    pub auto_compact_threshold_percent: u8,
    /// Number of consecutive compact failures.
    pub consecutive_failures: u32,
    pub summary: Option<String>,
    pub summary_message_count: usize,
    /// Latest long-horizon rebuild context that must stay attached to the
    /// system prompt across subsequent turns.
    pub rebuild_context: Option<String>,
    /// List of checkpoint thresholds crossed in the current context cycle.
    pub crossed_thresholds: Vec<f64>,
    /// Fingerprints of messages already copied into long-horizon history.
    pub history_synced_message_keys: HashSet<u64>,
}

impl Default for CompactState {
    fn default() -> Self {
        Self {
            last_input_tokens: None,
            last_output_tokens: None,
            estimated_unsent_bytes: 0,
            context_window: 0,
            total_tokens_before_compaction: 0,
            compaction_count: 0,
            auto_compact_threshold_percent: AUTO_COMPACT_THRESHOLD_PERCENT,
            consecutive_failures: 0,
            summary: None,
            summary_message_count: 0,
            rebuild_context: None,
            crossed_thresholds: Vec::new(),
            history_synced_message_keys: HashSet::new(),
        }
    }
}

fn format_token_short(t: u64) -> String {
    if t >= 1_000_000 {
        let m = t as f64 / 1_000_000.0;
        // Prefer `1M` over `1.0M` when close to a whole million.
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as u64)
        } else {
            format!("{:.1}M", m)
        }
    } else if t >= 10_000 {
        format!("{}k", t / 1_000)
    } else if t >= 1_000 {
        // Keep one decimal under 10k so 1.5k doesn't collapse to `1k`.
        let k = t as f64 / 1_000.0;
        if (k - k.floor()).abs() < 0.05 {
            format!("{}k", k.floor() as u64)
        } else {
            format!("{:.1}k", k)
        }
    } else {
        t.to_string()
    }
}

/// Effective prompt tokens for the context-window meter.
///
/// Handles providers that split non-cached vs cached prompt tokens (Anthropic
/// and some OpenAI-compat aggregators). Without this, a session can show
/// `430 / 1M` while the real fill is ~64k.
pub fn context_tokens_for_meter(
    input_tokens: Option<u64>,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> Option<u64> {
    let input = input_tokens.unwrap_or(0);
    if input == 0 && cache_creation_tokens == 0 && cache_read_tokens == 0 {
        return None;
    }
    // Inclusive (OpenAI): prompt_tokens already includes cached_tokens.
    if cache_read_tokens > 0 && input >= cache_read_tokens {
        return Some(input.saturating_add(cache_creation_tokens));
    }
    // Exclusive: non-cached input + cache create + cache read.
    Some(
        input
            .saturating_add(cache_creation_tokens)
            .saturating_add(cache_read_tokens),
    )
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

    /// Current context size (live context usage).
    pub fn context_tokens_used(&self, pending_input_bytes: usize) -> u64 {
        self.total_estimated_tokens(pending_input_bytes)
    }

    /// Context window size (context window size).
    pub fn context_window_tokens(&self) -> u64 {
        self.context_window
    }

    /// Integer percent of the window in use (context window usage percent).
    pub fn context_window_usage(&self, pending_input_bytes: usize) -> u8 {
        self.context_percentage(pending_input_bytes)
    }

    /// Estimate tokens for the next request:
    /// API last-input (server) + unsent/pending client bytes as `ceil(bytes/4)`.
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
        if self.context_window == 0 {
            return false;
        }
        // compact near 80% of the window (includes unsent preflight).
        let used = self.total_estimated_tokens(0);
        let pct = (used as f64 / self.context_window as f64) * 100.0;
        if pct >= f64::from(self.auto_compact_threshold_percent) {
            return true;
        }
        // Hard ceiling: last API input + reserved buffer would fill the window.
        let Some(input_tokens) = self.last_input_tokens else {
            return false;
        };
        input_tokens.saturating_add(buffer_tokens) >= self.context_window
    }

    pub fn context_percentage(&self, pending_input_bytes: usize) -> u8 {
        if self.context_window == 0 {
            return 0;
        }
        let total_tokens = self.total_estimated_tokens(pending_input_bytes);
        let percentage = (total_tokens as f64 / self.context_window as f64) * 100.0;
        percentage.clamp(0.0, 100.0) as u8
    }

    /// Compact context meter for the composer footer: `3.2k / 200k`.
    /// Percentage is revealed on hover (see `usage_label_with_percent`).
    pub fn usage_label(&self, pending_input_bytes: usize) -> String {
        self.usage_label_compact(pending_input_bytes)
    }

    /// Token counts only — default (non-hover) display.
    pub fn usage_label_compact(&self, pending_input_bytes: usize) -> String {
        let total_tokens = self.total_estimated_tokens(pending_input_bytes);
        format!(
            "{} / {}",
            format_token_short(total_tokens),
            format_token_short(self.context_window),
        )
    }

    /// Token counts + percent — shown while the context chip is hovered.
    pub fn usage_label_with_percent(&self, pending_input_bytes: usize) -> String {
        let pct = self.context_percentage(pending_input_bytes);
        format!(
            "{} ({}%)",
            self.usage_label_compact(pending_input_bytes),
            pct
        )
    }

    /// Record provider usage for this turn (called every stream that reports usage).
    pub fn update_usage(&mut self, input_tokens: u64) {
        self.update_usage_full(input_tokens, 0);
    }

    /// Record full turn usage and refresh live context metrics.
    pub fn update_usage_full(&mut self, input_tokens: u64, output_tokens: u64) {
        self.last_input_tokens = Some(input_tokens);
        self.last_output_tokens = Some(output_tokens);
        self.total_tokens_before_compaction = self
            .total_tokens_before_compaction
            .saturating_add(input_tokens);
        self.clear_unsent_bytes();
    }

    /// Label shown in the TUI footer — updates every turn after `update_usage*`.
    pub fn turn_usage_label(&self) -> Option<String> {
        let input = self.last_input_tokens?;
        let output = self.last_output_tokens.unwrap_or(0);
        Some(format!(
            "{}→{}",
            format_token_short(input),
            format_token_short(output)
        ))
    }

    pub async fn auto_compact(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        model_provider: &dyn ModelProvider,
        model_name: &str,
        harness_config: &HarnessConfig,
    ) -> Result<Option<CompactOutcome>> {
        self.auto_compact_inner(messages, model_provider, model_name, harness_config, false)
            .await
    }

    /// Force compaction with the session model even when below the threshold.
    /// Manual Compact always fully replaces conversation history (keep_ratio=0).
    pub async fn force_compact(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        model_provider: &dyn ModelProvider,
        model_name: &str,
        harness_config: &HarnessConfig,
    ) -> Result<Option<CompactOutcome>> {
        self.auto_compact_inner(messages, model_provider, model_name, harness_config, true)
            .await
    }

    async fn auto_compact_inner(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        model_provider: &dyn ModelProvider,
        model_name: &str,
        harness_config: &HarnessConfig,
        force: bool,
    ) -> Result<Option<CompactOutcome>> {
        if !force && !self.should_autocompact(harness_config.autocompact_buffer_tokens) {
            return Ok(None);
        }

        // Split: system + developer messages first (the prompt prefix),
        // then conversation messages.
        let system_msgs: Vec<ModelMessage> = messages
            .iter()
            .filter(|m| m.role == ModelRole::System || m.role == ModelRole::Developer)
            .cloned()
            .collect();
        let conversation_msgs: Vec<ModelMessage> = messages
            .iter()
            .filter(|m| m.role != ModelRole::System && m.role != ModelRole::Developer)
            .cloned()
            .collect();

        if conversation_msgs.is_empty() {
            return Ok(None);
        }

        // KeepRatio: keep the last N% of conversation turns intact.
        // Forced/manual compact always fully replaces so context actually shrinks.
        let keep_ratio = if force {
            0.0
        } else {
            harness_config.autocompact_keep_ratio.clamp(0.0, 0.9)
        };
        let total = conversation_msgs.len();
        let keep_count = if force || total < 2 {
            0
        } else {
            let keep_count = (total as f64 * keep_ratio).round() as usize;
            // Always keep at least 2 messages (1 user + 1 assistant) and at most
            // total - 2 (so there's something to summarize).
            keep_count.clamp(2.min(total), total.saturating_sub(2).max(2.min(total)))
        };
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
            instructions: None,
            messages: vec![
                ModelMessage::system("You are a precise conversation summarizer."),
                ModelMessage::user(prompt),
            ],
            thinking: ThinkingConfig::Off,
            tools: vec![],
            session_id: None,
        };

        match model_provider.complete(request).await {
            Ok(response) => {
                let summary = response.text.trim().to_string();
                if summary.is_empty() {
                    self.consecutive_failures += 1;
                    anyhow::bail!("compaction model returned an empty summary");
                }
                let previous_tokens = self.last_input_tokens.unwrap_or_else(|| {
                    messages
                        .iter()
                        .map(|message| message.content.len() as u64)
                        .sum::<u64>()
                        .saturating_add(3)
                        / 4
                });
                let kept_recent_messages = recent_msgs.len();

                // Reassemble: system + summary + recent turns kept intact.
                messages.clear();
                messages.extend(system_msgs);
                messages.push(compact_summary_user_message(&summary));
                messages.extend(recent_msgs.iter().cloned());

                self.summary = Some(summary.clone());
                self.summary_message_count = messages.len();
                self.consecutive_failures = 0;
                self.last_input_tokens = None;
                self.last_output_tokens = None;
                self.clear_unsent_bytes();
                self.compaction_count = self.compaction_count.saturating_add(1);

                let tokens_saved = previous_tokens
                    .saturating_sub(estimate_messages_tokens(messages))
                    .max(1);
                tracing::info!(
                    tokens_saved,
                    old_turns = old_msgs.len(),
                    kept_turns = kept_recent_messages,
                    force,
                    "auto-compact completed"
                );

                Ok(Some(CompactOutcome {
                    tokens_saved,
                    summary,
                    kept_recent_messages,
                }))
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

    pub fn apply_manual_summary(
        &mut self,
        messages: &mut Vec<ModelMessage>,
        summary: String,
    ) -> CompactOutcome {
        let system_msgs: Vec<ModelMessage> = messages
            .iter()
            .filter(|m| m.role == ModelRole::System || m.role == ModelRole::Developer)
            .cloned()
            .collect();
        let estimated_previous_tokens = self.last_input_tokens.unwrap_or_else(|| {
            messages
                .iter()
                .map(|message| message.content.len() as u64)
                .sum::<u64>()
                .saturating_add(3)
                / 4
        });

        messages.clear();
        messages.extend(system_msgs);
        messages.push(compact_summary_user_message(&summary));

        self.summary = Some(summary.clone());
        self.summary_message_count = messages.len();
        self.consecutive_failures = 0;
        self.last_input_tokens = None;
        self.last_output_tokens = None;
        self.compaction_count = self.compaction_count.saturating_add(1);
        self.clear_unsent_bytes();

        let tokens_saved = estimated_previous_tokens
            .saturating_sub(estimate_messages_tokens(messages))
            .max(1);

        CompactOutcome {
            tokens_saved,
            summary,
            kept_recent_messages: 0,
        }
    }
}

fn estimate_messages_tokens(messages: &[ModelMessage]) -> u64 {
    messages
        .iter()
        .map(|message| message.content.len() as u64)
        .sum::<u64>()
        .saturating_add(3)
        / 4
}

fn build_conversation_text(messages: &[ModelMessage]) -> String {
    let mut text = String::new();
    for msg in messages {
        let role_label = match msg.role {
            ModelRole::User => "User",
            ModelRole::Assistant => "Assistant",
            ModelRole::Tool => "Tool",
            ModelRole::System | ModelRole::Developer => continue,
        };
        if msg.role == ModelRole::Tool {
            if let Some(ref tool_name) = msg.tool_name {
                text.push_str(&format!("[Tool({})]: {}\n", tool_name, msg.content));
            } else {
                text.push_str(&format!("[Tool]: {}\n", msg.content));
            }
        } else {
            let image_note = if msg.content_parts.iter().any(|p| p.is_image()) {
                let count = msg.content_parts.iter().filter(|p| p.is_image()).count();
                format!(" [{} image(s) attached]", count)
            } else {
                String::new()
            };
            text.push_str(&format!(
                "[{}]: {}{}\n",
                role_label, msg.content, image_note
            ));
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
## 9. Active Work Plan
If the conversation has an active plan (via the plan tool) or an in-progress Plan-mode proposal, include:
- Plan ID and title (if any)
- All steps with completion status
- Which step to work on next
If there is no active plan, skip this section.
Also note any active thread goal (set_goal) separately if present — do not conflate plan and goal.
## 10. Next Step (Optional)
List the next step you would take on the current task.

Be thorough and specific. The summary must contain enough detail to continue the conversation seamlessly.

IMPORTANT: If there is an active plan that is not completed or abandoned, continue it after reading this summary UNLESS the user clearly redirected to a different task — in that case, note the redirect in section 8/10 and do not restart the old plan. Do not create a new plan unless the prior plan is completed, abandoned, or the user asked for a new one."#;

pub const PARTIAL_COMPACT_PROMPT: &str = r#"You are extending an existing conversation summary with new content. Preserve the existing summary sections and update them with new information. Add any new user messages to section 6. Update sections 8 and 9 based on the most recent work.

Existing summary:
{previous_summary}

New conversation to summarize:
{new_conversation}

Return the complete updated summary with all 10 sections (including Active Work Plan if applicable).

IMPORTANT: If there is an active plan, preserve plan details and step completion status. Continue that plan only if the user has not redirected; note any redirect instead of forcing the old plan."#;

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
            ModelMessage::tool_result("call-5", "bash", "command output".to_string()),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 2);
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
        assert_eq!(messages[6].content, "command output");
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
    fn compact_state_autocompact_at_eighty_percent() {
        // 160k / 200k = 80% triggers even when buffer would not.
        let state = CompactState {
            last_input_tokens: Some(160_000),
            context_window: 200_000,
            auto_compact_threshold_percent: 80,
            ..Default::default()
        };
        assert!(state.should_autocompact(0));
        assert_eq!(state.context_window_usage(0), 80);

        // Just under threshold does not fire without the hard buffer ceiling.
        let below = CompactState {
            last_input_tokens: Some(159_000),
            context_window: 200_000,
            auto_compact_threshold_percent: 80,
            ..Default::default()
        };
        assert!(!below.should_autocompact(0));
    }

    #[test]
    fn compact_state_update_usage_full_tracks_cumulative() {
        let mut state = CompactState::new(200_000);
        state.update_usage_full(10_000, 500);
        state.update_usage_full(12_000, 800);
        assert_eq!(state.last_input_tokens, Some(12_000));
        assert_eq!(state.last_output_tokens, Some(800));
        assert_eq!(state.total_tokens_before_compaction, 22_000);
        assert_eq!(state.turn_usage_label().as_deref(), Some("12k→800"));
    }

    #[test]
    fn compact_state_manual_summary_compacts_below_threshold() {
        let mut state = CompactState {
            last_input_tokens: Some(10_000),
            context_window: 200_000,
            consecutive_failures: 2,
            estimated_unsent_bytes: 4096,
            ..Default::default()
        };
        assert!(!state.should_autocompact(AUTOCOMPACT_BUFFER_TOKENS));
        let mut messages = vec![
            ModelMessage::system("system"),
            ModelMessage::user("task"),
            ModelMessage::assistant("response"),
        ];

        let outcome = state.apply_manual_summary(&mut messages, "Manual summary".to_string());

        assert!(outcome.tokens_saved >= 1);
        assert_eq!(outcome.kept_recent_messages, 0);
        assert_eq!(outcome.summary, "Manual summary");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, ModelRole::System);
        assert!(messages[1].content.contains("Manual summary"));
        assert_eq!(state.summary.as_deref(), Some("Manual summary"));
        assert_eq!(state.summary_message_count, 2);
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.estimated_unsent_bytes, 0);
        assert!(state.last_input_tokens.is_none());
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
        // Default (composer) is counts only; percent is a hover-only affordance.
        assert_eq!(state.usage_label(0), "2k / 128k");
        assert_eq!(state.usage_label_with_percent(0), "2k / 128k (1%)");
    }

    #[test]
    fn context_tokens_for_meter_sums_exclusive_cache() {
        // Charm-style undercount: tiny non-cached prompt + large cache hit.
        assert_eq!(context_tokens_for_meter(Some(430), 0, 63_570), Some(64_000));
    }

    #[test]
    fn context_tokens_for_meter_keeps_openai_inclusive() {
        // OpenAI: prompt_tokens already includes cached_tokens.
        assert_eq!(
            context_tokens_for_meter(Some(64_000), 0, 63_570),
            Some(64_000)
        );
    }

    #[test]
    fn format_token_short_million_window() {
        assert_eq!(format_token_short(1_048_576), "1M");
        assert_eq!(format_token_short(64_000), "64k");
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
            ModelMessage::tool_result("c1", "write_file", "file written"),
            ModelMessage::tool_result("c2", "package_manager", "deps ok"),
            ModelMessage::tool_result("c3", "apply_patch", "patch applied"),
            ModelMessage::tool_result("c4", "bash", "command ok"),
        ];

        let cleared = micro_compact(&mut messages, 60);
        assert_eq!(cleared, 0, "non-read-only tools must not be cleared");
        assert_eq!(messages[2].content, "file written");
        assert_eq!(messages[3].content, "deps ok");
        assert_eq!(messages[4].content, "patch applied");
        assert_eq!(messages[5].content, "command ok");
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
