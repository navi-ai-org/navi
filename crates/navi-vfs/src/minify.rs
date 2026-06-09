use anyhow::Result;
use tree_sitter::Parser;

use crate::lang::LangId;

/// Minify source code by walking the tree-sitter AST and collecting only
/// essential tokens. Returns the original source unchanged if the parse
/// produces errors.
pub fn minify(source: &str, lang: LangId, keep_comments: bool) -> Result<String> {
    let tree = parse(source, lang)?;
    let root = tree.root_node();

    if root.has_error() {
        tracing::warn!(lang = %lang.name(), "source has parse errors; returning original");
        return Ok(source.to_string());
    }

    let mut walker = Minifier::new(source.as_bytes(), lang, keep_comments);
    walker.walk_node(&root);
    Ok(walker.into_output())
}

fn parse(source: &str, lang: LangId) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.tree_sitter_language())?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter returned None"))
}

struct Minifier<'a> {
    src: &'a [u8],
    lang: LangId,
    keep_comments: bool,
    out: Vec<u8>,
    last_end: usize,
}

impl<'a> Minifier<'a> {
    fn new(src: &'a [u8], lang: LangId, keep_comments: bool) -> Self {
        Self {
            src,
            lang,
            keep_comments,
            out: Vec::with_capacity(src.len()),
            last_end: 0,
        }
    }

    fn into_output(self) -> String {
        // SAFETY: tree-sitter preserves valid UTF-8 boundaries.
        unsafe { String::from_utf8_unchecked(self.out) }
    }

