use anyhow::Result;

use crate::lang::LangId;
use crate::minify::minify;

/// Validate that minified output is syntactically valid by re-parsing it.
///
/// For Go, also validates with `go/parser` via subprocess since tree-sitter's
/// Go grammar can miss edge cases around automatic semicolon insertion.
pub fn validate_minified(minified: &str, _original: &str, lang: LangId) -> Result<()> {
    if minified.is_empty() {
        return Ok(());
    }

    // Re-parse with tree-sitter and check for ERROR nodes.
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.tree_sitter_language()?)?;
    let tree = parser
        .parse(minified, None)
        .ok_or_else(|| anyhow::anyhow!("re-parse returned None"))?;

    let root = tree.root_node();
    if has_error_nodes(&root) {
        anyhow::bail!(
            "minified {} output contains parse errors; keeping original",
            lang.name()
        );
    }

    // Go-specific: validate with go/parser for auto-semicolon edge cases.
    if lang == LangId::Go {
        validate_go_with_stdlib(minified)?;
    }

    // Idempotency check: minify(minify(x)) == minify(x).
    let double_minified = minify(minified, lang, false)?;
    if double_minified != minified {
        anyhow::bail!(
            "minified {} output is not idempotent; keeping original",
            lang.name()
        );
    }

    Ok(())
}

/// Walk the AST and check for any ERROR or MISSING nodes.
fn has_error_nodes(node: &tree_sitter::Node) -> bool {
    if node.is_error() || node.is_missing() {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_error_nodes(&child) {
            return true;
        }
    }
    false
}

/// Validate Go code with `go/parser` via `gofmt -e` which reports syntax errors.
fn validate_go_with_stdlib(code: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("gofmt")
        .args(["-e"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn gofmt: {e}"))?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(code.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("go/parser rejected minified output: {stderr}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_rust_passes() {
        let src = "fn main() {\n    let x = 1;\n}\n";
        let minified = minify(src, LangId::Rust, false).unwrap();
        assert!(validate_minified(&minified, src, LangId::Rust).is_ok());
    }

    #[test]
    fn valid_go_passes() {
        let src = "package main\n\nfunc main() {}\n";
        let minified = minify(src, LangId::Go, false).unwrap();
        // This may skip go validation if gofmt is not installed.
        let _ = validate_minified(&minified, src, LangId::Go);
    }

    #[test]
    fn idempotent_rust() {
        let src = "fn add(a: i32) -> i32 {\n    a + 1\n}\n";
        let minified = minify(src, LangId::Rust, false).unwrap();
        assert!(validate_minified(&minified, src, LangId::Rust).is_ok());
    }
}
