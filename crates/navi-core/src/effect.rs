//! Effect-based permissions: analyse what a tool execution actually affected
//! on disk and make post-execution security decisions.
//!
//! This module provides the [`EffectAnalyzer`] which inspects file paths for
//! sensitivity patterns (`.env`, `Cargo.toml`, `Dockerfile`, CI config, etc.)
//! and produces an [`EffectReport`] with a classified [`BlastRadius`].
//! The resulting [`PostDecision`] feeds into the security policy to escalate
//! guarded effects (e.g. roll back `.env` modifications).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What a tool execution actually affected on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectReport {
    /// Files that were created by the tool.
    pub files_created: Vec<PathBuf>,
    /// Files that were modified by the tool.
    pub files_modified: Vec<PathBuf>,
    /// Files that were deleted by the tool.
    pub files_deleted: Vec<PathBuf>,
    /// Human-readable descriptions of sensitive files affected
    /// (e.g. `"Cargo.toml (dependency)"`, `".env (secret)"`).
    pub key_files_affected: Vec<String>,
    /// Categorisation of how widespread the effect is.
    pub blast_radius: BlastRadius,
}

/// How widespread a tool's effect is.
///
/// Each variant carries a `&'static str` representation matching the
/// instruction spec for serialisation or display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlastRadius {
    /// A single non-sensitive file was touched.
    SingleFile,
    /// Multiple non-sensitive files were touched.
    MultipleFiles,
    /// A dependency manifest was modified (`Cargo.toml`, `package.json`, …).
    DependencyChange,
    /// CI/CD configuration was modified (`.github/`, `ci/`, …).
    CiConfig,
    /// A security-sensitive file was touched (`.env`, credentials, …).
    SecuritySensitive,
}

impl std::fmt::Display for BlastRadius {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SingleFile => write!(f, "single_file"),
            Self::MultipleFiles => write!(f, "multiple_files"),
            Self::DependencyChange => write!(f, "dependency_change"),
            Self::CiConfig => write!(f, "ci_config"),
            Self::SecuritySensitive => write!(f, "security_sensitive"),
        }
    }
}

/// Decision after analysing the effects of a completed tool execution.
///
/// Returned by [`SecurityPolicy::post_execution_effect_check`] to tell the
/// harness what to do about the just-completed tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostDecision {
    /// The effect is acceptable; no further action needed.
    Allow,
    /// The effect should be surfaced to the user for confirmation.
    Ask(String),
    /// The effect is unacceptable; the result should be treated as denied.
    Deny(String),
    /// The effect is problematic; the harness should attempt to roll back.
    Rollback(String),
}

/// Inspects file paths for sensitivity and categorises blast radius.
#[derive(Debug, Clone)]
pub struct EffectAnalyzer;

impl EffectAnalyzer {
    /// Analyse created / modified / deleted paths and produce an effect report.
    ///
    /// `created`, `modified`, and `deleted` are separate sets so the report
    /// can differentiate creation vs. modification when needed.
    pub fn analyze(created: &[PathBuf], modified: &[PathBuf], deleted: &[PathBuf]) -> EffectReport {
        let all_paths: Vec<&PathBuf> = created
            .iter()
            .chain(modified.iter())
            .chain(deleted.iter())
            .collect();

        let key_files_affected = Self::find_sensitive_files(&all_paths);
        let blast_radius = Self::classify_blast_radius(&all_paths, &key_files_affected);

        EffectReport {
            files_created: created.to_vec(),
            files_modified: modified.to_vec(),
            files_deleted: deleted.to_vec(),
            key_files_affected,
            blast_radius,
        }
    }