    fn walk_node(&mut self, node: &tree_sitter::Node) {
        // C/C++ preprocessor directives: emit as one unit with trailing newline.
        if matches!(self.lang, LangId::C | LangId::Cpp) && is_preproc_node(node.kind()) {
            let start = node.start_byte();
            let end = node.end_byte();
            self.emit_separator(start, &self.src[start..end]);
            let content = &self.src[start..end];
            let end_no_nl = content
                .iter()
                .rposition(|&b| b != b'\n' && b != b'\r')
                .map(|p| p + 1)
                .unwrap_or(0);
            self.out.extend_from_slice(&content[..end_no_nl]);
            self.out.push(b'\n');
            self.last_end = end;
            return;
        }

        // Comment nodes: handle at this level to emit/skip entire comment as one unit.
        if is_comment(node.kind()) {
            let start = node.start_byte();
            let end = node.end_byte();
            if self.keep_comments {
                self.emit_separator(start, &self.src[start..end]);
                self.out.extend_from_slice(&self.src[start..end]);
                // Line/doc comments need a trailing newline to avoid merging.
                let kind = node.kind();
                if (kind == "line_comment" || kind == "doc_comment") && !self.out.ends_with(b"\n") {
                    self.out.push(b'\n');
                }
                self.last_end = end;
            }
            // When skipping, don't update last_end — preserve the gap
            // (including any newline) for the next token's separator logic.
            return;
        }

        if node.child_count() == 0 {
            self.emit_leaf(node);
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                self.walk_node(&child);
            }
        }
    }

    fn emit_leaf(&mut self, node: &tree_sitter::Node) {
        let start = node.start_byte();
        let end = node.end_byte();
        let kind = node.kind();

        // Skip whitespace-only nodes.
        if kind == "\n" || kind == "\r" || kind == "\r\n" || kind == " " || kind == "\t" {
            self.last_end = end;
            return;
        }

        self.emit_separator(start, &self.src[start..end]);
        self.out.extend_from_slice(&self.src[start..end]);
        self.last_end = end;
    }

    /// Emit the correct separator between the last emitted token and the next one.
    fn emit_separator(&mut self, next_start: usize, next_token: &[u8]) {
        if next_start <= self.last_end {
            return;
        }

        let has_gap = self.last_end > 0 && next_start > self.last_end;

        // Language-specific: newline-based statement separators.
        if has_gap {
            let gap = &self.src[self.last_end..next_start];
            if gap.contains(&b'\n') {
                match self.lang {
                    // Go: auto-semicolon insertion after newline.
                    LangId::Go => {
                        self.emit_go_semicolon();
                        return;
                    }
                    // Ruby: semicolon after certain tokens when newline follows.
                    // Ruby is less strict than Go — semicolons are optional in most
                    // contexts, but we add them to ensure correctness.
                    LangId::Ruby => {
                        self.emit_ruby_semicolon();
                        return;
                    }
                    // Bash: newlines are statement separators. Emit a `;` to
                    // preserve statement boundaries.
                    LangId::Bash => {
                        self.out.push(b';');
                        return;
                    }
                    // PHP: newlines are NOT separators (semicolons are).
                    // Fall through to normal spacing.
                    // HTML/CSS/JSON: whitespace is insignificant, fall through.
                    _ => {}
                }
            }
        }

        if self.out.is_empty() {
            return;
        }

        if self.needs_space_between(next_token) {
            self.out.push(b' ');
        }
    }

    /// Determine if a space is needed between the last emitted character and
    /// the first character of the next token.
    fn needs_space_between(&self, next_token: &[u8]) -> bool {
        if self.out.is_empty() || next_token.is_empty() {
            return false;
        }

        let last = self.out[self.out.len() - 1];
        let next = next_token[0];

        // No space after open brackets.
        if matches!(last, b'(' | b'[' | b'{') {
            return false;
        }

        // No space before close brackets, commas, semicolons, colons.
        if matches!(next, b')' | b']' | b'}' | b',' | b';' | b':') {
            return false;
        }

        // Space between two identifier/keyword-like tokens (they'd merge).
        if is_ident_char(last) && is_ident_char(next) {
            return true;
        }

        // No space between ident and most operators.
        if is_ident_char(last) && matches!(next, b'+' | b'-' | b'*' | b'/' | b'%') {
            return false;
        }

        // No space after ident before `.` for member access.
        if is_ident_char(last) && next == b'.' {
            return false;
        }

        // No space before ident after most operators/punctuation.
        if !is_ident_char(last) && is_ident_char(next) {
            return false;
        }

        false
    }

    fn emit_go_semicolon(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let last = self.out[self.out.len() - 1];
        if go_auto_semi_after(last) {
            self.out.push(b';');
        }
    }

    fn emit_ruby_semicolon(&mut self) {
        if self.out.is_empty() {
            return;
        }
        let last = self.out[self.out.len() - 1];
        // Ruby auto-semicolon: after identifiers, literals, closing brackets,
        // and certain keywords.
        if ruby_auto_semi_after(last) {
            self.out.push(b';');
        }
    }
}

/// Check if a byte looks like it could be part of an identifier or literal.
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Go automatic semicolon insertion: tokens that trigger `;` when followed by newline.
fn go_auto_semi_after(last_byte: u8) -> bool {
    matches!(
        last_byte,
        b'a'..=b'z'
        | b'A'..=b'Z'
        | b'0'..=b'9'
        | b'_'
        | b'"'
        | b'\''
        | b'`'
        | b')'
        | b']'
        | b'}'
        | b'+'
        | b'-'
        | b'*'
        | b'!'
    )
}

/// Ruby auto-semicolon: similar to Go but also after `@`, `$`, `?`, `!` (method names).
fn ruby_auto_semi_after(last_byte: u8) -> bool {
    matches!(
        last_byte,
        b'a'..=b'z'
        | b'A'..=b'Z'
        | b'0'..=b'9'
        | b'_'
        | b'"'
        | b'\''
        | b'`'
        | b')'
        | b']'
        | b'}'
        | b'>'
        | b'@'
        | b'$'
        | b'!'
        | b'?'
    )
}

/// Detect comment node kinds across languages.
fn is_comment(kind: &str) -> bool {
    kind == "comment"
        || kind == "line_comment"
        || kind == "block_comment"
        || kind == "doc_comment"
        || kind == "documentation_comment"
        // HTML comments
        || kind == "html_comment"
    // PHP: heredoc/nowdoc are not comments but often treated similarly
}

