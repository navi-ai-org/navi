//! Repository intelligence: compact symbol, dependency, and test discovery.
//!
//! This module is intentionally UI-agnostic and deterministic. It gives the
//! harness structured repo facts before falling back to grep-heavy exploration.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoIndex {
    pub root: PathBuf,
    pub indexed_at_ms: u64,
    pub files: Vec<IndexedFile>,
    pub symbols: Vec<SymbolRecord>,
    pub imports: Vec<ImportRecord>,
    pub tests: Vec<TestTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub language: String,
    pub hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolRecord {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedSymbolRecord {
    pub symbol: SymbolRecord,
    pub score: f64,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextMatchRecord {
    pub path: PathBuf,
    pub line: usize,
    pub kind: String,
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportRecord {
    pub path: PathBuf,
    pub target: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceRecord {
    pub name: String,
    pub path: PathBuf,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyEdge {
    pub from: PathBuf,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestTarget {
    pub command: String,
    pub scope: String,
    pub confidence: u8,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChurnRecord {
    pub path: PathBuf,
    pub commits: u32,
}

#[derive(Debug, Default)]
pub struct RepoIntelligenceCache {
    files: HashMap<PathBuf, CachedFile>,
}

#[derive(Debug, Clone)]
struct CachedFile {
    metadata_key: String,
    indexed: IndexedFile,
    symbols: Vec<SymbolRecord>,
    imports: Vec<ImportRecord>,
    tests: Vec<TestTarget>,
}

impl RepoIntelligenceCache {
    pub fn index_project(&mut self, root: &Path) -> Result<RepoIndex> {
        build_index_with_cache(root, Some(self))
    }
}

pub fn build_index(root: &Path) -> Result<RepoIndex> {
    build_index_with_cache(root, None)
}

pub fn search_symbols(index: &RepoIndex, query: &str, kind: Option<&str>) -> Vec<SymbolRecord> {
    ranked_symbol_matches(index, query, kind)
        .into_iter()
        .map(|ranked| ranked.symbol)
        .collect()
}

pub fn goto_symbol(index: &RepoIndex, name: &str) -> Option<SymbolRecord> {
    ranked_symbol_matches(index, name, None)
        .into_iter()
        .next()
        .map(|ranked| ranked.symbol)
}

pub fn ranked_symbol_matches(
    index: &RepoIndex,
    query: &str,
    kind: Option<&str>,
) -> Vec<RankedSymbolRecord> {
    SymbolRanker::new(query).rank(index, kind)
}

pub fn search_text_matches(
    index: &RepoIndex,
    query: &str,
    max_results: usize,
) -> Vec<TextMatchRecord> {
    let query_alternatives = query_alternatives(query);
    if query_alternatives
        .iter()
        .all(|alternative| alternative.tokens.is_empty())
    {
        return Vec::new();
    }

    let documents = text_documents(index);
    let mut best_matches = documents
        .iter()
        .filter_map(|document| {
            let score = query_alternatives
                .iter()
                .map(|alternative| bm25_score(&documents, &alternative.tokens, document))
                .fold(0.0, f64::max);
            (score > 0.0).then(|| TextMatchRecord {
                path: document.path.clone(),
                line: document.line,
                kind: document.kind.clone(),
                text: document.text.clone(),
                score: round_score(score),
            })
        })
        .collect::<Vec<_>>();

    best_matches.sort_by(|left, right| {
        score_cmp(right.score, left.score)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.text.cmp(&right.text))
    });
    best_matches.truncate(max_results);
    best_matches
}

#[derive(Debug, Clone)]
struct QueryAlternative {
    normalized: String,
    tokens: Vec<String>,
}

#[derive(Debug)]
struct SymbolRanker {
    alternatives: Vec<QueryAlternative>,
}

#[derive(Debug, Clone)]
struct SymbolScore {
    score: f64,
    reasons: Vec<String>,
}

impl SymbolRanker {
    fn new(query: &str) -> Self {
        Self {
            alternatives: query_alternatives(query),
        }
    }

    fn rank(&self, index: &RepoIndex, kind: Option<&str>) -> Vec<RankedSymbolRecord> {
        let signature_documents = symbol_signature_documents(index);
        let kind = kind.map(str::to_ascii_lowercase);
        let mut ranked = index
            .symbols
            .iter()
            .filter(|symbol| {
                kind.as_ref()
                    .map(|kind| symbol.kind.eq_ignore_ascii_case(kind))
                    .unwrap_or(true)
            })
            .filter_map(|symbol| {
                let score = self.score_symbol(symbol, &signature_documents);
                (score.score > 0.0).then(|| RankedSymbolRecord {
                    symbol: symbol.clone(),
                    score: round_score(score.score),
                    reasons: score.reasons,
                })
            })
            .collect::<Vec<_>>();

        ranked.sort_by(|left, right| {
            score_cmp(right.score, left.score)
                .then_with(|| left.symbol.path.cmp(&right.symbol.path))
                .then_with(|| left.symbol.line.cmp(&right.symbol.line))
                .then_with(|| left.symbol.name.cmp(&right.symbol.name))
        });
        ranked
    }

    fn score_symbol(
        &self,
        symbol: &SymbolRecord,
        signature_documents: &[TextDocument],
    ) -> SymbolScore {
        if self
            .alternatives
            .iter()
            .all(|alternative| alternative.tokens.is_empty())
        {
            return SymbolScore {
                score: 1.0 + kind_boost(symbol),
                reasons: vec!["empty_query".to_string()],
            };
        }

        self.alternatives
            .iter()
            .map(|alternative| {
                score_symbol_for_alternative(symbol, alternative, signature_documents)
            })
            .max_by(|left, right| score_cmp(left.score, right.score))
            .unwrap_or(SymbolScore {
                score: 0.0,
                reasons: Vec::new(),
            })
    }
}

fn score_symbol_for_alternative(
    symbol: &SymbolRecord,
    query: &QueryAlternative,
    signature_documents: &[TextDocument],
) -> SymbolScore {
    let name_tokens = tokenize_identifier(&symbol.name);
    let signature_tokens = tokenize_identifier(&symbol.signature);
    let path_text = symbol.path.to_string_lossy();
    let path_tokens = tokenize_identifier(&path_text);
    let qualified_text = format!("{}::{}", path_text, symbol.name);
    let normalized_name = normalize_identifier(&symbol.name);
    let normalized_signature = normalize_identifier(&symbol.signature);
    let normalized_path = normalize_identifier(&path_text);
    let normalized_qualified = normalize_identifier(&qualified_text);

    let mut score = 0.0;
    let mut reasons = Vec::new();

    if normalized_name == query.normalized {
        score += 1000.0;
        reasons.push("exact_name".to_string());
    } else if normalized_qualified == query.normalized {
        score += 950.0;
        reasons.push("exact_qualified".to_string());
    }

    if !query.tokens.is_empty() {
        let name_coverage = token_coverage(&query.tokens, &name_tokens);
        if name_coverage > 0.0 {
            score += 360.0 * name_coverage;
            reasons.push(format!("name_token_coverage:{name_coverage:.2}"));
            if name_coverage >= 1.0 {
                score += 140.0;
                reasons.push("all_name_tokens".to_string());
            }
        }

        if tokens_in_order(&query.tokens, &name_tokens) {
            score += 90.0;
            reasons.push("tokens_in_order".to_string());
        }

        let path_coverage = token_coverage(&query.tokens, &path_tokens);
        if path_coverage > 0.0 {
            score += 22.0 * path_coverage;
            reasons.push(format!("path_tokens:{path_coverage:.2}"));
        }

        let signature_coverage = token_coverage(&query.tokens, &signature_tokens);
        if signature_coverage > 0.0 {
            score += 28.0 * signature_coverage;
            reasons.push(format!("signature_tokens:{signature_coverage:.2}"));
        }
    }

    if !query.normalized.is_empty() {
        if normalized_name.starts_with(&query.normalized) {
            score += 160.0;
            reasons.push("name_prefix".to_string());
        }
        if normalized_name.ends_with(&query.normalized) {
            score += 95.0;
            reasons.push("name_suffix".to_string());
        }
        if normalized_name.contains(&query.normalized) {
            score += 130.0;
            reasons.push("name_contains".to_string());
        } else if is_subsequence(&query.normalized, &normalized_name) {
            score += 65.0;
            reasons.push("name_subsequence".to_string());
        }

        if normalized_signature.contains(&query.normalized) {
            score += 32.0;
            reasons.push("signature_contains".to_string());
        }
        if normalized_path.contains(&query.normalized) {
            score += 18.0;
            reasons.push("path_contains".to_string());
        }

        if let Some(distance_score) = edit_distance_score(&query.normalized, &normalized_name) {
            score += distance_score;
            reasons.push("edit_distance".to_string());
        }
    }

    let bm25 = signature_documents
        .iter()
        .find(|document| document.path == symbol.path && document.line == symbol.line)
        .map(|document| bm25_score(signature_documents, &query.tokens, document))
        .unwrap_or(0.0)
        .min(35.0);
    if bm25 > 0.0 {
        score += bm25;
        reasons.push("bm25_signature".to_string());
    }

    if score > 0.0 {
        let boost = kind_boost(symbol);
        if boost > 0.0 {
            score += boost;
            reasons.push(format!("kind:{}", symbol.kind));
        }
    }

    SymbolScore { score, reasons }
}

fn query_alternatives(query: &str) -> Vec<QueryAlternative> {
    let alternatives = query
        .split('|')
        .map(str::trim)
        .filter(|alternative| !alternative.is_empty())
        .map(|alternative| QueryAlternative {
            normalized: normalize_identifier(alternative),
            tokens: tokenize_identifier(alternative),
        })
        .collect::<Vec<_>>();
    if alternatives.is_empty() {
        vec![QueryAlternative {
            normalized: String::new(),
            tokens: Vec::new(),
        }]
    } else {
        alternatives
    }
}

fn tokenize_identifier(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if !ch.is_ascii_alphanumeric() {
            push_token(&mut parts, &mut current);
            previous = None;
            continue;
        }

        let next = chars.peek().copied();
        let boundary = previous.is_some_and(|prev| {
            (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
                || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
                || (prev.is_ascii_uppercase()
                    && ch.is_ascii_uppercase()
                    && next.is_some_and(|next| next.is_ascii_lowercase()))
        });
        if boundary {
            push_token(&mut parts, &mut current);
        }
        current.push(ch);
        previous = Some(ch);
    }
    push_token(&mut parts, &mut current);
    parts
}

fn push_token(parts: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }
    let token = normalize_token(current);
    if !token.is_empty() {
        parts.push(token);
    }
    current.clear();
}

fn normalize_token(token: &str) -> String {
    let lower = token.to_ascii_lowercase();
    if lower.len() > 5 && lower.ends_with("ing") {
        return lower.trim_end_matches("ing").to_string();
    }
    if lower.len() > 4 && lower.ends_with("ies") {
        return format!("{}y", &lower[..lower.len() - 3]);
    }
    if lower.len() > 4 && lower.ends_with("es") && !lower.ends_with("ses") {
        return lower[..lower.len() - 2].to_string();
    }
    if lower.len() > 3 && lower.ends_with('s') && !lower.ends_with("ss") {
        return lower[..lower.len() - 1].to_string();
    }
    lower
}

fn normalize_identifier(text: &str) -> String {
    tokenize_identifier(text).join("")
}

fn token_coverage(query_tokens: &[String], candidate_tokens: &[String]) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let candidate_tokens = candidate_tokens.iter().collect::<HashSet<_>>();
    let covered = query_tokens
        .iter()
        .filter(|token| candidate_tokens.contains(token))
        .count();
    covered as f64 / query_tokens.len() as f64
}

fn tokens_in_order(query_tokens: &[String], candidate_tokens: &[String]) -> bool {
    if query_tokens.is_empty() {
        return false;
    }
    let mut candidate_iter = candidate_tokens.iter();
    query_tokens
        .iter()
        .all(|query| candidate_iter.any(|candidate| candidate == query))
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut haystack = haystack.chars();
    needle
        .chars()
        .all(|needle_ch| haystack.any(|haystack_ch| haystack_ch == needle_ch))
}

fn edit_distance_score(query: &str, candidate: &str) -> Option<f64> {
    if query.len() < 4 || candidate.len() < 4 || query.len().abs_diff(candidate.len()) > 2 {
        return None;
    }
    let distance = edit_distance(query, candidate);
    (distance <= 2).then_some(80.0 - (distance as f64 * 22.0))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    for (left_idx, left_ch) in left.chars().enumerate() {
        let mut current = vec![left_idx + 1];
        for (right_idx, right_ch) in right_chars.iter().enumerate() {
            let insert = current[right_idx] + 1;
            let delete = previous[right_idx + 1] + 1;
            let replace = previous[right_idx] + usize::from(left_ch != *right_ch);
            current.push(insert.min(delete).min(replace));
        }
        previous = current;
    }
    previous[right_chars.len()]
}

fn kind_boost(symbol: &SymbolRecord) -> f64 {
    match symbol.kind.as_str() {
        "function" | "struct" | "class" | "trait" | "interface" => 18.0,
        "enum" | "type" => 12.0,
        "impl" | "constant" => 8.0,
        _ => 0.0,
    }
}

#[derive(Debug, Clone)]
struct TextDocument {
    path: PathBuf,
    line: usize,
    kind: String,
    text: String,
    tokens: Vec<String>,
}

fn text_documents(index: &RepoIndex) -> Vec<TextDocument> {
    let mut documents = symbol_signature_documents(index);
    for file in &index.files {
        let path = index.root.join(&file.path);
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        documents.extend(comment_documents(&file.path, &file.language, &content));
        documents.extend(snippet_documents(&file.path, &content));
    }
    dedupe_documents(documents)
}

fn symbol_signature_documents(index: &RepoIndex) -> Vec<TextDocument> {
    index
        .symbols
        .iter()
        .map(|symbol| {
            let text = format!("{} {}", symbol.name, symbol.signature);
            TextDocument {
                path: symbol.path.clone(),
                line: symbol.line,
                kind: "signature".to_string(),
                tokens: tokenize_identifier(&text),
                text: compact_text(&text),
            }
        })
        .collect()
}

fn comment_documents(path: &Path, language: &str, content: &str) -> Vec<TextDocument> {
    let mut documents = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let (kind, text) = if let Some(text) = trimmed.strip_prefix("///") {
            ("doc", text)
        } else if let Some(text) = trimmed.strip_prefix("//!") {
            ("doc", text)
        } else if let Some(text) = trimmed.strip_prefix("//") {
            ("comment", text)
        } else if language == "python" {
            if let Some(text) = trimmed.strip_prefix('#') {
                ("comment", text)
            } else {
                continue;
            }
        } else if let Some(text) = trimmed
            .strip_prefix("/*")
            .map(|value| value.trim_end_matches("*/"))
        {
            ("comment", text)
        } else if let Some(text) = trimmed
            .strip_prefix('*')
            .map(|value| value.trim_end_matches("*/"))
        {
            ("comment", text)
        } else {
            continue;
        };

        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        documents.push(TextDocument {
            path: path.to_path_buf(),
            line: idx + 1,
            kind: kind.to_string(),
            text: compact_text(text),
            tokens: tokenize_identifier(text),
        });
    }
    documents
}

fn snippet_documents(path: &Path, content: &str) -> Vec<TextDocument> {
    content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let text = line.trim();
            if text.is_empty()
                || text.starts_with("//")
                || text.starts_with("///")
                || text.starts_with("#")
                || text.len() > 280
            {
                return None;
            }
            let tokens = tokenize_identifier(text);
            (tokens.len() >= 3).then(|| TextDocument {
                path: path.to_path_buf(),
                line: idx + 1,
                kind: "snippet".to_string(),
                text: compact_text(text),
                tokens,
            })
        })
        .collect()
}

fn dedupe_documents(documents: Vec<TextDocument>) -> Vec<TextDocument> {
    let mut seen = BTreeSet::new();
    documents
        .into_iter()
        .filter(|document| {
            seen.insert((
                document.path.clone(),
                document.line,
                document.kind.clone(),
                document.text.clone(),
            ))
        })
        .collect()
}

fn bm25_score(corpus: &[TextDocument], query_tokens: &[String], document: &TextDocument) -> f64 {
    if corpus.is_empty() || query_tokens.is_empty() || document.tokens.is_empty() {
        return 0.0;
    }
    let average_len = corpus
        .iter()
        .map(|doc| doc.tokens.len() as f64)
        .sum::<f64>()
        / corpus.len() as f64;
    let document_len = document.tokens.len() as f64;
    let mut frequencies = HashMap::<&str, usize>::new();
    for token in &document.tokens {
        *frequencies.entry(token.as_str()).or_default() += 1;
    }

    let unique_query_tokens = query_tokens.iter().collect::<BTreeSet<_>>();
    let mut score = 0.0;
    for token in unique_query_tokens {
        let Some(term_frequency) = frequencies.get(token.as_str()).copied() else {
            continue;
        };
        let document_frequency = corpus
            .iter()
            .filter(|doc| doc.tokens.iter().any(|candidate| candidate == token))
            .count() as f64;
        let corpus_len = corpus.len() as f64;
        let idf = (1.0 + (corpus_len - document_frequency + 0.5) / (document_frequency + 0.5)).ln();
        let tf = term_frequency as f64;
        let k1 = 1.2;
        let b = 0.75;
        score += idf * (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * document_len / average_len));
    }
    score * 12.0
}

