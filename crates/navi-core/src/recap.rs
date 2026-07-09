//! Session recap — short post-turn summary after each successful turn.
//!
//! Shows a compact one-liner under a "Recap" label:
//! - You asked … : <concrete specifics>
//! - We <past-tense verb> … : <concrete specifics>
//! ~25–40 words, never a wall of assistant prose.

use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::Result;

/// Suppress UI recap when the assistant output is a long dump (long-tail).
pub const RECAP_LONG_TAIL_CHARS: usize = 4_000;
/// Max tool calls before treating a turn as long-tail (display suppressed).
pub const RECAP_LONG_TAIL_TOOL_CALLS: usize = 40;
/// Soft max length for a displayed recap (one short sentence).
pub const RECAP_MAX_DISPLAY_CHARS: usize = 160;
/// Prefer staying under this word count (~25–40 words).
const RECAP_TARGET_WORDS: usize = 40;

/// Whether the recap should be hidden in the chat (artifact may still be saved).
pub fn should_suppress_recap(assistant_chars: usize, tool_call_count: usize) -> bool {
    assistant_chars > RECAP_LONG_TAIL_CHARS || tool_call_count > RECAP_LONG_TAIL_TOOL_CALLS
}

/// Fast extractive/synthetic recap — no model call. Prefer for UI latency / fallback.
///
/// Produces a **short** outcome line, not a dump of the assistant message.
pub fn local_recap(user_prompt: &str, assistant_text: &str) -> String {
    local_recap_with_tools(user_prompt, assistant_text, &[])
}

/// Like [`local_recap`], optionally using tool names for "We …" framing.
pub fn local_recap_with_tools(
    user_prompt: &str,
    assistant_text: &str,
    tool_names: &[String],
) -> String {
    let user = collapse_ws(user_prompt);
    let assistant = collapse_ws(assistant_text);
    let body = synthesize_local(&user, &assistant, tool_names);
    finalize_recap(&body)
}

/// LLM recap. One short sentence only.
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
        names.into_iter().take(8).collect::<Vec<_>>().join(", ")
    };

    let user = truncate_chars(&collapse_ws(user_prompt), 280);
    let assistant = truncate_chars(&collapse_ws(assistant_text), 520);

    let prompt = format!(
        "Write ONE sentence recap body for this coding-agent turn. \
         Output ONLY the body (the UI adds the \"Recap\" label).\n\n\
         Lead with agency:\n\
         - \"You asked …\" if the turn was mainly questions, walkthroughs, planning, \
           or review with no landed change.\n\
         - \"We <past-tense verb> …\" if the agent implemented, fixed, installed, \
           merged, or changed code/config/docs (e.g. \"We fixed\", \"We wired\" — \
           not \"We did fix\").\n\
         - If almost nothing happened: \"You had just begun this session.\"\n\n\
         Shape: <lead>: <concrete specifics — file/crate/flag/behavior>. \
         ~25–40 words. One sentence. No markdown, no bullets, no quotes, no preamble.\n\n\
         User: {user}\n\
         Assistant: {assistant}\n\
         Tools: {tools}\n"
    );

    let request = ModelRequest {
        model: model.to_string(),
        instructions: None,
        messages: vec![
            ModelMessage::system(
                "You write ultra-short session recaps for a coding CLI. \
                 Exactly one concise sentence. Past-tense outcomes preferred when work landed. \
                 Never invent work not reflected in the turn. Never dump the assistant message.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
    };

    let response = provider.complete(request).await?;
    let cleaned = clean_recap_text(&response.text);
    if cleaned.is_empty() {
        Ok(local_recap_with_tools(
            user_prompt,
            assistant_text,
            tool_names,
        ))
    } else {
        Ok(finalize_recap(&cleaned))
    }
}

fn clean_recap_text(raw: &str) -> String {
    let mut text = collapse_ws(raw);
    for prefix in [
        "Recap:",
        "Recap",
        "Summary:",
        "TL;DR:",
        "tl;dr:",
        "Here's a recap:",
        "Here is a recap:",
        "◈ recap",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest
                .trim_start_matches([':', '—', '-', ' '])
                .trim()
                .to_string();
        }
    }
    // Drop surrounding quotes the model sometimes adds.
    if (text.starts_with('"') && text.ends_with('"'))
        || (text.starts_with('“') && text.ends_with('”'))
    {
        text = text
            .chars()
            .skip(1)
            .take(text.chars().count().saturating_sub(2))
            .collect();
    }
    // Keep a single sentence.
    first_sentence(&text)
}

/// Synthesize a short short line without calling a model.
fn synthesize_local(user: &str, assistant: &str, tool_names: &[String]) -> String {
    let user_snip = snippet_topic(user, 72);
    let asst_snip = snippet_outcome(assistant, 88);
    let has_tools = !tool_names.is_empty();
    let looks_like_landed =
        has_tools || looks_landed_change(assistant) || looks_landed_change(user);

    if user_snip.is_empty() && asst_snip.is_empty() {
        return "Turn completed.".to_string();
    }

    if looks_like_landed {
        if !asst_snip.is_empty() {
            // Prefer concrete outcome from assistant when short enough.
            if !is_process_language(&asst_snip) {
                return format!("We finished: {asst_snip}");
            }
        }
        if !user_snip.is_empty() {
            return format!("We worked on: {user_snip}");
        }
        return format!("We finished: {asst_snip}");
    }

    // Question / plan / no landed change — "You asked …"
    if !user_snip.is_empty() {
        if !asst_snip.is_empty() && !is_process_language(&asst_snip) {
            return format!("You asked about {user_snip}: {asst_snip}");
        }
        return format!("You asked: {user_snip}");
    }

    if !asst_snip.is_empty() {
        return asst_snip;
    }

    "Turn completed.".to_string()
}

