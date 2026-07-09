//! Session recap — short post-turn summary (Grok Build TUI analogue).
//!
//! After a successful turn, NAVI can surface a one-line "Recap" of what happened.
//! Long-tail turns are summarized into an artifact path but may be suppressed in the UI.

use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::Result;

/// Suppress UI recap when the assistant output is a long dump (Grok long-tail).
pub const RECAP_LONG_TAIL_CHARS: usize = 4_000;
/// Max tool calls before treating a turn as long-tail (display suppressed).
pub const RECAP_LONG_TAIL_TOOL_CALLS: usize = 40;
/// Soft max length for a displayed recap line.
pub const RECAP_MAX_DISPLAY_CHARS: usize = 280;

/// Whether the recap should be hidden in the chat (artifact may still be saved).
pub fn should_suppress_recap(assistant_chars: usize, tool_call_count: usize) -> bool {
    assistant_chars > RECAP_LONG_TAIL_CHARS || tool_call_count > RECAP_LONG_TAIL_TOOL_CALLS
}

/// Fast extractive recap — no model call. Prefer for UI latency.
pub fn local_recap(user_prompt: &str, assistant_text: &str) -> String {
    let user = collapse_ws(user_prompt);
    let assistant = collapse_ws(assistant_text);

    let body = if !assistant.is_empty() {
        first_sentence(&assistant)
    } else if !user.is_empty() {
        format!("Handled: {}", first_sentence(&user))
    } else {
        "Turn completed.".to_string()
    };

    truncate_chars(&body, RECAP_MAX_DISPLAY_CHARS)
}

/// LLM recap (Grok-style). Uses a short system prompt; caller should pass a
/// compact transcript excerpt only.
pub async fn llm_recap(
    provider: &dyn ModelProvider,
    model: &str,
    user_prompt: &str,
    assistant_text: &str,
    tool_names: &[String],
) -> Result<String> {
    let tools = if tool_names.is_empty() {
        "none".to_string()
    } else {
        let mut names = tool_names.to_vec();
        names.sort();
        names.dedup();
        names.join(", ")
    };

    let user = truncate_chars(&collapse_ws(user_prompt), 600);
    let assistant = truncate_chars(&collapse_ws(assistant_text), 1_200);

    let prompt = format!(
        "Write a 1-2 sentence recap of this coding-agent turn for the user.\n\
         Be concrete (what changed / what was decided). No preamble, no markdown headings.\n\n\
         User: {user}\n\
         Assistant: {assistant}\n\
         Tools used: {tools}\n"
    );

    let request = ModelRequest {
        model: model.to_string(),
        instructions: None,
        messages: vec![
            ModelMessage::system(
                "You write terse session recaps for a coding agent CLI. One or two sentences only.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = provider.complete(request).await?;
    let cleaned = clean_recap_text(&response.text);
    if cleaned.is_empty() {
        Ok(local_recap(user_prompt, assistant_text))
    } else {
        Ok(truncate_chars(&cleaned, RECAP_MAX_DISPLAY_CHARS))
    }
}

fn clean_recap_text(raw: &str) -> String {
    let mut text = collapse_ws(raw);
    for prefix in ["Recap:", "Summary:", "TL;DR:", "tl;dr:"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim().to_string();
        }
    }
    text
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let end = trimmed
        .find(['.', '!', '?'])
        .map(|i| i + 1)
        .unwrap_or(trimmed.len().min(200));
    collapse_ws(&trimmed[..end])
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_recap_uses_first_assistant_sentence() {
        let recap = local_recap(
            "fix the footer",
            "Updated the token meter. Also refactored helpers.",
        );
        assert!(recap.starts_with("Updated the token meter"));
        assert!(!recap.contains("refactored"));
    }

    #[test]
    fn suppress_long_tail_output() {
        assert!(should_suppress_recap(RECAP_LONG_TAIL_CHARS + 1, 0));
        assert!(should_suppress_recap(10, RECAP_LONG_TAIL_TOOL_CALLS + 1));
        assert!(!should_suppress_recap(100, 2));
    }

    #[test]
    fn truncate_adds_ellipsis() {
        let long = "x".repeat(500);
        let out = truncate_chars(&long, 50);
        assert_eq!(out.chars().count(), 50);
        assert!(out.ends_with('…'));
    }
}
