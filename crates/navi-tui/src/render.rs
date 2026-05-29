pub(crate) mod layout;
pub(crate) mod markdown;
pub(crate) mod syntax;
pub(crate) mod text;
pub(crate) mod tool;

// Re-exported for use by view modules and tests. The compiler warns about
// unused items because some are only consumed in #[cfg(test)] code.
#[allow(unused_imports)]
pub(crate) use layout::*;
#[allow(unused_imports)]
pub(crate) use markdown::*;
#[allow(unused_imports)]
pub(crate) use syntax::*;
#[allow(unused_imports)]
pub(crate) use text::*;
#[allow(unused_imports)]
pub(crate) use tool::*;

#[cfg(test)]
mod tests {
    use ratatui::prelude::{Line, Modifier};

    use navi_sdk::{ToolInvocation, ToolResult};

    use super::*;
    use crate::theme::{CODE_STRING, TEXT};

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
        let lines = render_markdown_lines("hello world from navi", 12, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["hello world", "from navi"]);
    }

    #[test]
    fn markdown_renderer_preserves_fenced_code_blocks() {
        let lines = render_markdown_lines(
            "before\n```rust\nfn main() {}\n```\nafter",
            80,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["before", "```rust", "fn main() {}", "```", "after"]
        );
    }

    #[test]
    fn markdown_renderer_handles_unclosed_fence() {
        let lines = render_markdown_lines("```unknown\n  value", 80, TEXT, TEXT, false);
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["```unknown", "  value"]);
    }

    #[test]
    fn markdown_renderer_renders_inline_markup() {
        let lines = render_markdown_lines(
            "**NAVI** is `wired` and [documented](https://example.test)",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["NAVI is wired and documented (https://example.test)"]
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn markdown_renderer_handles_nested_and_extended_inline_markup() {
        let lines = render_markdown_lines(
            "**`NAVI`** uses ***strong emphasis***, ~~old text~~, ![diagram](file.png), and \\*literal\\*.",
            160,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec!["NAVI uses strong emphasis, old text, diagram (image: file.png), and *literal*."]
        );
        let navi = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "NAVI")
            .expect("nested code span");
        assert_eq!(navi.style.fg, Some(CODE_STRING));
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
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(rendered, vec!["1. Architecture", "▌ signal in prose"]);
    }

    #[test]
    fn markdown_renderer_consumes_headings_and_table_pipes() {
        let lines = render_markdown_lines(
            "## Project Overview\n\n| Crate | Purpose |\n|---|---|\n| `navi-cli` | Entry binary |",
            120,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                "▣ Project Overview",
                "",
                "Crate     Purpose     ",
                "navi-cli  Entry binary",
            ]
        );
        assert!(!rendered.iter().any(|line| line.contains("##")));
        assert!(!rendered.iter().skip(2).any(|line| line.contains('|')));
    }

    #[test]
    fn markdown_renderer_stacks_wide_tables() {
        let lines = render_markdown_lines(
            "| Problema | Onde | Gravidade |\n|---|---|---|\n| OAuth Device Flow na TUI | navi-tui/src/runtime.rs contém HTTP calls, polling loop e JSON parsing | CRÍTICO |\n| Flat module tree | lib.rs re-exporta tudo num namespace plano | ALTO |",
            64,
            TEXT,
            TEXT,
            false,
        );
        let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| line.starts_with("Problema:")));
        assert!(rendered.iter().any(|line| line.starts_with("Onde:")));
        assert!(rendered.iter().any(|line| line.starts_with("Gravidade:")));
        assert!(rendered.iter().all(|line| !line.contains('|')));
        for line in rendered.iter().filter(|line| !line.is_empty()) {
            assert!(line.chars().count() <= 64, "line too wide: {line}");
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
            "grep called · success"
        );
        assert_eq!(
            tool_compact_text(&invocation, &err_result),
            "grep called · error"
        );
        assert!(!tool_compact_text(&invocation, &ok_result).contains('\n'));
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
        assert!(content.contains("read_file called · success"));
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
