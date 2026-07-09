pub(crate) mod layout;
pub(crate) mod markdown;
pub(crate) mod status;
pub(crate) mod syntax;
pub(crate) mod text;
pub(crate) mod tool;
pub(crate) mod tool_policy;

pub(crate) use layout::{
    clear_modal_area, command_row, command_scroll_offset, fill_modal_scrim, fill_modal_surface,
    modal_block, modal_list_highlight_style, modal_rect, opaque_fill, truncate_display,
};
pub(crate) use text::{mask_key_segment, project_label};

#[cfg(test)]
mod tests {
    use ratatui::prelude::{Line, Modifier};

    use navi_sdk::{ToolInvocation, ToolResult};

    use super::*;
    use crate::render::markdown::render_markdown_lines;
    use crate::render::syntax::highlight_code_line;
    use crate::render::text::{display_width, wrap_spans_to_width, wrap_text};
    use crate::render::tool::{tool_compact_text, tool_full_content};
    use crate::theme::ThemeId;
    use crate::theme::code_block_bg;

    fn test_palette() -> crate::theme::ThemePalette {
        ThemeId::Lain.palette()
    }

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wrap_text_handles_long_lines() {
        let text = "Hello world this is a very long line that should wrap properly";
        let lines = wrap_text(text, 20);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.chars().count() <= 20);
        }
    }

    #[test]
    fn wrap_text_preserves_newlines() {
        let text = "Line one\nLine two\nLine three";
        let lines = wrap_text(text, 50);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Line one");
        assert_eq!(lines[1], "Line two");
        assert_eq!(lines[2], "Line three");
    }

    #[test]
    fn markdown_renderer_wraps_plain_text() {
        // CONTENT_GUTTER (2) is reserved; wrap budget is width - 2.
        let lines = render_markdown_lines(
            "hello world from navi",
            14,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["  hello world", "  from navi"]);
    }

    #[test]
    fn markdown_renderer_renders_fenced_code_as_panel() {
        let lines = render_markdown_lines(
            "before\n```rust\nfn main() {}\n```\nafter",
            80,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "  before",
                "",
                "  │  ",
                "  │  fn main() {}",
                "  │  ",
                "",
                "  after"
            ]
        );
    }

    #[test]
    fn markdown_renderer_wraps_long_code_lines() {
        let long_line = format!("```rust\n\"message\": \"{}\"\n```", "x".repeat(120));
        let width = 40;
        let lines = render_markdown_lines(
            &long_line,
            width,
            test_palette().text,
            test_palette().text,
            false,
        );
        let code_lines: Vec<_> = lines
            .iter()
            .filter(|line| {
                line.spans
                    .iter()
                    .any(|span| span.style.bg == Some(code_block_bg()))
            })
            .collect();
        assert!(code_lines.len() > 1, "expected wrapped code lines");
        for line in &code_lines {
            let used: usize = line
                .spans
                .iter()
                .map(|span| display_width(&span.content))
                .sum();
            assert!(
                used <= width,
                "code line wider than viewport: {used} > {width}"
            );
        }
    }

    #[test]
    fn wrap_spans_to_width_splits_highlighted_spans() {
        let spans = highlight_code_line("\"abcdefghijklmnop\"", "rust");
        let wrapped = wrap_spans_to_width(&spans, 8);
        let rendered: Vec<String> = wrapped
            .iter()
            .map(|line| line.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        assert_eq!(rendered.len(), 3);
        for line in rendered {
            assert!(line.chars().count() <= 8);
        }
    }

    #[test]
    fn markdown_renderer_handles_unclosed_fence() {
        let lines = render_markdown_lines(
            "```unknown\n  value",
            80,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["  │  ", "  │    value"]);
    }

    #[test]
    fn markdown_renderer_renders_inline_markup() {
        let lines = render_markdown_lines(
            "**NAVI** is `wired` and [documented](https://example.test)",
            120,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["  NAVI is wired and documented (https://example.test)"]
        );
        // First span is the content gutter; bold is on the NAVI span.
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.content.as_ref() == "NAVI"
                    && span.style.add_modifier.contains(Modifier::BOLD))
        );
    }

    #[test]
    fn markdown_renderer_handles_nested_and_extended_inline_markup() {
        let lines = render_markdown_lines(
            "**`NAVI`** uses ***strong emphasis***, ~~old text~~, ![diagram](file.png), and \\*literal\\*.",
            160,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "  NAVI uses strong emphasis, old text, diagram (image: file.png), and *literal*."
            ]
        );
        let navi = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "NAVI")
            .expect("nested code span");
        assert_eq!(navi.style.fg, Some(test_palette().code_string));
        assert!(navi.style.add_modifier.contains(Modifier::BOLD));

        let strong_emphasis = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "strong emphasis")
            .expect("strong emphasis span");
        assert!(strong_emphasis.style.add_modifier.contains(Modifier::BOLD));
        assert!(
            strong_emphasis
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );

        let old = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "old text")
            .expect("strikethrough span");
        assert!(old.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn markdown_renderer_handles_lists_and_quotes() {
        let lines = render_markdown_lines(
            "1. **Architecture**\n> signal in prose",
            120,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["  1. Architecture", "", "  ◇ signal in prose"]
        );
    }

    #[test]
    fn markdown_renderer_consumes_headings_and_table_pipes() {
        let lines = render_markdown_lines(
            "## Project Overview\n\n| Crate | Purpose |\n|---|---|\n| `navi-cli` | Entry binary |",
            120,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        // full box frame (outer ┌┐└┘, header ┼, body │).
        assert_eq!(rendered[0], "  ◆ Project Overview");
        assert!(rendered[1].trim().is_empty());
        let table = &rendered[2..];
        assert!(
            table.iter().any(|l| l.contains('┌') && l.contains('┐')),
            "top border: {table:?}"
        );
        assert!(
            table.iter().any(|l| l.contains('├') && l.contains('┤')),
            "header rule: {table:?}"
        );
        assert!(
            table.iter().any(|l| l.contains('└') && l.contains('┘')),
            "bottom border: {table:?}"
        );
        assert!(
            table
                .iter()
                .any(|l| l.contains("Crate") && l.contains("Purpose")),
            "header row: {table:?}"
        );
        assert!(
            table
                .iter()
                .any(|l| l.contains("navi-cli") && l.contains("Entry binary")),
            "body row: {table:?}"
        );
        assert!(!rendered.iter().any(|line| line.contains("##")));
        // Markdown pipe characters are consumed; box-drawing uses │ not |.
        assert!(!rendered.iter().skip(2).any(|line| line.contains('|')));
        // Gutter is spaces (block pad), never a bare quote-bar at column 0.
        assert!(table[0].starts_with("  "));
        assert!(!table[0].starts_with('│'));
    }

    #[test]
    fn markdown_renderer_stacks_wide_tables() {
        let lines = render_markdown_lines(
            "| Problema | Onde | Gravidade |\n|---|---|---|\n| OAuth Device Flow na TUI | navi-tui/src/runtime.rs contém HTTP calls, polling loop e JSON parsing | CRÍTICO |\n| Flat module tree | lib.rs re-exporta tudo num namespace plano | ALTO |",
            64,
            test_palette().text,
            test_palette().text,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        // Join without newlines so wrapped cells still match as contiguous substrings.
        let flat: String = rendered.iter().map(String::as_str).collect();
        assert!(flat.contains("Problema"), "got: {flat}");
        assert!(
            flat.contains("OAuth Device Flow na TUI"),
            "expected OAuth cell content, got: {flat}"
        );
        assert!(flat.contains("Onde"), "got: {flat}");
        assert!(flat.contains("navi-tui/src/runtime.rs"), "got: {flat}");
        assert!(flat.contains("Gravidade"), "got: {flat}");
        assert!(flat.contains("CRÍTICO"), "got: {flat}");
        // No raw markdown pipes left.
        assert!(rendered.iter().all(|line| !line.contains('|')));
        for line in rendered.iter().filter(|line| !line.is_empty()) {
            assert!(
                display_width(line) <= 64,
                "line too wide ({}): {line}",
                display_width(line)
            );
        }
    }

    #[test]
    fn code_highlighting_uses_varied_colors() {
        let spans = highlight_code_line("fn main() { let value = \"x\"; }", "rust");
        let mut colors = Vec::new();
        for color in spans.iter().filter_map(|span| span.style.fg) {
            if !colors.contains(&color) {
                colors.push(color);
            }
        }

        assert!(colors.len() >= 3);
    }

    #[test]
    fn tool_compact_text_is_one_line_with_status() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "grep".to_string(),
            input: serde_json::json!({ "pattern": "NAVI" }),
        };
        let ok_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({ "matches": [] }),
        };
        let err_result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: false,
            output: serde_json::json!({ "error": "denied" }),
        };

        assert_eq!(
            tool_compact_text(&invocation, &ok_result),
            "Search \"NAVI\" in . (0 matches)"
        );
        assert_eq!(
            tool_compact_text(&invocation, &err_result),
            "Search \"NAVI\" in . (0 matches) · error: denied"
        );
        assert!(!tool_compact_text(&invocation, &ok_result).contains('\n'));
    }

    #[test]
    fn read_file_summary_shows_relative_path_line_range_and_read_count() {
        let path = std::env::current_dir()
            .unwrap()
            .join("Cargo.toml")
            .to_string_lossy()
            .to_string();
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": path }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": invocation.input["path"],
                "content": "a\nb\nc\n",
                "start_line": 2,
                "end_line": 4,
                "total_lines": 10,
                "truncated": true,
            }),
        };

        assert_eq!(
            tool_compact_text(&invocation, &result),
            "Read Cargo.toml (lines 2-4 of 10, 3 lines read)"
        );
        assert!(
            tool_full_content(&invocation, &result)
                .contains("View Cargo.toml (lines 2-4 of 10, 3 lines read)")
        );
    }

    #[test]
    fn tool_full_content_sanitizes_read_file_without_json_io() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "Cargo.toml" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "Cargo.toml",
                "content": "[workspace]\n",
                "truncated": false,
            }),
        };

        let content = tool_full_content(&invocation, &result);
        assert!(content.contains("Read Cargo.toml"));
        assert!(content.contains("View Cargo.toml"));
        assert!(content.contains("[workspace]"));
        assert!(!content.contains("Input"));
        assert!(!content.contains("Output"));
        assert!(!content.contains("\"path\""));
    }

    #[test]
    fn read_file_tool_full_content_uses_fenced_code_for_highlighting() {
        let invocation = ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({ "path": "src/lib.rs" }),
        };
        let result = ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: serde_json::json!({
                "path": "src/lib.rs",
                "content": "fn main() {}\n",
            }),
        };

        let content = tool_full_content(&invocation, &result);

        assert!(content.contains("```rust"));
        assert!(content.contains("fn main() {}"));
    }

    #[test]
    fn mask_key_hides_middle_characters() {
        let short = "sk-abc";
        assert_eq!(mask_key_segment(short), "sk-abc");

        let long = "sk-proj-abcdefghijklmnop";
        let masked = mask_key_segment(long);
        assert!(masked.starts_with("sk-pro"));
        assert!(masked.ends_with("mnop"));
        assert!(masked.contains('•'));
    }
}