fn compact_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

fn score_cmp(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

fn round_score(score: f64) -> f64 {
    (score * 100.0).round() / 100.0
}

pub fn references(index: &RepoIndex, name: &str) -> Vec<ReferenceRecord> {
    let mut refs = Vec::new();
    for file in &index.files {
        let path = index.root.join(&file.path);
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if contains_identifier(line, name) {
                refs.push(ReferenceRecord {
                    name: name.to_string(),
                    path: file.path.clone(),
                    line: idx + 1,
                    text: line.trim().chars().take(240).collect(),
                });
            }
        }
    }
    refs
}

pub fn dependency_edges(index: &RepoIndex) -> Vec<DependencyEdge> {
    index
        .imports
        .iter()
        .map(|import| DependencyEdge {
            from: import.path.clone(),
            to: import.target.clone(),
        })
        .collect()
}

pub fn discover_tests(root: &Path, touched_paths: &[PathBuf]) -> Vec<TestTarget> {
    let mut targets = BTreeMap::<String, TestTarget>::new();
    let justfile = root.join("justfile");
    let has_just = justfile.exists();
    let has_cargo = root.join("Cargo.toml").exists();

    if has_just && has_cargo {
        for path in touched_paths {
            if let Some(crate_name) = crate_name_from_path(path) {
                let command = format!("just test-crate {crate_name}");
                targets.insert(
                    command.clone(),
                    TestTarget {
                        command,
                        scope: crate_name,
                        confidence: 95,
                        reason: "Rust crate path under workspace with just test-crate recipe"
                            .to_string(),
                    },
                );
            }
        }
        targets
            .entry("just test".to_string())
            .or_insert(TestTarget {
                command: "just test".to_string(),
                scope: "workspace".to_string(),
                confidence: 70,
                reason: "Rust workspace with justfile".to_string(),
            });
    } else if has_cargo {
        targets
            .entry("cargo test".to_string())
            .or_insert(TestTarget {
                command: "cargo test".to_string(),
                scope: "workspace".to_string(),
                confidence: 60,
                reason: "Cargo.toml detected".to_string(),
            });
    }

    if root.join("package.json").exists() {
        targets.entry("npm test".to_string()).or_insert(TestTarget {
            command: "npm test".to_string(),
            scope: "package".to_string(),
            confidence: 60,
            reason: "package.json detected".to_string(),
        });
    }
    if root.join("pyproject.toml").exists() || root.join("pytest.ini").exists() {
        targets
            .entry("pytest -q".to_string())
            .or_insert(TestTarget {
                command: "pytest -q".to_string(),
                scope: "python".to_string(),
                confidence: 60,
                reason: "Python test config detected".to_string(),
            });
    }

    dedupe_tests(targets.into_values().collect())
}

