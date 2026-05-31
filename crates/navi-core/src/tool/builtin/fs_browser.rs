use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const FS_MAX_DEPTH: u64 = 10;
const FS_MAX_RESULTS: usize = 500;

pub(crate) struct FsBrowserTool;

#[async_trait]
impl Tool for FsBrowserTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "fs_browser",
            "Browse the project filesystem. Actions: list (directory contents), tree (directory tree), find (search by pattern), stat (file metadata).",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "tree", "find", "stat"],
                        "description": "Operation to perform. list: flat file listing. tree: directory tree. find: search by pattern. stat: file metadata."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory or file path. Defaults to current project root."
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Substring filter for list/find actions. Matches against file names."
                    },
                    "depth": {
                        "type": "integer",
                        "description": "Maximum directory depth for tree action. Defaults to 3."
                    },
                    "hidden": {
                        "type": "boolean",
                        "description": "Include dotfiles and hidden directories. Defaults to false."
                    }
                },
                "required": ["action"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let action = helpers::required_string(&invocation.input, "action")?.to_string();
        let path =
            helpers::optional_string(&invocation.input, "path").unwrap_or_else(|| ".".to_string());
        let pattern = helpers::optional_string(&invocation.input, "pattern");
        let depth = helpers::optional_u64(&invocation.input, "depth")
            .unwrap_or(3)
            .min(FS_MAX_DEPTH);
        let hidden = helpers::optional_bool(&invocation.input, "hidden").unwrap_or(false);

        match action.as_str() {
            "list" => action_list(&invocation.id, &path, pattern.as_deref(), hidden).await,
            "tree" => action_tree(&invocation.id, &path, depth, hidden).await,
            "find" => action_find(&invocation.id, &path, pattern.as_deref(), hidden).await,
            "stat" => action_stat(&invocation.id, &path),
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_fs_action",
                    format!("unknown fs_browser action: {action}"),
                    true,
                    Some("Use list, tree, find, or stat."),
                    None,
                ),
            }),
        }
    }
}

async fn action_list(
    invocation_id: &str,
    root: &str,
    pattern: Option<&str>,
    hidden: bool,
) -> Result<ToolResult> {
    let root_path = Path::new(root).to_path_buf();
    let config = CollectConfig {
        pattern: pattern.map(|s| s.to_string()),
        hidden,
        max_results: FS_MAX_RESULTS,
    };
    let files = tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        collect_files_recursive(&root_path, &config, 0, u64::MAX, &mut files);
        files
    })
    .await
    .map_err(|e| anyhow::anyhow!("task join error: {e}"))?;

    let truncated = files.len() >= FS_MAX_RESULTS;
    Ok(helpers::ok(
        invocation_id.to_string(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "path": root,
            "files": files,
            "total": files.len(),
            "truncated": truncated,
        }),
    ))
}

async fn action_tree(
    invocation_id: &str,
    root: &str,
    depth: u64,
    hidden: bool,
) -> Result<ToolResult> {
    let root_path = Path::new(root).to_path_buf();
    let entries = tokio::task::spawn_blocking(move || build_tree(&root_path, depth, hidden, 0))
        .await
        .map_err(|e| anyhow::anyhow!("task join error: {e}"))?;

    Ok(helpers::ok(
        invocation_id.to_string(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "path": root,
            "entries": entries,
            "total": count_entries(&entries),
        }),
    ))
}

async fn action_find(
    invocation_id: &str,
    root: &str,
    pattern: Option<&str>,
    hidden: bool,
) -> Result<ToolResult> {
    let root_path = Path::new(root).to_path_buf();
    let config = CollectConfig {
        pattern: pattern.map(|s| s.to_string()),
        hidden,
        max_results: FS_MAX_RESULTS,
    };
    let files = tokio::task::spawn_blocking(move || {
        let mut files = Vec::new();
        collect_files_recursive(&root_path, &config, 0, u64::MAX, &mut files);
        files
    })
    .await
    .map_err(|e| anyhow::anyhow!("task join error: {e}"))?;

    let truncated = files.len() >= FS_MAX_RESULTS;
    Ok(helpers::ok(
        invocation_id.to_string(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "path": root,
            "files": files,
            "total": files.len(),
            "truncated": truncated,
        }),
    ))
}

