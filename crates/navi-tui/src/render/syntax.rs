use ratatui::prelude::{Span, Style};
use ratatui::style::Color;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::theme::*;

pub(crate) fn highlight_code_line(raw_line: &str, language: &str) -> Vec<Span<'static>> {
    let syntax_set = syntax_set();
    let syntax = syntax_set
        .find_syntax_by_token(language)
        .or_else(|| syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, syntax_theme());

    match highlighter.highlight_line(raw_line, syntax_set) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_style(style)))
            .collect(),
        Err(_) => vec![Span::styled(
            raw_line.to_string(),
            Style::default().fg(TEXT),
        )],
    }
}

pub(crate) fn syntect_style(style: SyntectStyle) -> Style {
    Style::default().fg(lain_code_color(style))
}

pub(crate) fn lain_code_color(style: SyntectStyle) -> Color {
    let color = style.foreground;
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
        || (color.r < 118 && color.g < 118 && color.b < 118)
    {
        CODE_COMMENT
    } else if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        CODE_FUNC
    } else if color.r > 190 && color.b > 165 && color.g < 170 {
        CODE_KEYWORD
    } else if color.g > color.r.saturating_add(25) && color.g > color.b.saturating_add(5) {
        Color::Rgb(143, 232, 173)
    } else if color.b > color.r.saturating_add(25) && color.g > color.r.saturating_add(10) {
        CODE_TYPE
    } else if color.b > color.r.saturating_add(25) {
        CODE_NUMBER
    } else if color.r > 175 && color.g > 145 && color.b < 145 {
        CODE_CONST
    } else if color.r > 180 && color.b > 95 && color.g < 135 {
        CODE_OPERATOR
    } else if color.r < 175 && color.g < 175 && color.b < 175 {
        CODE_PUNCT
    } else if color.r > 200 && color.g > 200 && color.b > 200 {
        TEXT
    } else {
        Color::Rgb(
            boost_code_channel(color.r),
            boost_code_channel(color.g),
            boost_code_channel(color.b),
        )
    }
}

pub(crate) fn boost_code_channel(value: u8) -> u8 {
    value.max(96).saturating_add(22)
}

pub(crate) fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub(crate) fn syntax_theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default()
    })
}