pub fn churn_from_git_log(root: &Path, max_entries: usize) -> Vec<ChurnRecord> {
    let output = std::process::Command::new("git")
        .arg("log")
        .arg("--name-only")
        .arg("--pretty=format:")
        .current_dir(root)
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let mut counts = BTreeMap::<PathBuf, u32>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        *counts.entry(PathBuf::from(line)).or_default() += 1;
    }
    let mut records = counts
        .into_iter()
        .map(|(path, commits)| ChurnRecord { path, commits })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        right
            .commits
            .cmp(&left.commits)
            .then_with(|| left.path.cmp(&right.path))
    });
    records.truncate(max_entries);
    records
}

fn build_index_with_cache(
    root: &Path,
    mut cache: Option<&mut RepoIntelligenceCache>,
) -> Result<RepoIndex> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve repo root {}", root.display()))?;
    let mut files = Vec::new();
    collect_source_files(&root, &root, &mut files)?;
    files.sort();

    let mut indexed_files = Vec::new();
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut tests = discover_tests(&root, &[]);

    for relative in files {
        let absolute = root.join(&relative);
        let metadata = fs::metadata(&absolute)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let metadata_key = format!("{}:{}:{modified}", metadata.len(), relative.display());
        if let Some(cache) = cache.as_deref_mut()
            && let Some(cached) = cache.files.get(&relative)
            && cached.metadata_key == metadata_key
        {
            indexed_files.push(cached.indexed.clone());
            symbols.extend(cached.symbols.clone());
            imports.extend(cached.imports.clone());
            tests.extend(cached.tests.clone());
            continue;
        }

        let content = fs::read_to_string(&absolute)
            .with_context(|| format!("failed to read source file {}", absolute.display()))?;
        let language = language_for_path(&relative).to_string();
        let hash = hash_content(&content);
        let indexed = IndexedFile {
            path: relative.clone(),
            language,
            hash,
            bytes: metadata.len(),
        };
        let file_symbols = extract_symbols(&relative, &content);
        let file_imports = extract_imports(&relative, &content);
        let file_tests = test_targets_for_file(&relative);
        if let Some(cache) = cache.as_deref_mut() {
            cache.files.insert(
                relative.clone(),
                CachedFile {
                    metadata_key,
                    indexed: indexed.clone(),
                    symbols: file_symbols.clone(),
                    imports: file_imports.clone(),
                    tests: file_tests.clone(),
                },
            );
        }
        indexed_files.push(indexed);
        symbols.extend(file_symbols);
        imports.extend(file_imports);
        tests.extend(file_tests);
    }

    tests = dedupe_tests(tests);
    Ok(RepoIndex {
        root,
        indexed_at_ms: now_ms(),
        files: indexed_files,
        symbols,
        imports,
        tests,
    })
}

