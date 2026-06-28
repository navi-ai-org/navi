//! Structured operational memory entries and retrieval.

use crate::security::redact_secrets;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationalMemoryEntry {
    pub id: String,
    pub scope: MemoryScope,
    pub text: String,
    pub source_trace: Option<String>,
    pub confidence: f64,
    pub expires_at_ms: Option<u64>,
    pub verifier_evidence: Vec<String>,
    pub owner: Option<String>,
    pub files: Vec<PathBuf>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Session,
    Project,
    UserTeam,
    Procedural,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OperationalMemoryStore {
    entries: Vec<OperationalMemoryEntry>,
}

impl OperationalMemoryStore {
    pub fn insert(&mut self, mut entry: OperationalMemoryEntry) {
        entry.text = redact_secrets(&entry.text);
        self.entries.retain(|existing| existing.id != entry.id);
        self.entries.push(entry);
    }

    pub fn retrieve(
        &self,
        task: &str,
        files: &[PathBuf],
        now_ms: u64,
        budget_bytes: usize,
    ) -> Vec<OperationalMemoryEntry> {
        let task = task.to_lowercase();
        let mut scored = self
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .expires_at_ms
                    .map(|expires| now_ms <= expires)
                    .unwrap_or(true)
            })
            .map(|entry| (memory_score(entry, &task, files), entry))
            .filter(|(score, _)| *score > 0.0)
            .collect::<Vec<_>>();
        scored.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });

        let mut used = 0usize;
        let mut out = Vec::new();
        for (_, entry) in scored {
            let bytes = entry.text.len();
            if used.saturating_add(bytes) > budget_bytes {
                continue;
            }
            used += bytes;
            out.push(entry.clone());
        }
        out
    }

    pub fn render_prompt_context(
        &self,
        task: &str,
        files: &[PathBuf],
        now_ms: u64,
        budget_bytes: usize,
    ) -> String {
        self.retrieve(task, files, now_ms, budget_bytes)
            .into_iter()
            .map(|entry| format!("- [{}] {}", scope_label(&entry.scope), entry.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn entries(&self) -> &[OperationalMemoryEntry] {
        &self.entries
    }
}

fn memory_score(entry: &OperationalMemoryEntry, task: &str, files: &[PathBuf]) -> f64 {
    let mut score = entry.confidence.max(0.0);
    let text = entry.text.to_lowercase();
    for term in task.split_whitespace() {
        if text.contains(term) || entry.tags.iter().any(|tag| tag.contains(term)) {
            score += 1.0;
        }
    }
    for file in files {
        if entry.files.iter().any(|candidate| candidate == file) {
            score += 2.0;
        }
    }
    if !entry.verifier_evidence.is_empty() {
        score += 1.0;
    }
    score
}

fn scope_label(scope: &MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Session => "session",
        MemoryScope::Project => "project",
        MemoryScope::UserTeam => "team",
        MemoryScope::Procedural => "procedure",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, text: &str) -> OperationalMemoryEntry {
        OperationalMemoryEntry {
            id: id.to_string(),
            scope: MemoryScope::Procedural,
            text: text.to_string(),
            source_trace: None,
            confidence: 1.0,
            expires_at_ms: None,
            verifier_evidence: vec!["just test".to_string()],
            owner: None,
            files: vec![PathBuf::from("src/lib.rs")],
            tags: vec!["rust".to_string()],
        }
    }

    #[test]
    fn retrieves_by_task_file_and_budget() {
        let mut store = OperationalMemoryStore::default();
        store.insert(entry(
            "a",
            "Run just test-crate navi-core for Rust core changes",
        ));
        store.insert(entry("b", "Unrelated note"));

        let found = store.retrieve("rust core", &[PathBuf::from("src/lib.rs")], 0, 200);

        assert_eq!(found[0].id, "a");
    }

    #[test]
    fn redacts_secret_like_memory_text() {
        let mut store = OperationalMemoryStore::default();
        store.insert(entry("secret", "token = sk-12345678901234567890"));

        assert!(!store.entries()[0].text.contains("sk-123"));
    }

    #[test]
    fn injection_budget_skips_large_entries_instead_of_stopping() {
        let mut store = OperationalMemoryStore::default();
        store.insert(entry("large", "rust ".repeat(200).as_str()));
        store.insert(entry(
            "small",
            "rust workflow: run just test-crate navi-core",
        ));

        let rendered = store.render_prompt_context("rust", &[], 0, 80);

        assert!(rendered.contains("rust workflow"));
        assert!(!rendered.contains("rust rust rust rust rust rust"));
    }
}