fn action_stat(invocation_id: &str, path: &str) -> Result<ToolResult> {
    let p = Path::new(path);
    let meta = fs::metadata(p).with_context(|| format!("failed to stat {path}"))?;

    let file_type = if meta.is_dir() {
        "dir"
    } else if meta.is_file() {
        "file"
    } else if meta.is_symlink() {
        "symlink"
    } else {
        "other"
    };

    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    let permissions = format!("{:o}", unix_permissions(&meta));

    Ok(helpers::ok(
        invocation_id.to_string(),
        json!({
            "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
            "path": path,
            "type": file_type,
            "size": meta.len(),
            "modified": modified,
            "permissions": permissions,
        }),
    ))
}

#[cfg(unix)]
fn unix_permissions(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode()
}

#[cfg(not(unix))]
fn unix_permissions(_meta: &fs::Metadata) -> u32 {
    0
}

struct CollectConfig {
    pattern: Option<String>,
    hidden: bool,
    max_results: usize,
}

fn collect_files_recursive(
    root: &Path,
    config: &CollectConfig,
    depth: u64,
    max_depth: u64,
    files: &mut Vec<String>,
) {
    if files.len() >= config.max_results || depth > max_depth {
        return;
    }

    if !root.exists() {
        return;
    }

    if root.is_file() {
        let display = root.display().to_string();
        if matches_pattern(&display, config.pattern.as_deref()) {
            files.push(display);
        }
        return;
    }

    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if files.len() >= config.max_results {
            break;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if !config.hidden && name_str.starts_with('.') {
            continue;
        }

        if should_skip_dir(&name_str) {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, config, depth + 1, max_depth, files);
        } else {
            let display = path.display().to_string();
            if matches_pattern(&display, config.pattern.as_deref()) {
                files.push(display);
            }
        }
    }
}

fn build_tree(root: &Path, max_depth: u64, hidden: bool, depth: u64) -> Vec<Value> {
    let mut entries = Vec::new();

    if !root.is_dir() || depth > max_depth {
        return entries;
    }

    let dir_entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return entries,
    };

    for entry in dir_entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();

        if !hidden && name_str.starts_with('.') {
            continue;
        }

        if should_skip_dir(&name_str) {
            continue;
        }

        let path = entry.path();
        let meta = fs::metadata(&path).ok();

        if path.is_dir() {
            let children = if depth < max_depth {
                build_tree(&path, max_depth, hidden, depth + 1)
            } else {
                Vec::new()
            };
            let child_count = children.len();
            entries.push(json!({
                "name": name_str,
                "type": "dir",
                "children": child_count,
                "entries": children,
            }));
        } else {
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            entries.push(json!({
                "name": name_str,
                "type": "file",
                "size": size,
            }));
        }
    }

    entries
}

fn count_entries(entries: &[Value]) -> usize {
    let mut count = 0;
    for entry in entries {
        count += 1;
        if let Some(children) = entry.get("entries").and_then(|v| v.as_array()) {
            count += count_entries(children);
        }
    }
    count
}