fn collect_source_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules" | "dist") {
            continue;
        }
        if path.is_dir() {
            collect_source_files(root, &path, out)?;
            continue;
        }
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        if language_for_path(relative) != "unknown" {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn extract_symbols(path: &Path, content: &str) -> Vec<SymbolRecord> {
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        let Some((kind, name)) = symbol_from_line(trimmed) else {
            continue;
        };
        out.push(SymbolRecord {
            name,
            kind: kind.to_string(),
            path: path.to_path_buf(),
            line: idx + 1,
            signature: compact_signature(trimmed),
        });
    }
    out
}

fn compact_signature(line: &str) -> String {
    line.split('{')
        .next()
        .unwrap_or(line)
        .trim_end()
        .chars()
        .take(240)
        .collect()
}

fn symbol_from_line(line: &str) -> Option<(&'static str, String)> {
    let line = line.strip_prefix("pub ").unwrap_or(line);
    for (prefix, kind) in [
        ("fn ", "function"),
        ("async fn ", "function"),
        ("struct ", "struct"),
        ("enum ", "enum"),
        ("trait ", "trait"),
        ("type ", "type"),
        ("impl ", "impl"),
        ("const ", "constant"),
        ("class ", "class"),
        ("function ", "function"),
        ("interface ", "interface"),
        ("export function ", "function"),
        ("export class ", "class"),
        ("export interface ", "interface"),
    ] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name = rest
                .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                .next()
                .unwrap_or_default();
            if !name.is_empty() {
                return Some((kind, name.to_string()));
            }
        }
    }
    None
}

