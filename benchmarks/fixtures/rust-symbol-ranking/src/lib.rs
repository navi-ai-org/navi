#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRecord {
    pub name: &'static str,
    pub qualified_name: &'static str,
    pub kind: &'static str,
    pub docs: &'static str,
}

pub fn fixture_symbols() -> Vec<SymbolRecord> {
    vec![
        SymbolRecord {
            name: "DocumentationOnly",
            qualified_name: "crate::docs::DocumentationOnly",
            kind: "Comment",
            docs: "A paragraph that mentions search tools and symbol lookup in prose.",
        },
        SymbolRecord {
            name: "Search",
            qualified_name: "crate::search::Search",
            kind: "Function",
            docs: "Runs a basic search.",
        },
        SymbolRecord {
            name: "SearchTool",
            qualified_name: "crate::tools::SearchTool",
            kind: "Struct",
            docs: "Tool wrapper for search.",
        },
        SymbolRecord {
            name: "FuzzyToolSearch",
            qualified_name: "crate::tools::FuzzyToolSearch",
            kind: "Struct",
            docs: "Symbol search with fuzzy identifier matching.",
        },
    ]
}

pub fn search_symbols(symbols: &[SymbolRecord], query: &str) -> Vec<SymbolRecord> {
    let needle = query.to_ascii_lowercase();
    let mut matches = symbols
        .iter()
        .filter(|symbol| {
            symbol.name.to_ascii_lowercase().contains(&needle)
                || symbol.qualified_name.to_ascii_lowercase().contains(&needle)
                || symbol.docs.to_ascii_lowercase().contains(&needle)
        })
        .cloned()
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.name.cmp(right.name));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_rank_best_identifier_match_first() {
        let results = search_symbols(&fixture_symbols(), "ToolSearch|SearchTool|Search");
        let names = results.iter().map(|symbol| symbol.name).collect::<Vec<_>>();

        assert_eq!(names.first(), Some(&"FuzzyToolSearch"));
        assert!(
            names.iter().position(|name| *name == "SearchTool")
                < names.iter().position(|name| *name == "DocumentationOnly")
        );
    }

    #[test]
    fn symbolic_match_beats_docs_only_match() {
        let results = search_symbols(&fixture_symbols(), "Search");
        let names = results.iter().map(|symbol| symbol.name).collect::<Vec<_>>();

        assert!(names.starts_with(&["Search", "SearchTool"]));
        assert_eq!(names.last(), Some(&"DocumentationOnly"));
    }

    #[test]
    fn natural_language_can_still_find_docs() {
        let results = search_symbols(&fixture_symbols(), "paragraph symbol lookup prose");

        assert_eq!(results.first().map(|symbol| symbol.name), Some("DocumentationOnly"));
    }
}
