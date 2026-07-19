use anyhow::{Context, Result, bail};
use std::path::Path;
use tree_sitter::{Node, Parser};

use crate::lang::{LangId, detect_language};

const MAX_SIGNATURE_BYTES: usize = 240;
const MAX_DIAGNOSTICS: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSymbol {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub language: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub start_column: usize,
    pub end_column: usize,
    pub name_start_byte: usize,
    pub name_end_byte: usize,
    pub parent_id: Option<String>,
    pub signature: String,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeReference {
    pub name: String,
    pub kind: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub column: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeDiagnostic {
    pub kind: String,
    pub message: String,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEdit {
    pub content: String,
    pub edits: usize,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertPosition {
    Before,
    After,
}

/// Extract compact symbol metadata for a single source file.
pub fn symbols_for_source(path: &Path, source: &str) -> Result<Vec<CodeSymbol>> {
    let lang = detect_supported_language(path)?;
    let tree = parse_source(source, lang)?;
    let mut symbols = Vec::new();
    collect_symbols(tree.root_node(), source, lang, None, &mut symbols);
    Ok(symbols)
}

/// Return tree-sitter parse diagnostics for a single source file.
pub fn diagnostics_for_source(path: &Path, source: &str) -> Result<Vec<CodeDiagnostic>> {
    let lang = detect_supported_language(path)?;
    let tree = parse_source(source, lang)?;
    let mut diagnostics = Vec::new();
    collect_diagnostics(tree.root_node(), &mut diagnostics);
    Ok(diagnostics)
}

/// Find exact identifier references in a single source file.
pub fn references_for_source(path: &Path, source: &str, name: &str) -> Result<Vec<CodeReference>> {
    if name.trim().is_empty() {
        bail!("reference name cannot be empty");
    }
    let lang = detect_supported_language(path)?;
    let tree = parse_source(source, lang)?;
    let mut references = Vec::new();
    collect_references(tree.root_node(), source, name, &mut references);
    Ok(references)
}

/// Replace a full symbol definition/body identified by symbol id or unique name.
pub fn replace_symbol_definition(
    path: &Path,
    source: &str,
    selector: &str,
    replacement: &str,
    expected_hash: Option<&str>,
) -> Result<SourceEdit> {
    let symbol = resolve_symbol(path, source, selector)?;
    validate_expected_hash(&symbol, expected_hash)?;

    let mut content = String::with_capacity(source.len() + replacement.len());
    content.push_str(&source[..symbol.start_byte]);
    content.push_str(replacement);
    content.push_str(&source[symbol.end_byte..]);
    validate_source(path, &content)?;

    Ok(SourceEdit {
        content,
        edits: 1,
        start_line: symbol.start_line,
        end_line: symbol.end_line,
    })
}

/// Insert text before or after a symbol identified by symbol id or unique name.
pub fn insert_around_symbol(
    path: &Path,
    source: &str,
    selector: &str,
    insertion: &str,
    position: InsertPosition,
    expected_hash: Option<&str>,
) -> Result<SourceEdit> {
    let symbol = resolve_symbol(path, source, selector)?;
    validate_expected_hash(&symbol, expected_hash)?;
    let byte = match position {
        InsertPosition::Before => symbol.start_byte,
        InsertPosition::After => symbol.end_byte,
    };
    let insertion = normalize_insertion(source, byte, insertion, position);

    let mut content = String::with_capacity(source.len() + insertion.len());
    content.push_str(&source[..byte]);
    content.push_str(&insertion);
    content.push_str(&source[byte..]);
    validate_source(path, &content)?;

    Ok(SourceEdit {
        content,
        edits: 1,
        start_line: symbol.start_line,
        end_line: symbol.end_line,
    })
}

/// Rename exact identifier tokens in one source file and validate the result.
pub fn rename_identifier(
    path: &Path,
    source: &str,
    old_name: &str,
    new_name: &str,
) -> Result<SourceEdit> {
    validate_identifier(old_name).with_context(|| format!("invalid old_name `{old_name}`"))?;
    validate_identifier(new_name).with_context(|| format!("invalid new_name `{new_name}`"))?;
    if old_name == new_name {
        return Ok(SourceEdit {
            content: source.to_string(),
            edits: 0,
            start_line: 0,
            end_line: 0,
        });
    }

    let references = references_for_source(path, source, old_name)?;
    if references.is_empty() {
        bail!("no identifier references named `{old_name}` found");
    }

    let start_line = references.iter().map(|r| r.line).min().unwrap_or(0);
    let end_line = references.iter().map(|r| r.line).max().unwrap_or(0);
    let mut content = source.to_string();
    for reference in references.iter().rev() {
        content.replace_range(reference.start_byte..reference.end_byte, new_name);
    }
    validate_source(path, &content)?;

    Ok(SourceEdit {
        content,
        edits: references.len(),
        start_line,
        end_line,
    })
}

pub fn resolve_symbol(path: &Path, source: &str, selector: &str) -> Result<CodeSymbol> {
    let selector = selector.trim();
    if selector.is_empty() {
        bail!("symbol selector cannot be empty");
    }
    let symbols = symbols_for_source(path, source)?;
    if let Some(symbol) = symbols.iter().find(|symbol| symbol.id == selector) {
        return Ok(symbol.clone());
    }

    let matches = symbols
        .into_iter()
        .filter(|symbol| symbol.name == selector)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => bail!("symbol `{selector}` not found"),
        1 => Ok(matches
            .into_iter()
            .next()
            .context("internal error: expected exactly one symbol match")?),
        _ => bail!(
            "symbol `{selector}` is ambiguous; use a symbol id from symbols_overview/find_symbol"
        ),
    }
}

pub fn validate_source(path: &Path, source: &str) -> Result<()> {
    let diagnostics = diagnostics_for_source(path, source)?;
    if let Some(first) = diagnostics.first() {
        bail!(
            "edited source has parse diagnostics at {}:{}: {}",
            first.start_line,
            first.start_column,
            first.message
        );
    }
    Ok(())
}

pub fn content_hash(content: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn detect_supported_language(path: &Path) -> Result<LangId> {
    detect_language(path)
        .with_context(|| format!("unsupported source language: {}", path.display()))
}

fn parse_source(source: &str, lang: LangId) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(&lang.tree_sitter_language()?)?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter returned None"))
}

fn collect_symbols(
    node: Node<'_>,
    source: &str,
    lang: LangId,
    parent_id: Option<String>,
    symbols: &mut Vec<CodeSymbol>,
) {
    let current_id = if let Some(kind) = symbol_kind_for_node(node, lang) {
        if let Some(name) = node_name(node, source) {
            let text = &source[node.start_byte()..node.end_byte()];
            let start = node.start_position();
            let end = node.end_position();
            let hash = content_hash(text);
            let id = format!(
                "{}:{}:{}:{}",
                kind,
                name.text,
                start.row + 1,
                node.start_byte()
            );
            symbols.push(CodeSymbol {
                id: id.clone(),
                name: name.text,
                kind: kind.to_string(),
                language: lang.name().to_string(),
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
                start_line: start.row + 1,
                end_line: end.row + 1,
                start_column: start.column + 1,
                end_column: end.column + 1,
                name_start_byte: name.start_byte,
                name_end_byte: name.end_byte,
                parent_id: parent_id.clone(),
                signature: signature_preview(text),
                hash,
            });
            Some(id)
        } else {
            parent_id
        }
    } else {
        parent_id
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_symbols(child, source, lang, current_id.clone(), symbols);
        }
    }
}

fn collect_diagnostics(node: Node<'_>, diagnostics: &mut Vec<CodeDiagnostic>) {
    if diagnostics.len() >= MAX_DIAGNOSTICS {
        return;
    }
    if node.is_error() || node.is_missing() {
        let start = node.start_position();
        let end = node.end_position();
        let kind = if node.is_missing() {
            "missing"
        } else {
            "error"
        };
        diagnostics.push(CodeDiagnostic {
            kind: kind.to_string(),
            message: format!("{kind} node `{}`", node.kind()),
            start_line: start.row + 1,
            start_column: start.column + 1,
            end_line: end.row + 1,
            end_column: end.column + 1,
        });
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_diagnostics(child, diagnostics);
        if diagnostics.len() >= MAX_DIAGNOSTICS {
            break;
        }
    }
}

fn collect_references(
    node: Node<'_>,
    source: &str,
    name: &str,
    references: &mut Vec<CodeReference>,
) {
    if is_identifier_node(node.kind()) && node_text(node, source).as_deref() == Some(name) {
        let start = node.start_position();
        references.push(CodeReference {
            name: name.to_string(),
            kind: node.kind().to_string(),
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            line: start.row + 1,
            column: start.column + 1,
            snippet: line_snippet(source, start.row),
        });
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named() {
            collect_references(child, source, name, references);
        }
    }
}

fn symbol_kind_for_node(node: Node<'_>, lang: LangId) -> Option<&'static str> {
    if matches!(lang, LangId::JavaScript | LangId::TypeScript | LangId::Tsx)
        && node.kind() == "variable_declarator"
        && let Some(value) = node.child_by_field_name("value")
    {
        return match value.kind() {
            "arrow_function" | "function" | "function_expression" | "generator_function" => {
                Some("function")
            }
            "class" | "class_expression" => Some("class"),
            _ => None,
        };
    }

    match node.kind() {
        "function_item"
        | "function_signature_item"
        | "function_definition"
        | "function_declaration"
        | "generator_function_declaration"
        | "method_declaration"
        | "method_definition"
        | "method"
        | "singleton_method"
        | "function" => Some("function"),
        "class_declaration"
        | "class_definition"
        | "class"
        | "class_specifier"
        | "abstract_class_declaration" => Some("class"),
        "struct_item" | "struct_specifier" | "record_declaration" => Some("struct"),
        "enum_item" | "enum_declaration" | "enum_specifier" => Some("enum"),
        "trait_item" | "trait_declaration" => Some("trait"),
        "interface_declaration" => Some("interface"),
        "impl_item" => Some("impl"),
        "type_item" | "type_alias_declaration" | "type_declaration" | "type_definition" => {
            Some("type")
        }
        "const_item" | "const_declaration" | "constant_declaration" => Some("const"),
        "static_item" => Some("static"),
        "mod_item" | "module" | "module_declaration" => Some("module"),
        "macro_definition" | "macro_rule" => Some("macro"),
        "constructor_declaration" => Some("constructor"),
        _ => None,
    }
}

struct NameRange {
    text: String,
    start_byte: usize,
    end_byte: usize,
}

fn node_name(node: Node<'_>, source: &str) -> Option<NameRange> {
    if let Some(name) = node.child_by_field_name("name") {
        return name_range(name, source);
    }

    for field in ["declarator", "type", "path"] {
        if let Some(child) = node.child_by_field_name(field)
            && let Some(name) = find_identifier_descendant(child, source)
        {
            return Some(name);
        }
    }

    find_identifier_descendant(node, source)
}

fn find_identifier_descendant(node: Node<'_>, source: &str) -> Option<NameRange> {
    if is_identifier_node(node.kind()) {
        return name_range(node, source);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.is_named()
            && !is_comment_or_string_node(child.kind())
            && let Some(name) = find_identifier_descendant(child, source)
        {
            return Some(name);
        }
    }
    None
}

fn name_range(node: Node<'_>, source: &str) -> Option<NameRange> {
    let text = node_text(node, source)?;
    if !is_identifier_like(&text) {
        return None;
    }
    Some(NameRange {
        text,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    })
}

fn node_text(node: Node<'_>, source: &str) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(str::to_string)
}

fn is_identifier_node(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "property_identifier"
            | "field_identifier"
            | "shorthand_property_identifier"
            | "constant_identifier"
            | "variable_name"
            | "name"
    )
}

fn is_comment_or_string_node(kind: &str) -> bool {
    kind.contains("comment") || kind.contains("string") || kind == "raw_string_literal"
}

fn is_identifier_like(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

fn validate_identifier(value: &str) -> Result<()> {
    if is_identifier_like(value) {
        Ok(())
    } else {
        bail!("identifier must match [A-Za-z_$][A-Za-z0-9_$]*")
    }
}

fn signature_preview(text: &str) -> String {
    let mut preview = String::new();
    for line in text.lines().take(6) {
        if !preview.is_empty() {
            preview.push(' ');
        }
        preview.push_str(line.trim());
        if line.contains('{')
            || line.trim_end().ends_with(';')
            || preview.len() >= MAX_SIGNATURE_BYTES
        {
            break;
        }
    }
    let preview = normalize_ws(&preview);
    truncate_chars(&preview, MAX_SIGNATURE_BYTES)
}

fn normalize_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut out = String::new();
    for ch in value.chars() {
        if out.len() + ch.len_utf8() + 1 > max_bytes {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn line_snippet(source: &str, row: usize) -> String {
    source
        .lines()
        .nth(row)
        .map(str::trim)
        .map(|line| truncate_chars(line, MAX_SIGNATURE_BYTES))
        .unwrap_or_default()
}

fn validate_expected_hash(symbol: &CodeSymbol, expected_hash: Option<&str>) -> Result<()> {
    if let Some(expected) = expected_hash
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && expected != symbol.hash
    {
        bail!(
            "stale symbol `{}`: expected hash `{}`, current hash `{}`",
            symbol.id,
            expected,
            symbol.hash
        );
    }
    Ok(())
}

fn normalize_insertion(
    source: &str,
    byte: usize,
    insertion: &str,
    position: InsertPosition,
) -> String {
    if insertion.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    if matches!(position, InsertPosition::After)
        && byte > 0
        && !source[..byte].ends_with('\n')
        && !insertion.starts_with('\n')
    {
        out.push('\n');
    }
    out.push_str(insertion);
    if matches!(position, InsertPosition::Before)
        && byte < source.len()
        && !out.ends_with('\n')
        && !source[byte..].starts_with('\n')
    {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST_SRC: &str = r#"
pub struct Greeter;

impl Greeter {
    pub fn greet(&self) -> &'static str {
        "hello"
    }
}

fn helper(value: i32) -> i32 {
    value + 1
}
"#;

    #[test]
    fn extracts_rust_symbols_with_hashes() {
        let symbols = symbols_for_source(Path::new("src/lib.rs"), RUST_SRC).unwrap();
        assert!(
            symbols
                .iter()
                .any(|s| s.kind == "struct" && s.name == "Greeter")
        );
        let helper = symbols.iter().find(|s| s.name == "helper").unwrap();
        assert_eq!(helper.kind, "function");
        assert!(!helper.hash.is_empty());
        assert!(helper.signature.contains("fn helper"));
    }

    #[test]
    fn extracts_javascript_arrow_function_symbols() {
        let source = "export const greet = (name) => `hello ${name}`;\n";
        let symbols = symbols_for_source(Path::new("src/app.ts"), source).unwrap();

        let greet = symbols.iter().find(|s| s.name == "greet").unwrap();
        assert_eq!(greet.kind, "function");
        assert!(greet.signature.contains("greet"));
    }

    #[test]
    fn parses_tsx_with_jsx_syntax() {
        let source = "export const View = () => <div>{'hello'}</div>;\n";
        let symbols = symbols_for_source(Path::new("src/View.tsx"), source).unwrap();

        let view = symbols.iter().find(|s| s.name == "View").unwrap();
        assert_eq!(view.kind, "function");
    }

    #[test]
    fn finds_identifier_references() {
        let refs = references_for_source(Path::new("src/lib.rs"), RUST_SRC, "value").unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|r| r.snippet.contains("value")));
    }

    #[test]
    fn replace_symbol_definition_validates_hash() {
        let symbol = resolve_symbol(Path::new("src/lib.rs"), RUST_SRC, "helper").unwrap();
        let replacement = "fn helper(value: i32) -> i32 {\n    value + 2\n}";
        let edit = replace_symbol_definition(
            Path::new("src/lib.rs"),
            RUST_SRC,
            &symbol.id,
            replacement,
            Some(&symbol.hash),
        )
        .unwrap();
        assert!(edit.content.contains("value + 2"));
        assert_eq!(edit.edits, 1);
    }

    #[test]
    fn rejects_stale_hash() {
        let err = replace_symbol_definition(
            Path::new("src/lib.rs"),
            RUST_SRC,
            "helper",
            "fn helper() {}",
            Some("bad"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("stale symbol"));
    }

    #[test]
    fn rename_identifier_updates_tokens_only() {
        let edit = rename_identifier(Path::new("src/lib.rs"), RUST_SRC, "value", "amount").unwrap();
        assert_eq!(edit.edits, 2);
        assert!(edit.content.contains("helper(amount: i32)"));
        assert!(edit.content.contains("amount + 1"));
    }

    #[test]
    fn diagnostics_report_parse_errors() {
        let diagnostics = diagnostics_for_source(Path::new("src/lib.rs"), "fn broken( {").unwrap();
        assert!(!diagnostics.is_empty());
    }
}