fn extract_imports(path: &Path, content: &str) -> Vec<ImportRecord> {
    let mut out = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        let target = if let Some(rest) = trimmed.strip_prefix("use ") {
            rest.trim_end_matches(';')
                .split("::")
                .next()
                .map(str::to_string)
        } else if let Some(rest) = trimmed.strip_prefix("mod ") {
            rest.trim_end_matches(';')
                .split_whitespace()
                .next()
                .map(str::to_string)
        } else if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
            quoted_module(trimmed)
        } else {
            None
        };
        if let Some(target) = target.filter(|value| !value.is_empty()) {
            out.push(ImportRecord {
                path: path.to_path_buf(),
                target,
                line: idx + 1,
            });
        }
    }
    out
}

fn quoted_module(line: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let mut parts = line.rsplit(quote);
        let _tail = parts.next()?;
        let value = parts.next()?;
        if !value.is_empty() && !value.contains(' ') {
            return Some(value.to_string());
        }
    }
    None
}

fn test_targets_for_file(path: &Path) -> Vec<TestTarget> {
    let path_str = path.to_string_lossy();
    let mut out = Vec::new();
    if path_str.ends_with("_test.rs") || path_str.contains("/tests/") {
        out.push(TestTarget {
            command: "just test".to_string(),
            scope: path_str.to_string(),
            confidence: 80,
            reason: "test file detected".to_string(),
        });
    }
    if path_str.ends_with(".test.ts")
        || path_str.ends_with(".test.tsx")
        || path_str.ends_with(".spec.ts")
        || path_str.ends_with(".spec.tsx")
    {
        out.push(TestTarget {
            command: format!("npm test -- {}", path.display()),
            scope: path_str.to_string(),
            confidence: 75,
            reason: "JS/TS test file detected".to_string(),
        });
    }
    out
}