    /// Check a single path against known sensitivity patterns.
    ///
    /// Returns a human-readable label when the path matches a sensitivity
    /// pattern, or `None` if the path is unremarkable.
    pub fn check_sensitivity(path: &Path) -> Option<String> {
        let file_name = path.file_name()?.to_str()?;
        let path_str = path.to_string_lossy();

        // Security-sensitive files (secrets, credentials).
        if file_name == ".env" {
            return Some(".env (secret)".to_string());
        }
        if file_name == ".env.example" {
            return Some(".env.example (secret template)".to_string());
        }
        if file_name == ".gitignore" {
            return Some(".gitignore (ignore rules)".to_string());
        }

        // Dependency manifests.
        if file_name == "Cargo.toml" {
            return Some("Cargo.toml (dependency)".to_string());
        }
        if file_name == "Cargo.lock" {
            return Some("Cargo.lock (lockfile)".to_string());
        }
        if file_name == "package.json" {
            return Some("package.json (dependency)".to_string());
        }
        if file_name == "package-lock.json" {
            return Some("package-lock.json (lockfile)".to_string());
        }
        if file_name == "yarn.lock" {
            return Some("yarn.lock (lockfile)".to_string());
        }
        if file_name == "pnpm-lock.yaml" {
            return Some("pnpm-lock.yaml (lockfile)".to_string());
        }

        // Container / infrastructure.
        if file_name == "Dockerfile" {
            return Some("Dockerfile (container)".to_string());
        }
        if file_name == "docker-compose.yml" || file_name == "docker-compose.yaml" {
            return Some(format!("{file_name} (container)"));
        }

        // CI/CD configuration.
        if path_str.contains("/.github/") || path_str.starts_with(".github/") {
            return Some(".github/ (CI config)".to_string());
        }
        if path_str.contains("/ci/") || path_str.starts_with("ci/") {
            return Some("ci/ (CI config)".to_string());
        }

        // Package manager / runtime config.
        if matches!(
            file_name.as_ref(),
            ".npmrc" | ".yarnrc" | ".yarnrc.yml" | ".browserslistrc"
        ) {
            return Some(format!("{file_name} (config)"));
        }

        None
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    fn find_sensitive_files(paths: &[&PathBuf]) -> Vec<String> {
        let mut sensitive = Vec::new();
        for path in paths {
            if let Some(label) = Self::check_sensitivity(path) {
                if !sensitive.contains(&label) {
                    sensitive.push(label);
                }
            }
        }
        sensitive
    }

    fn classify_blast_radius(all_paths: &[&PathBuf], key_files: &[String]) -> BlastRadius {
        // Security-sensitive files take highest priority.
        if key_files.iter().any(|l| l.contains("secret")) {
            return BlastRadius::SecuritySensitive;
        }

        // CI configuration changes.
        if key_files.iter().any(|l| l.contains("CI config")) {
            return BlastRadius::CiConfig;
        }

        // Dependency / lockfile changes.
        if key_files
            .iter()
            .any(|l| l.contains("dependency") || l.contains("lockfile"))
        {
            return BlastRadius::DependencyChange;
        }

        // Container config.
        if key_files.iter().any(|l| l.contains("container")) {
            return BlastRadius::DependencyChange;
        }

        match all_paths.len() {
            0 | 1 => BlastRadius::SingleFile,
            _ => BlastRadius::MultipleFiles,
        }
    }
}

/// Extract file paths referenced by a completed tool result and its
/// originating invocation.
///
/// Looks at common fields in both the result output and the invocation input
/// so that the [`EffectAnalyzer`] has paths to inspect.
pub fn extract_paths(
    result: &crate::tool::ToolResult,
    invocation: &crate::tool::ToolInvocation,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();

    // Result output: direct write provides "path".
    if let Some(p) = result
        .output
        .get("path")
        .and_then(serde_json::Value::as_str)
    {
        push_unique_path(&mut paths, PathBuf::from(p));
    }

    // Result output: patch mode provides "affected_paths".
    if let Some(affected) = result
        .output
        .get("affected_paths")
        .and_then(serde_json::Value::as_array)
    {
        for v in affected {
            if let Some(p) = v.as_str() {
                push_unique_path(&mut paths, PathBuf::from(p));
            }
        }
    }

    // Invocation input: "file", "path", or Crush-compatible "file_path".
    for key in &["file", "path", "file_path"] {
        if let Some(p) = invocation
            .input
            .get(*key)
            .and_then(serde_json::Value::as_str)
        {
            push_unique_path(&mut paths, PathBuf::from(p));
        }
    }

    // Result output: files_changed from edit/multiedit/search-replace.
    if let Some(changed) = result
        .output
        .get("files_changed")
        .and_then(serde_json::Value::as_array)
    {
        for v in changed {
            if let Some(p) = v.as_str() {
                push_unique_path(&mut paths, PathBuf::from(p));
            }
        }
    }

    paths
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolInvocation, ToolResult};
    use serde_json::json;
    use std::path::PathBuf;

    // ── Sensitivity detection ────────────────────────────────────────────

    #[test]
    fn analyzer_detects_env_file() {
        let report = EffectAnalyzer::analyze(&[PathBuf::from(".env")], &[], &[]);
        assert!(
            report.key_files_affected.iter().any(|k| k.contains(".env")),
            "expected .env in key_files_affected, got {:?}",
            report.key_files_affected
        );
        assert_eq!(report.blast_radius, BlastRadius::SecuritySensitive);
    }