fn looks_landed_change(text: &str) -> bool {
    let lower = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "installed",
        "fixed",
        "updated",
        "created",
        "added",
        "removed",
        "merged",
        "wired",
        "implemented",
        "patched",
        "refactored",
        "wrote",
        "built",
        "deployed",
        "committed",
        "corrigi",
        "instalei",
        "criei",
        "atualizei",
        "implementei",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn is_process_language(text: &str) -> bool {
    let lower = text.to_lowercase();
    const PREFIXES: &[&str] = &[
        "i'll ",
        "i will ",
        "let me ",
        "i'm going ",
        "i am going ",
        "vou ",
        "deixe-me ",
        "agora vou ",
        "next i ",
        "first i ",
        "going to ",
    ];
    PREFIXES.iter().any(|p| lower.starts_with(p))
        || lower.contains("vou fazer")
        || lower.contains("i'll create")
        || lower.contains("let me check")
}

/// Short topic from user prompt (no full dump).
fn snippet_topic(text: &str, max: usize) -> String {
    let t = collapse_ws(text);
    if t.is_empty() {
        return String::new();
    }
    // Drop common command-ish noise.
    let t = t.trim_start_matches(['/', '!', '#']).trim().to_string();
    truncate_at_word(&t, max)
}

/// Prefer a short conclusive line from assistant; never a long plan dump.
fn snippet_outcome(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Prefer last short non-heading paragraph (often the conclusion).
    let paragraphs: Vec<&str> = trimmed
        .split(|c| c == '\n')
        .map(str::trim)
        .filter(|p| {
            !p.is_empty()
                && !p.starts_with('#')
                && !p.starts_with("```")
                && !p.starts_with('|')
                && !p.starts_with('-')
                && !p.starts_with('*')
        })
        .collect();

    let candidate = paragraphs
        .iter()
        .rev()
        .find(|p| {
            let n = collapse_ws(p).chars().count();
            (12..max.saturating_add(40)).contains(&n) && !is_process_language(&collapse_ws(p))
        })
        .or_else(|| paragraphs.first())
        .copied()
        .unwrap_or(trimmed);

    let sentence = first_sentence(&collapse_ws(candidate));
    truncate_at_word(&sentence, max)
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let hard = 140usize;
    let punct = trimmed.find(['.', '!', '?', '。', '！', '？']);
    let end = match punct {
        Some(i) if i + 1 <= hard => i + 1,
        Some(_) | None => trimmed
            .char_indices()
            .nth(hard)
            .map(|(i, _)| i)
            .unwrap_or(trimmed.len()),
    };
    collapse_ws(&trimmed[..end.min(trimmed.len())])
}

fn finalize_recap(body: &str) -> String {
    let mut text = collapse_ws(body);
    // Soft word cap, then hard char cap.
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > RECAP_TARGET_WORDS {
        text = words[..RECAP_TARGET_WORDS].join(" ");
        if !text.ends_with(['.', '!', '?', '…']) {
            text.push('…');
        }
    }
    truncate_chars(&text, RECAP_MAX_DISPLAY_CHARS)
}

fn truncate_at_word(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    if let Some(pos) = out.rfind(char::is_whitespace) {
        out.truncate(pos);
    }
    out.push('…');
    out
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
    fn local_recap_you_asked_for_questions() {
        let recap = local_recap(
            "how do retries work in the payment client?",
            "Retries use exponential backoff in billing/retry.rs, max 5 attempts.",
        );
        assert!(
            recap.starts_with("You asked") || recap.contains("retries"),
            "unexpected recap: {recap}"
        );
        assert!(recap.chars().count() <= RECAP_MAX_DISPLAY_CHARS);
        assert!(recap.split_whitespace().count() <= RECAP_TARGET_WORDS + 2);
    }

    #[test]
    fn local_recap_stays_short_on_long_plan() {
        let long = "Sem pygame/tkinter no ambiente — vou fazer um simulador gráfico completo no navegador (HTML+Canvas), auto-contido e sem dependências, com vários módulos e arquivos e muita explicação detalhada sobre arquitetura e performance e testes e documentação e mais texto que não deveria aparecer inteiro no recap da UI.";
        let recap = local_recap("instala pygame e faz o jogo", long);
        assert!(
            recap.chars().count() <= RECAP_MAX_DISPLAY_CHARS,
            "recap too long: {} chars — {recap:?}",
            recap.chars().count()
        );
        // Must not dump the whole plan paragraph.
        assert!(
            !recap.contains("muita explicação detalhada"),
            "recap still dumps plan: {recap}"
        );
    }

    #[test]
    fn local_recap_we_framing_when_tools_used() {
        let tools = vec!["bash".into(), "write".into()];
        let recap = local_recap_with_tools(
            "add a footer token meter",
            "Updated the footer meter and wired usage labels.",
            &tools,
        );
        assert!(
            recap.starts_with("We ") || recap.contains("footer") || recap.contains("meter"),
            "unexpected: {recap}"
        );
        assert!(recap.chars().count() <= RECAP_MAX_DISPLAY_CHARS);
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

    #[test]
    fn clean_strips_recap_prefix() {
        let cleaned = clean_recap_text("Recap: We fixed the footer meter.");
        assert!(cleaned.starts_with("We fixed"));
    }
}
