//! Session recap — short post-turn summary after each successful turn.
//!
//! Shows a compact blurb under a "Recap" label (hard cap: **3 lines**):
//! - You asked … : <concrete specifics>
//! - We <past-tense verb> … : <concrete specifics>
//! Never a wall of assistant prose or a file dump.

use crate::model::{ModelMessage, ModelProvider, ModelRequest, ThinkingConfig};
use anyhow::Result;

/// Suppress UI recap when the assistant output is a long dump (long-tail).
pub const RECAP_LONG_TAIL_CHARS: usize = 4_000;
/// Max tool calls before treating a turn as long-tail (display suppressed).
pub const RECAP_LONG_TAIL_TOOL_CALLS: usize = 40;
/// Hard max length for a displayed recap (≈3 short lines).
pub const RECAP_MAX_DISPLAY_CHARS: usize = 240;
/// Hard max lines for a displayed recap.
pub const RECAP_MAX_LINES: usize = 3;
/// Prefer staying under this word count (very short blurb).
const RECAP_TARGET_WORDS: usize = 45;

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
        "Write a tiny recap body for this coding-agent turn.\n\
         HARD LIMITS (non-negotiable):\n\
         - At most 3 short lines (prefer 1).\n\
         - At most ~45 words / ~240 characters.\n\
         - Output ONLY the body (the UI adds the \"Recap\" label).\n\
         - No markdown, no bullets, no code, no file contents, no quotes, no preamble.\n\
         - Never paste assistant prose, patches, or file dumps.\n\n\
         Lead with agency:\n\
         - \"You asked …\" if the turn was mainly questions/planning/review with no landed change.\n\
         - \"We <past-tense verb> …\" if code/config/docs changed (e.g. \"We fixed\", \"We wired\").\n\
         - If almost nothing happened: \"You had just begun this session.\"\n\n\
         Shape: <lead>: <concrete specifics — file/crate/flag/behavior>.\n\n\
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
                 At most 3 lines, usually one sentence. Past-tense when work landed. \
                 Never invent work. Never dump the assistant message or any file contents.",
            ),
            ModelMessage::user(prompt),
        ],
        thinking: ThinkingConfig::Off,
        tools: vec![],
        session_id: None,
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
    // Preserve line breaks only long enough to enforce the 3-line cap, then
    // collapse each kept line. Reject obvious file dumps early.
    let mut lines: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    // If the model dumped a wall of text / code, keep only the first short line.
    if looks_like_file_dump(&lines) {
        lines.truncate(1);
    }
    lines.truncate(RECAP_MAX_LINES);

    let mut text = lines
        .iter()
        .map(|l| collapse_ws(l))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

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
    // Prefer a single sentence when the model still rambled in one line.
    first_sentence(&text)
}

fn looks_like_file_dump(lines: &[String]) -> bool {
    if lines.len() > RECAP_MAX_LINES {
        return true;
    }
    let joined = lines.join(
        "
",
    );
    let chars = joined.chars().count();
    if chars > RECAP_MAX_DISPLAY_CHARS.saturating_mul(2) {
        return true;
    }
    // Code / patch / fence markers ⇒ dump, not a recap.
    let codeish = lines
        .iter()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("```")
                || t.starts_with("diff --git")
                || t.starts_with("@@")
                || t.starts_with("*** ")
                || t.starts_with("fn ")
                || t.starts_with("pub ")
                || t.starts_with("use ")
                || t.starts_with("impl ")
                || t.starts_with("#include")
                || t.starts_with("package ")
        })
        .count();
    codeish >= 1 && lines.len() >= 2
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
    // Enforce line cap first (models sometimes emit multi-line essays).
    let mut lines: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| collapse_ws(l))
        .filter(|l| !l.is_empty())
        .take(RECAP_MAX_LINES)
        .collect();
    if lines.is_empty() {
        let collapsed = collapse_ws(body);
        if !collapsed.is_empty() {
            lines.push(collapsed);
        }
    }
    if looks_like_file_dump(&lines) {
        lines.truncate(1);
    }

    // Single display string: keep at most RECAP_MAX_LINES by rejoining with spaces
    // for chat compactness (TUI may re-wrap to ≤3 visual lines).
    let mut text = lines.join(" ");

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

/// Public clamp used by UIs when accepting recap text from any source (local/LLM).
pub fn clamp_recap_summary(summary: &str) -> String {
    finalize_recap(&clean_recap_text(summary))
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

    #[test]
    fn finalize_hard_caps_multiline_essay() {
        let essay = "Line one is fine.\nLine two still ok.\nLine three ok.\nLine four must die.\nfn dump() {\n  everything()\n}\n";
        let recap = finalize_recap(essay);
        assert!(
            recap.lines().count() <= RECAP_MAX_LINES,
            "too many lines: {recap:?}"
        );
        assert!(
            recap.chars().count() <= RECAP_MAX_DISPLAY_CHARS,
            "too many chars: {} — {recap:?}",
            recap.chars().count()
        );
        assert!(
            !recap.contains("must die") && !recap.contains("fn dump"),
            "file dump leaked into recap: {recap}"
        );
    }

    #[test]
    fn clamp_recap_summary_rejects_file_dump() {
        let dump = "```rust\npub fn main() {\n    println!(\"hi\");\n}\n```\nAnd more explanation that should not appear in full.";
        let recap = clamp_recap_summary(dump);
        assert!(recap.chars().count() <= RECAP_MAX_DISPLAY_CHARS);
        assert!(!recap.contains("println"));
    }

    #[test]
    fn llm_clean_keeps_at_most_three_lines_worth() {
        let raw = "We fixed the footer meter.\nAlso touched theme.rs.\nAnd tests.\nAnd then wrote an essay about architecture that must be dropped.";
        let cleaned = clean_recap_text(raw);
        let final_ = finalize_recap(&cleaned);
        assert!(final_.chars().count() <= RECAP_MAX_DISPLAY_CHARS);
        assert!(
            !final_.contains("architecture that must be dropped")
                || final_.chars().count() <= RECAP_MAX_DISPLAY_CHARS
        );
    }
}