    #[test]
    fn analyzer_detects_cargo_toml_change() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("Cargo.toml")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("Cargo.toml")),
            "expected Cargo.toml in key_files_affected, got {:?}",
            report.key_files_affected
        );
        assert_eq!(report.blast_radius, BlastRadius::DependencyChange);
    }

    #[test]
    fn analyzer_ignores_src_lib_rs_change() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("src/lib.rs")], &[]);
        assert!(
            report.key_files_affected.is_empty(),
            "expected no key files for src/lib.rs, got {:?}",
            report.key_files_affected
        );
        assert_eq!(report.blast_radius, BlastRadius::SingleFile);
    }

    #[test]
    fn analyzer_detects_package_json() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("package.json")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("package.json")),
            "expected package.json in key_files_affected"
        );
        assert_eq!(report.blast_radius, BlastRadius::DependencyChange);
    }

    #[test]
    fn analyzer_detects_dockerfile() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("Dockerfile")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("Dockerfile")),
            "expected Dockerfile in key_files_affected"
        );
    }

    #[test]
    fn analyzer_detects_gitignore() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from(".gitignore")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains(".gitignore")),
            "expected .gitignore in key_files_affected"
        );
    }

    #[test]
    fn analyzer_detects_github_ci_directory() {
        let report =
            EffectAnalyzer::analyze(&[], &[PathBuf::from(".github/workflows/ci.yml")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("CI config")),
            "expected CI config in key_files_affected, got {:?}",
            report.key_files_affected
        );
        assert_eq!(report.blast_radius, BlastRadius::CiConfig);
    }

    #[test]
    fn analyzer_detects_ci_folder() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("ci/build.sh")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("CI config")),
            "expected CI config for ci/ path"
        );
    }

    #[test]
    fn analyzer_detects_cargo_lock() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("Cargo.lock")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("lockfile")),
            "expected lockfile label for Cargo.lock"
        );
        assert_eq!(report.blast_radius, BlastRadius::DependencyChange);
    }

    #[test]
    fn analyzer_detects_yarn_lock() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from("yarn.lock")], &[]);
        assert!(
            report
                .key_files_affected
                .iter()
                .any(|k| k.contains("lockfile")),
            "expected lockfile label for yarn.lock"
        );
    }

    // ── Sensitive file in deleted set ─────────────────────────────────────

    #[test]
    fn sensitive_file_detected_in_deleted_set() {
        let report = EffectAnalyzer::analyze(&[], &[], &[PathBuf::from(".env")]);
        assert!(
            report.key_files_affected.iter().any(|k| k.contains(".env")),
            "expected .env when it is in the deleted set"
        );
        assert_eq!(report.blast_radius, BlastRadius::SecuritySensitive);
    }

    // ── Multiple files blast radius ───────────────────────────────────────

    #[test]
    fn multiple_files_blast_radius() {
        let report = EffectAnalyzer::analyze(
            &[PathBuf::from("src/main.rs")],
            &[PathBuf::from("src/lib.rs")],
            &[],
        );
        assert_eq!(report.blast_radius, BlastRadius::MultipleFiles);
    }

    // ── PostDecision::Rollback for guarded effects ─────────────────────────

    #[test]
    fn post_decision_rollback_for_guarded_effects() {
        let report = EffectAnalyzer::analyze(&[], &[PathBuf::from(".env")], &[]);

        let decision = if report.blast_radius == BlastRadius::SecuritySensitive {
            PostDecision::Rollback(
                "Modifying .env file -- sensitive secrets may be exposed".to_string(),
            )
        } else {
            PostDecision::Allow
        };

        assert_eq!(
            decision,
            PostDecision::Rollback(
                "Modifying .env file -- sensitive secrets may be exposed".to_string()
            )
        );
    }

    // ── extract_paths from ToolResult ─────────────────────────────────────

    #[test]
    fn extract_paths_from_direct_write_result() {
        let result = ToolResult {
            invocation_id: "i1".into(),
            ok: true,
            output: json!({"path": "src/main.rs", "lines_added": 1}),
        };
        let inv = ToolInvocation {
            id: "i1".into(),
            tool_name: "write".into(),
            input: json!({"path": "src/main.rs", "content": "fn main() {}"}),
        };

        let paths = extract_paths(&result, &inv);
        assert_eq!(paths, vec![PathBuf::from("src/main.rs")]);
    }

    #[test]
    fn extract_paths_from_patch_result() {
        let result = ToolResult {
            invocation_id: "i2".into(),
            ok: true,
            output: json!({
                "method": "structured",
                "affected_paths": ["a.txt", "b.txt"],
            }),
        };
        let inv = ToolInvocation {
            id: "i2".into(),
            tool_name: "write".into(),
            input: json!({"patch": "*** Begin Patch\n*** Add File: a.txt\n+hello\n*** End Patch"}),
        };

        let paths = extract_paths(&result, &inv);
        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&PathBuf::from("a.txt")));
        assert!(paths.contains(&PathBuf::from("b.txt")));
    }

    // ── BlastRadius Display ──────────────────────────────────────────────

    #[test]
    fn blast_radius_display() {
        assert_eq!(BlastRadius::SingleFile.to_string(), "single_file");
        assert_eq!(BlastRadius::MultipleFiles.to_string(), "multiple_files");
        assert_eq!(
            BlastRadius::DependencyChange.to_string(),
            "dependency_change"
        );
        assert_eq!(BlastRadius::CiConfig.to_string(), "ci_config");
        assert_eq!(
            BlastRadius::SecuritySensitive.to_string(),
            "security_sensitive"
        );
    }
}