fn crate_name_from_path(path: &Path) -> Option<String> {
    let mut components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy());
    while let Some(component) = components.next() {
        if component == "crates" {
            return components.next().map(|value| value.to_string());
        }
    }
    None
}

fn dedupe_tests(tests: Vec<TestTarget>) -> Vec<TestTarget> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for test in tests {
        if seen.insert(test.command.clone()) {
            out.push(test);
        }
    }
    out.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| left.command.cmp(&right.command))
    });
    out
}

fn contains_identifier(line: &str, name: &str) -> bool {
    line.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .any(|part| part == name)
}

fn language_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => "rust",
        Some("ts") | Some("tsx") => "typescript",
        Some("js") | Some("jsx") => "javascript",
        Some("go") => "go",
        Some("py") => "python",
        _ => "unknown",
    }
}

fn hash_content(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn ensure_indexed(index: &RepoIndex) -> Result<()> {
    if index.files.is_empty() {
        bail!("repository index is empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_source(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn indexes_rust_symbols_and_references() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "use std::fmt;\npub struct Engine;\npub fn run_engine() {}\nfn call() { run_engine(); }\n",
        )
        .unwrap();

        let index = build_index(dir.path()).unwrap();

        let matches = search_symbols(&index, "run_engine", Some("function"));
        assert_eq!(matches.len(), 1, "symbols: {:?}", index.symbols);
        assert_eq!(goto_symbol(&index, "Engine").unwrap().kind, "struct");
        assert_eq!(references(&index, "run_engine").len(), 2);
        assert_eq!(dependency_edges(&index)[0].to, "std");
    }

    #[test]
    fn discovers_smallest_just_test_command_for_crate_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(dir.path().join("justfile"), "test:\n  cargo test\n").unwrap();

        let tests = discover_tests(dir.path(), &[PathBuf::from("crates/navi-core/src/lib.rs")]);

        assert_eq!(
            tests[0].command, "just test-crate navi-core",
            "tests: {tests:?}"
        );
    }

    #[test]
    fn cache_reuses_unchanged_file_records() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "pub fn stable() {}\n").unwrap();
        let mut cache = RepoIntelligenceCache::default();

        let first = cache.index_project(dir.path()).unwrap();
        let second = cache.index_project(dir.path()).unwrap();

        assert_eq!(first.files[0].hash, second.files[0].hash);
        assert_eq!(second.symbols[0].name, "stable");
    }

    #[test]
    fn tokenizes_identifiers_and_tool_names() {
        assert_eq!(
            tokenize_identifier("FuzzyToolSearch"),
            vec!["fuzzy", "tool", "search"]
        );
        assert_eq!(tokenize_identifier("tool_search"), vec!["tool", "search"]);
        assert_eq!(tokenize_identifier("symbol.goto"), vec!["symbol", "goto"]);
        assert_eq!(
            tokenize_identifier("dependency_graph.query"),
            vec!["dependency", "graph", "query"]
        );
    }

    #[test]
    fn ranks_symbol_alternatives_by_best_candidate() {
        let dir = tempfile::tempdir().unwrap();
        write_source(
            dir.path(),
            "src/lib.rs",
            "pub struct FuzzyToolSearch;\npub struct OtherThing;\n",
        );
        let index = build_index(dir.path()).unwrap();

        let matches = search_symbols(&index, "ToolSearch|SearchTool|Search", None);

        assert_eq!(matches[0].name, "FuzzyToolSearch");
    }

    #[test]
    fn symbolic_matches_outrank_bm25_text_matches() {
        let dir = tempfile::tempdir().unwrap();
        write_source(
            dir.path(),
            "src/lib.rs",
            "/// This comment talks about search but is not the target.\npub struct FuzzyToolSearch;\n",
        );
        let index = build_index(dir.path()).unwrap();

        let ranked = ranked_symbol_matches(&index, "Search", None);
        let text_matches = search_text_matches(&index, "Search", 10);

        assert_eq!(ranked[0].symbol.name, "FuzzyToolSearch");
        assert!(
            ranked[0].score > text_matches[0].score,
            "symbol={:?} text={:?}",
            ranked[0],
            text_matches[0]
        );
    }

    #[test]
    fn goto_symbol_uses_ranker_instead_of_first_contains() {
        let dir = tempfile::tempdir().unwrap();
        write_source(
            dir.path(),
            "src/lib.rs",
            "pub struct PrefixSearchToolSuffix;\npub struct SearchTool;\n",
        );
        let index = build_index(dir.path()).unwrap();

        let symbol = goto_symbol(&index, "SearchTool").unwrap();

        assert_eq!(symbol.name, "SearchTool");
    }

    #[test]
    fn bm25_returns_docs_comments_and_snippets_for_natural_language() {
        let dir = tempfile::tempdir().unwrap();
        write_source(
            dir.path(),
            "src/lib.rs",
            "/// Tool that searches symbols in docs and comments.\npub fn fuzzy_tool_search() {}\nfn caller() { fuzzy_tool_search(); }\n",
        );
        let index = build_index(dir.path()).unwrap();

        let text_matches = search_text_matches(&index, "tool that searches symbols in docs", 10);

        assert!(
            text_matches.iter().any(|record| record.kind == "doc"),
            "text_matches: {text_matches:?}"
        );
        assert!(
            text_matches
                .iter()
                .any(|record| record.kind == "signature" || record.kind == "snippet"),
            "text_matches: {text_matches:?}"
        );
    }

    #[test]
    fn ranking_ties_are_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        write_source(dir.path(), "src/b.rs", "pub struct SearchThing;\n");
        write_source(dir.path(), "src/a.rs", "pub struct SearchThing;\n");
        let index = build_index(dir.path()).unwrap();

        let matches = search_symbols(&index, "SearchThing", Some("struct"));

        assert_eq!(matches[0].path, PathBuf::from("src/a.rs"));
        assert_eq!(matches[1].path, PathBuf::from("src/b.rs"));
    }
}
