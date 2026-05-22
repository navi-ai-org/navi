use crate::config::HarnessConfig;
use crate::model::{ModelMessage, ModelProvider, ModelRequest, ModelRole, ThinkingConfig};
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};

const READ_ONLY_TOOLS: &[&str] = &["read_file", "list_files", "grep", "bash"];

pub fn micro_compact(messages: &mut Vec<ModelMessage>, gap_threshold_minutes: u64) -> usize {
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
        if msg.role == ModelRole::Tool {
            if let Some(ref tool_name) = msg.tool_name {
                if READ_ONLY_TOOLS.contains(&tool_name.as_str())
                    && !msg.content.contains("[Old tool result content cleared]")
                {
                    msg.content = "[Old tool result content cleared]".to_string();
                    cleared += 1;
                }
            }
        }
    }
    cleared
}

pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
pub const MAX_CONSECUTIVE_FAILURES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactThreshold {
    Normal,
    Warning,
    Error,
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

#[derive(Debug, Clone, Default)]
pub struct CompactState {
    pub last_input_tokens: Option<u64>,
    pub context_window: u64,
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

    pub fn threshold_level(&self) -> CompactThreshold {
        if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            return CompactThreshold::CircuitOpen;
        }
        let Some(input_tokens) = self.last_input_tokens else {
            return CompactThreshold::Normal;
        };
        let remaining = self.context_window.saturating_sub(input_tokens);
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

    pub fn context_percentage(&self) -> u8 {
        let Some(input_tokens) = self.last_input_tokens else {
            return 0;
        };
        if self.context_window == 0 {
            return 0;
        }
        ((input_tokens * 100) / self.context_window).min(100) as u8
    }

    pub fn usage_label(&self) -> String {
        let pct = self.context_percentage();
        let format_tokens = |t: u64| {
            if t >= 1_000_000 {
                format!("{:.1}M", t as f64 / 1_000_000.0)
            } else if t >= 1_000 {
                format!("{}k", t / 1_000)
            } else {
                t.to_string()
            }
        };

        let used = self.last_input_tokens.unwrap_or(0);
        let total = self.context_window;

        format!("{} / {} ({}%)", format_tokens(used), format_tokens(total), pct)
    }

    pub fn update_usage(&mut self, input_tokens: u64) {
        self.last_input_tokens = Some(input_tokens);
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

        let conversation_text = build_conversation_text(messages);
        if conversation_text.trim().is_empty() {
            return Ok(None);
        }

        let prompt = if let Some(ref prev_summary) = self.summary {
            PARTIAL_COMPACT_PROMPT
                .replace("{previous_summary}", prev_summary)
                .replace("{new_conversation}", &conversation_text)
        } else {
            format!(
                "{}\n\nConversation to summarize:\n{}",
                COMPACT_PROMPT, conversation_text
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
                let system_msg = messages
                    .first()
                    .cloned()
                    .unwrap_or_else(|| ModelMessage::system(""));
                messages.clear();
                messages.push(system_msg);
                messages.push(ModelMessage::user(format!(
                    "Here is a summary of the conversation so far:\n\n{}",
                    summary
                )));

                self.summary = Some(summary);
                self.summary_message_count = 0;
                self.consecutive_failures = 0;
                self.last_input_tokens = None;

                let tokens_saved =
                    previous_tokens.saturating_sub(harness_config.autocompact_max_output_tokens);
                tracing::info!(tokens_saved, "auto-compact completed");

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

## 1. Pedido e Intenção Primária
Capture all explicit requests and intents from the user in detail.

## 2. Conceitos Técnicos-Chave
List all technical concepts, technologies, and frameworks discussed.

## 3. Arquivos e Trechos de Código
Enumerate specific files and code snippets that were examined, modified, or created. Include file paths and relevant code.

## 4. Erros e Correções
List all errors that appeared and how they were fixed.

## 5. Resolução de Problemas
Document problems solved and ongoing investigations.

## 6. Todas as Mensagens do Usuário
List ALL user messages that are not tool results. These are critical — include them verbatim, one per line, prefixed with "> ".

## 7. Tarefas Pendentes
List pending tasks that were explicitly requested.

## 8. Trabalho Atual
Describe in precise detail what was being worked on immediately before this summary request.

## 9. Próximo Passo Opcional
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
        assert_eq!(state.threshold_level(), CompactThreshold::Normal);
    }

    #[test]
    fn compact_state_threshold_warning() {
        let state = CompactState {
            last_input_tokens: Some(170_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(), CompactThreshold::Warning);
    }

    #[test]
    fn compact_state_threshold_error() {
        let state = CompactState {
            last_input_tokens: Some(181_000),
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(), CompactThreshold::Error);
    }

    #[test]
    fn compact_state_circuit_breaker() {
        let state = CompactState {
            last_input_tokens: Some(50_000),
            context_window: 200_000,
            consecutive_failures: 3,
            ..Default::default()
        };
        assert_eq!(state.threshold_level(), CompactThreshold::CircuitOpen);
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
        assert_eq!(state.context_percentage(), 50);
    }

    #[test]
    fn compact_state_no_usage_returns_zero_percent() {
        let state = CompactState {
            last_input_tokens: None,
            context_window: 200_000,
            ..Default::default()
        };
        assert_eq!(state.context_percentage(), 0);
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
}