fn matches_pattern(name: &str, pattern: Option<&str>) -> bool {
    match pattern {
        Some(p) => name.contains(p),
        None => true,
    }
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".cache" | ".venv" | "__pycache__"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── matches_pattern ────────────────────────────────────────────────────

    #[test]
    fn matches_pattern_none_matches_everything() {
        assert!(matches_pattern("anything.txt", None));
    }

    #[test]
    fn matches_pattern_substring_match() {
        assert!(matches_pattern("/path/to/readme.md", Some(".md")));
    }

    #[test]
    fn matches_pattern_no_match() {
        assert!(!matches_pattern("/path/to/file.rs", Some(".md")));
    }

    #[test]
    fn matches_pattern_case_sensitive() {
        assert!(!matches_pattern("README.MD", Some(".md")));
    }

    // ── should_skip_dir ────────────────────────────────────────────────────

    #[test]
    fn should_skip_git() {
        assert!(should_skip_dir(".git"));
    }

    #[test]
    fn should_skip_target() {
        assert!(should_skip_dir("target"));
    }

    #[test]
    fn should_skip_node_modules() {
        assert!(should_skip_dir("node_modules"));
    }

    #[test]
    fn should_skip_cache_dirs() {
        assert!(should_skip_dir(".cache"));
        assert!(should_skip_dir(".venv"));
        assert!(should_skip_dir("__pycache__"));
    }

    #[test]
    fn should_not_skip_normal_dirs() {
        assert!(!should_skip_dir("src"));
        assert!(!should_skip_dir("tests"));
        assert!(!should_skip_dir("docs"));
    }

    #[test]
    fn should_not_skip_similar_names() {
        assert!(!should_skip_dir("target_debug"));
        assert!(!should_skip_dir("my.git"));
        assert!(!should_skip_dir("node_modules_backup"));
    }

    // ── count_entries ──────────────────────────────────────────────────────

    #[test]
    fn count_entries_empty() {
        assert_eq!(count_entries(&[]), 0);
    }

    #[test]
    fn count_entries_flat() {
        let entries = vec![
            json!({"name": "a", "type": "file"}),
            json!({"name": "b", "type": "file"}),
        ];
        assert_eq!(count_entries(&entries), 2);
    }

    #[test]
    fn count_entries_nested() {
        let entries = vec![
            json!({
                "name": "dir",
                "type": "dir",
                "entries": [
                    json!({"name": "child1", "type": "file"}),
                    json!({"name": "child2", "type": "file"}),
                ]
            }),
            json!({"name": "root_file", "type": "file"}),
        ];
        // 1 (dir) + 2 (children) + 1 (root_file) = 4
        assert_eq!(count_entries(&entries), 4);
    }

    #[test]
    fn count_entries_deeply_nested() {
        let entries = vec![json!({
            "name": "a",
            "type": "dir",
            "entries": [json!({
                "name": "b",
                "type": "dir",
                "entries": [json!({"name": "c", "type": "file"})]
            })]
        })];
        assert_eq!(count_entries(&entries), 3);
    }

    // ── collect_files_recursive ────────────────────────────────────────────

    #[test]
    fn collect_files_finds_all_files() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("a.txt"), "a").unwrap();
        std::fs::write(tempdir.path().join("b.rs"), "b").unwrap();
        std::fs::create_dir(tempdir.path().join("sub")).unwrap();
        std::fs::write(tempdir.path().join("sub/c.txt"), "c").unwrap();

        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 3);
    }

    #[test]
    fn collect_files_with_pattern_filter() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("readme.md"), "md").unwrap();
        std::fs::write(tempdir.path().join("code.rs"), "rs").unwrap();
        std::fs::write(tempdir.path().join("notes.md"), "md").unwrap();

        let config = CollectConfig {
            pattern: Some(".md".to_string()),
            hidden: false,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.ends_with(".md")));
    }

    #[test]
    fn collect_files_skips_hidden_by_default() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("visible.txt"), "v").unwrap();
        std::fs::create_dir(tempdir.path().join(".hidden")).unwrap();
        std::fs::write(tempdir.path().join(".hidden/secret.txt"), "s").unwrap();

        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("visible.txt"));
    }

    #[test]
    fn collect_files_includes_hidden_when_enabled() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("visible.txt"), "v").unwrap();
        std::fs::create_dir(tempdir.path().join(".hidden")).unwrap();
        std::fs::write(tempdir.path().join(".hidden/secret.txt"), "s").unwrap();

        let config = CollectConfig {
            pattern: None,
            hidden: true,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn collect_files_skips_build_dirs() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("src.rs"), "s").unwrap();
        std::fs::create_dir(tempdir.path().join("target")).unwrap();
        std::fs::write(tempdir.path().join("target/out"), "o").unwrap();
        std::fs::create_dir(tempdir.path().join("node_modules")).unwrap();
        std::fs::write(tempdir.path().join("node_modules/pkg"), "p").unwrap();

        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("src.rs"));
    }

    #[test]
    fn collect_files_respects_max_results() {
        let tempdir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(tempdir.path().join(format!("file{i}.txt")), "").unwrap();
        }

        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 3,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 3);
    }

    // ── build_tree ─────────────────────────────────────────────────────────

    #[test]
    fn build_tree_returns_files_and_dirs() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("file.txt"), "f").unwrap();
        std::fs::create_dir(tempdir.path().join("sub")).unwrap();
        std::fs::write(tempdir.path().join("sub/child.txt"), "c").unwrap();

        let entries = build_tree(tempdir.path(), 3, false, 0);
        assert!(entries.len() >= 2);

        let file_entry = entries.iter().find(|e| e["name"] == "file.txt").unwrap();
        assert_eq!(file_entry["type"], "file");

        let dir_entry = entries.iter().find(|e| e["name"] == "sub").unwrap();
        assert_eq!(dir_entry["type"], "dir");
        let children = dir_entry["entries"].as_array().unwrap();
        assert!(children.iter().any(|c| c["name"] == "child.txt"));
    }

    #[test]
    fn build_tree_respects_max_depth() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("a/b/c")).unwrap();
        std::fs::write(tempdir.path().join("a/b/c/deep.txt"), "d").unwrap();

        let entries = build_tree(tempdir.path(), 1, false, 0);
        let a_entry = entries.iter().find(|e| e["name"] == "a").unwrap();
        let a_children = a_entry["entries"].as_array().unwrap();
        let b_entry = a_children.iter().find(|e| e["name"] == "b").unwrap();
        // At depth 1, b should have no entries (would need depth 2+)
        assert!(
            b_entry.get("entries").is_none() || b_entry["entries"].as_array().unwrap().is_empty()
        );
    }

    #[test]
    fn build_tree_skips_hidden_by_default() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("visible.txt"), "v").unwrap();
        std::fs::create_dir(tempdir.path().join(".git")).unwrap();
        std::fs::write(tempdir.path().join(".git/config"), "c").unwrap();

        let entries = build_tree(tempdir.path(), 3, false, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "visible.txt");
    }

    #[test]
    fn build_tree_includes_hidden_when_enabled() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::write(tempdir.path().join("visible.txt"), "v").unwrap();
        std::fs::create_dir(tempdir.path().join(".hidden")).unwrap();

        let entries = build_tree(tempdir.path(), 3, true, 0);
        assert_eq!(entries.len(), 2);
    }

    // ── Mutation-killing: collect_files_recursive boundary ────────────────

    #[test]
    fn collect_files_exact_max_not_truncated() {
        let tempdir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            std::fs::write(tempdir.path().join(format!("f{i}.txt")), "").unwrap();
        }
        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 5,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, u64::MAX, &mut files);
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn collect_files_stops_at_max_depth() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("a/b/c")).unwrap();
        std::fs::write(tempdir.path().join("a/b/c/deep.txt"), "d").unwrap();
        let config = CollectConfig {
            pattern: None,
            hidden: false,
            max_results: 500,
        };
        let mut files = Vec::new();
        collect_files_recursive(tempdir.path(), &config, 0, 1, &mut files);
        // At depth 1, should find a/ but not recurse into a/b/c
        assert_eq!(files.len(), 0);
    }

    // ── Mutation-killing: build_tree depth boundary ───────────────────────

    #[test]
    fn build_tree_at_exact_depth_returns_empty_children() {
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tempdir.path().join("a/b")).unwrap();
        std::fs::write(tempdir.path().join("a/b/file.txt"), "f").unwrap();
        // depth=1: a/ should have children, but b/ at depth=1 should not
        let entries = build_tree(tempdir.path(), 1, false, 0);
        let a = entries.iter().find(|e| e["name"] == "a").unwrap();
        let a_children = a["entries"].as_array().unwrap();
        let b = a_children.iter().find(|e| e["name"] == "b").unwrap();
        assert!(b["entries"].as_array().unwrap().is_empty());
    }

    // ── Mutation-killing: unix_permissions ────────────────────────────────

    #[test]
    fn unix_permissions_returns_nonzero_for_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test.txt");
        std::fs::write(&path, "content").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let perms = unix_permissions(&meta);
        // Regular file should have some permissions (not 0)
        assert!(perms > 0);
    }
}