/// Detect C/C++ preprocessor node kinds.
fn is_preproc_node(kind: &str) -> bool {
    kind.starts_with("preproc_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn min(s: &str, lang: LangId) -> String {
        minify(s, lang, false).unwrap()
    }

    // ── Tier 1 ──────────────────────────────────────────────

    #[test]
    fn rust_basic() {
        let src = r#"
fn main() {
    let x = 1;
    println!("{}", x);
}
"#;
        let out = min(src, LangId::Rust);
        assert!(out.contains("fn main(){"), "got: {out}");
        assert!(out.contains("let x=1;"), "got: {out}");
        assert!(!out.contains("    "), "got: {out}");
    }

    #[test]
    fn rust_keeps_structure() {
        let src = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        let out = min(src, LangId::Rust);
        assert!(out.contains("fn add(a:i32,b:i32)->i32{"), "got: {out}");
    }

    #[test]
    fn rust_operators() {
        let src = "fn main() {\n    let y = x + 1;\n}\n";
        let out = min(src, LangId::Rust);
        assert!(out.contains("y=x+1;"), "got: {out}");
    }

    #[test]
    fn go_auto_semicolon() {
        let src = "package main\n\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n";
        let out = min(src, LangId::Go);
        assert!(out.contains("package main;"), "got: {out}");
        assert!(out.contains("func main(){"), "got: {out}");
    }

    #[test]
    fn c_preprocessor_preserves_newline() {
        let src = "#include <stdio.h>\nint main() { return 0; }\n";
        let out = min(src, LangId::C);
        assert!(out.contains("#include <stdio.h>\n"), "got: {out}");
        assert!(out.contains("int main(){"), "got: {out}");
    }

    #[test]
    fn python_basic() {
        let src = "def foo():\n    x = 1\n    return x\n";
        let out = min(src, LangId::Python);
        assert!(out.contains("def foo():"), "got: {out}");
        assert!(out.contains("x=1"), "got: {out}");
        assert!(out.contains("return x"), "got: {out}");
    }

    #[test]
    fn java_basic() {
        let src = "public class Main {\n    public static void main(String[] args) {}\n}\n";
        let out = min(src, LangId::Java);
        assert!(out.contains("public class Main{"), "got: {out}");
        assert!(
            out.contains("public static void main(String[]args){}"),
            "got: {out}"
        );
    }

    #[test]
    fn comments_stripped_by_default() {
        let src = "fn main() {\n    // comment\n    let x = 1;\n}\n";
        let out = min(src, LangId::Rust);
        assert!(!out.contains("comment"), "got: {out}");
        assert!(out.contains("let x=1;"), "got: {out}");
    }

    #[test]
    fn comments_kept_when_configured() {
        let src = "fn main() {\n    // keep me\n    let x = 1;\n}\n";
        let out = minify(src, LangId::Rust, true).unwrap();
        assert!(out.contains("// keep me"), "got: {out}");
    }

    #[test]
    fn block_comment_kept() {
        let src = "fn main() {\n    /* block */\n    let x = 1;\n}\n";
        let out = minify(src, LangId::Rust, true).unwrap();
        assert!(out.contains("/* block */"), "got: {out}");
    }

    #[test]
    fn idempotent() {
        let src = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        let first = min(src, LangId::Rust);
        let second = min(&first, LangId::Rust);
        assert_eq!(first, second);
    }

    #[test]
    fn empty_source() {
        assert_eq!(min("", LangId::Rust), "");
    }

    #[test]
    fn single_token() {
        assert_eq!(min("x", LangId::Rust), "x");
    }

    #[test]
    fn javascript_basic() {
        let src = "function hello() {\n  console.log(\"world\");\n}\n";
        let out = min(src, LangId::JavaScript);
        assert!(out.contains("function hello(){"), "got: {out}");
        assert!(out.contains("console.log(\"world\")"), "got: {out}");
    }

    #[test]
    fn typescript_basic() {
        let src = "function add(a: number, b: number): number {\n  return a + b;\n}\n";
        let out = min(src, LangId::TypeScript);
        assert!(
            out.contains("function add(a:number,b:number):number{"),
            "got: {out}"
        );
    }

    #[test]
    fn no_space_before_brackets() {
        let src = "fn main() {\n    foo(1, 2);\n}\n";
        let out = min(src, LangId::Rust);
        assert!(out.contains("foo(1,2)"), "got: {out}");
    }

    #[test]
    fn no_space_after_dot() {
        let src = "let x = foo.bar;\n";
        let out = min(src, LangId::Rust);
        assert!(out.contains("foo.bar"), "got: {out}");
    }

    #[test]
    fn cpp_multiple_includes() {
        let src = "#include <stdio.h>\n#include <stdlib.h>\nint main() { return 0; }\n";
        let out = min(src, LangId::Cpp);
        assert!(out.contains("#include <stdio.h>\n"), "got: {out}");
        assert!(out.contains("#include <stdlib.h>\n"), "got: {out}");
    }

    // ── Tier 2 ──────────────────────────────────────────────

    #[test]
    fn ruby_basic() {
        let src = "def hello\n  puts \"world\"\nend\n";
        let out = min(src, LangId::Ruby);
        assert!(out.contains("def hello;"), "got: {out}");
        assert!(out.contains("puts\"world\";"), "got: {out}");
        assert!(out.contains("end"), "got: {out}");
    }

    #[test]
    fn ruby_class() {
        let src = "class Foo\n  def bar\n    42\n  end\nend\n";
        let out = min(src, LangId::Ruby);
        assert!(out.contains("class Foo;"), "got: {out}");
        assert!(out.contains("def bar;"), "got: {out}");
    }

    #[test]
    fn php_basic() {
        let src = "<?php\nfunction hello() {\n  echo \"world\";\n}\n";
        let out = min(src, LangId::Php);
        assert!(out.contains("<?php"), "got: {out}");
        assert!(out.contains("function hello(){"), "got: {out}");
        assert!(out.contains("echo\"world\""), "got: {out}");
    }

    #[test]
    fn bash_basic() {
        let src = "#!/bin/bash\nif [ -f file ]; then\n  echo \"exists\"\nfi\n";
        let out = min(src, LangId::Bash);
        assert!(out.contains("echo\"exists\";"), "got: {out}");
    }

    #[test]
    fn html_basic() {
        let src = "<html>\n  <body>\n    <p>Hello</p>\n  </body>\n</html>\n";
        let out = min(src, LangId::Html);
        assert!(out.contains("<html>"), "got: {out}");
        assert!(out.contains("<p>Hello</p>"), "got: {out}");
    }

    #[test]
    fn css_basic() {
        let src = "body {\n  color: red;\n  margin: 0;\n}\n";
        let out = min(src, LangId::Css);
        assert!(out.contains("body{"), "got: {out}");
        assert!(out.contains("color:red;"), "got: {out}");
    }

    #[test]
    fn json_basic() {
        let src = "{\n  \"name\": \"test\",\n  \"value\": 42\n}\n";
        let out = min(src, LangId::Json);
        assert!(
            out.contains("{\"name\":\"test\",\"value\":42}"),
            "got: {out}"
        );
    }

    #[test]
    fn csharp_basic() {
        let src = "public class Main {\n    public static void Main(string[] args) {}\n}\n";
        let out = min(src, LangId::CSharp);
        assert!(out.contains("public class Main{"), "got: {out}");
    }

    #[test]
    fn html_comment_stripped() {
        let src = "<div><!-- comment -->text</div>\n";
        let out = min(src, LangId::Html);
        assert!(!out.contains("comment"), "got: {out}");
        assert!(out.contains("<div>text</div>"), "got: {out}");
    }

    #[test]
    fn json_no_comments() {
        let src = "{\n  \"key\": \"value\"\n}\n";
        let out = min(src, LangId::Json);
        assert!(out.contains("{\"key\":\"value\"}"), "got: {out}");
    }
}
