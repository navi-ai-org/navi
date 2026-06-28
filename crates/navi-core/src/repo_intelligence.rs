//! Repository intelligence: compact symbol, dependency, and test discovery.
//!
//! This module is intentionally UI-agnostic and deterministic. It gives the
//! harness structured repo facts before falling back to grep-heavy exploration.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    let query = query.to_lowercase();
    let kind = kind.map(str::to_lowercase);
    index
        .symbols
        .iter()
        .filter(|symbol| {
            kind.as_ref()
                .map(|kind| symbol.kind.eq_ignore_ascii_case(kind))
                .unwrap_or(true)
        })
        .filter(|symbol| {
            query.is_empty()
                || symbol.name.to_lowercase().contains(&query)
                || symbol.signature.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

pub fn goto_symbol(index: &RepoIndex, name: &str) -> Option<SymbolRecord> {
    index
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .or_else(|| {
            index
                .symbols
                .iter()
                .find(|symbol| symbol.name.contains(name))
        })
        .cloned()
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
}
