use ratatui::prelude::{Span, Style};

use crate::theme::*;

pub(crate) struct CodeHighlighter {
    language: String,
}

impl CodeHighlighter {
    pub(crate) fn new(language: &str) -> Self {
        Self {
            language: language.trim().to_ascii_lowercase(),
        }
    }

    pub(crate) fn highlight_line(&mut self, raw_line: &str) -> Vec<Span<'static>> {
        highlight_code_line(raw_line, &self.language)
    }
}

pub(crate) fn highlight_code_line(raw_line: &str, language: &str) -> Vec<Span<'static>> {
    if raw_line.is_empty() {
        return vec![Span::styled(String::new(), code_style(text()))];
    }

    let comment_marker = comment_marker(language);
    if let Some(index) = raw_line.find(comment_marker) {
        let mut spans = highlight_code_without_comments(&raw_line[..index]);
        spans.push(Span::styled(
            raw_line[index..].to_string(),
            code_style(code_comment()),
        ));
        return spans;
    }

    highlight_code_without_comments(raw_line)
}

fn highlight_code_without_comments(raw_line: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut token = String::new();
    let mut chars = raw_line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' || ch == '\'' || ch == '`' {
            flush_token(&mut spans, &mut token);
            let quote = ch;
            let mut string = String::from(ch);
            let mut escaped = false;
            for next in chars.by_ref() {
                string.push(next);
                if escaped {
                    escaped = false;
                    continue;
                }
                if next == '\\' {
                    escaped = true;
                    continue;
                }
                if next == quote {
                    break;
                }
            }
            spans.push(Span::styled(string, code_style(code_string())));
        } else if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            flush_token(&mut spans, &mut token);
            let color = if ch.is_ascii_punctuation() {
                code_punct()
            } else {
                text()
            };
            spans.push(Span::styled(ch.to_string(), code_style(color)));
        }
    }

    flush_token(&mut spans, &mut token);
    spans
}

fn flush_token(spans: &mut Vec<Span<'static>>, token: &mut String) {
    if token.is_empty() {
        return;
    }

    let color = if is_keyword(token) {
        code_keyword()
    } else if token.chars().all(|ch| ch.is_ascii_digit()) {
        code_number()
    } else if token
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        code_type()
    } else {
        text()
    };
    spans.push(Span::styled(std::mem::take(token), code_style(color)));
}

fn code_style(color: ratatui::style::Color) -> Style {
    // Foreground syntax color only — no panel background fill.
    Style::default().fg(color)
}

fn comment_marker(language: &str) -> &'static str {
    match language {
        "bash" | "sh" | "shell" | "python" | "py" | "toml" | "yaml" | "yml" => "#",
        _ => "//",
    }
}

fn is_keyword(token: &str) -> bool {
    matches!(
        token,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "else"
            | "enum"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "mut"
            | "pub"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "true"
            | "type"
            | "use"
            | "while"
    )
}
