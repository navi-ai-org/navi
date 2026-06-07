use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

const GIT_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitFileChange {
    file: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitStatusOutput {
    branch: String,
    ahead: u64,
    behind: u64,
    staged: Vec<GitFileChange>,
    modified: Vec<GitFileChange>,
    untracked: Vec<String>,
    conflicts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitDiffFile {
    file: String,
    additions: u64,
    deletions: u64,
    binary: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitDiffStats {
    additions: u64,
    deletions: u64,
    files_changed: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitDiffOutput {
    files: Vec<GitDiffFile>,
    stats: GitDiffStats,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitCommit {
    hash: String,
    author: String,
    date: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitLogOutput {
    commits: Vec<GitCommit>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitBranchInfo {
    name: String,
    hash: String,
    message: String,
    current: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitBranchOutput {
    current: String,
    branches: Vec<GitBranchInfo>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitRemoteInfo {
    name: String,
    url: String,
    r#type: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct GitRemoteOutput {
    remotes: Vec<GitRemoteInfo>,
}

pub(crate) struct GitOpsTool {
    project_root: PathBuf,
}

impl GitOpsTool {
    pub(crate) fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[async_trait]
impl Tool for GitOpsTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "git_ops",
            "Run git operations with structured output. Read-only commands (status, diff, log, branch) bypass approval. Destructive commands (stash, remote, push, rm, checkout, reset) require approval.",
            ToolKind::Command,
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["status", "diff", "log", "branch", "stash", "remote"],
                        "description": "Git operation to perform."
                    },
                    "args": {
                        "type": "string",
                        "description": "Extra arguments for the git command (e.g. specific file for diff, branch name for log)."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["json", "text"],
                        "description": "Output format. Defaults to json."
                    }
                },
                "required": ["command"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        let command = helpers::required_string(&invocation.input, "command")?.to_string();
        let args = helpers::optional_string(&invocation.input, "args");
        let format = helpers::optional_string(&invocation.input, "format")
            .unwrap_or_else(|| "json".to_string());

        match command.as_str() {
            "status" => git_status(&self.project_root, &invocation.id, args.as_deref()).await,
            "diff" => git_diff(&self.project_root, &invocation.id, args.as_deref(), &format).await,
            "log" => git_log(&self.project_root, &invocation.id, args.as_deref()).await,
            "branch" => git_branch(&self.project_root, &invocation.id, args.as_deref()).await,
            "stash" => git_stash(&self.project_root, &invocation.id, args.as_deref()).await,
            "remote" => git_remote(&self.project_root, &invocation.id, args.as_deref()).await,
            _ => Ok(ToolResult {
                invocation_id: invocation.id,
                ok: false,
                output: helpers::tool_error(
                    "unknown_git_command",
                    format!("unknown git_ops command: {command}"),
                    true,
                    Some("Use one of: status, diff, log, branch, stash, remote."),
                    None,
                ),
            }),
        }
    }
}

async fn run_git(project_root: &Path, args: &[&str]) -> Result<(bool, String, String)> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.current_dir(project_root);
    for arg in args {
        cmd.arg(arg);
    }
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to run git")?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    Ok((output.status.success(), stdout, stderr))
}

async fn git_status(
    project_root: &Path,
    invocation_id: &str,
    _args: Option<&str>,
) -> Result<ToolResult> {
    let (ok, stdout, stderr) = run_git(
        project_root,
        &["status", "--porcelain=v2", "-z", "--branch"],
    )
    .await?;

    if !ok {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "git_status_failed",
                "git status failed",
                true,
                Some("Run git status manually for the raw error."),
                Some(stderr),
            ),
        });
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(parse_git_status_porcelain_v2(&stdout)),
    ))
}

fn parse_git_status_porcelain_v2(stdout: &str) -> GitStatusOutput {
    let mut branch = String::new();
    let mut ahead = 0u64;
    let mut behind = 0u64;
    let mut staged: Vec<GitFileChange> = Vec::new();
    let mut modified: Vec<GitFileChange> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();

    let records: Vec<&str> = if stdout.contains('\0') {
        stdout
            .split('\0')
            .flat_map(str::lines)
            .filter(|record| !record.is_empty())
            .collect()
    } else {
        stdout.lines().filter(|record| !record.is_empty()).collect()
    };

    let mut index = 0;
    while index < records.len() {
        let line = records[index];
        if line.starts_with("# branch.head ") {
            branch = line
                .strip_prefix("# branch.head ")
                .unwrap_or("")
                .to_string();
        } else if line.starts_with("# branch.ab ") {
            let ab = line.strip_prefix("# branch.ab ").unwrap_or("");
            let parts: Vec<&str> = ab.split_whitespace().collect();
            if parts.len() >= 2 {
                ahead = parts[0].trim_start_matches('+').parse().unwrap_or(0);
                behind = parts[1].trim_start_matches('-').parse().unwrap_or(0);
            }
        } else if line.starts_with("1 ") {
            // Staged change: "1 <XY> ... <path>"
            let parts: Vec<&str> = line.splitn(9, ' ').collect();
            if parts.len() >= 9 {
                let xy = parts[1];
                let path = parts[8];
                staged.push(GitFileChange {
                    file: path.to_string(),
                    status: xy_status(xy),
                    previous_file: None,
                });
            }
        } else if line.starts_with("2 ") {
            // Rename/copy: "2 <XY> ... <score> <path>[\t<orig-path>]"
            let parts: Vec<&str> = line.splitn(10, ' ').collect();
            if parts.len() >= 10 {
                let xy = parts[1];
                let mut paths = parts[9].splitn(2, '\t');
                let path = paths.next().unwrap_or_default();
                let previous_file = paths.next().map(str::to_string).or_else(|| {
                    records.get(index + 1).and_then(|next| {
                        (!is_status_record(next)).then(|| {
                            index += 1;
                            (*next).to_string()
                        })
                    })
                });
                modified.push(GitFileChange {
                    file: path.to_string(),
                    status: xy_status(xy),
                    previous_file,
                });
            }
        } else if line.starts_with("? ") {
            // Untracked: "? <path>"
            if let Some(path) = line.strip_prefix("? ") {
                untracked.push(path.to_string());
            }
        } else if line.starts_with("u ") {
            // Conflict: "u <XY> ... <path>"
            let parts: Vec<&str> = line.splitn(11, ' ').collect();
            if parts.len() >= 11 {
                conflicts.push(parts[10].to_string());
            }
        }
        index += 1;
    }

    GitStatusOutput {
        branch,
        ahead,
        behind,
        staged,
        modified,
        untracked,
        conflicts,
    }
}

fn is_status_record(record: &str) -> bool {
    record.starts_with("# ")
        || record.starts_with("1 ")
        || record.starts_with("2 ")
        || record.starts_with("u ")
        || record.starts_with("? ")
        || record.starts_with("! ")
}

async fn git_diff(
    project_root: &Path,
    invocation_id: &str,
    args: Option<&str>,
    format: &str,
) -> Result<ToolResult> {
    let mut git_args = vec!["diff", "--numstat"];
    let extra;
    if let Some(a) = args {
        extra = a.to_string();
        git_args.push(&extra);
    }

    let (ok, stdout, stderr) = run_git(project_root, &git_args).await?;

    if !ok {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "git_diff_failed",
                "git diff failed",
                true,
                Some("Check that the requested path/revision exists."),
                Some(stderr),
            ),
        });
    }

    if format == "text" {
        // Get full diff text
        let mut text_args = vec!["diff"];
        let extra_text;
        if let Some(a) = args {
            extra_text = a.to_string();
            text_args.push(&extra_text);
        }
        let (_, text_stdout, _) = run_git(project_root, &text_args).await?;
        return Ok(helpers::ok(
            invocation_id.to_string(),
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "diff": helpers::truncate_string(text_stdout, GIT_OUTPUT_LIMIT_BYTES),
            }),
        ));
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(parse_git_diff_numstat(&stdout)),
    ))
}

fn parse_git_diff_numstat(stdout: &str) -> GitDiffOutput {
    let mut files: Vec<GitDiffFile> = Vec::new();
    let mut total_additions = 0u64;
    let mut total_deletions = 0u64;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let binary = parts[0] == "-" || parts[1] == "-";
            let additions: u64 = parts[0].parse().unwrap_or(0);
            let deletions: u64 = parts[1].parse().unwrap_or(0);
            let file = parts[2];
            total_additions += additions;
            total_deletions += deletions;
            files.push(GitDiffFile {
                file: file.to_string(),
                additions,
                deletions,
                binary,
            });
        }
    }

    let files_changed = files.len();
    GitDiffOutput {
        files,
        stats: GitDiffStats {
            additions: total_additions,
            deletions: total_deletions,
            files_changed,
        },
    }
}

async fn git_log(
    project_root: &Path,
    invocation_id: &str,
    args: Option<&str>,
) -> Result<ToolResult> {
    let format_str = "%H|%an|%ai|%s";
    let mut git_args = vec!["log", &format_str, "--format", format_str, "-n", "20"];
    let extra;
    if let Some(a) = args {
        extra = a.to_string();
        git_args.push(&extra);
    }

    let (ok, stdout, stderr) = run_git(project_root, &git_args).await?;

    if !ok {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "git_log_failed",
                "git log failed",
                true,
                None,
                Some(stderr),
            ),
        });
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(parse_git_log_output(&stdout)),
    ))
}

fn parse_git_log_output(stdout: &str) -> GitLogOutput {
    let mut commits: Vec<GitCommit> = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(4, '|').collect();
        if parts.len() >= 4 {
            commits.push(GitCommit {
                hash: parts[0].to_string(),
                author: parts[1].to_string(),
                date: parts[2].to_string(),
                message: parts[3].to_string(),
            });
        }
    }

    GitLogOutput { commits }
}

async fn git_branch(
    project_root: &Path,
    invocation_id: &str,
    args: Option<&str>,
) -> Result<ToolResult> {
    let mut git_args = vec!["branch", "-v", "--no-color"];
    let extra;
    if let Some(a) = args {
        extra = a.to_string();
        git_args.push(&extra);
    }

    let (ok, stdout, stderr) = run_git(project_root, &git_args).await?;

    if !ok {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "git_branch_failed",
                "git branch failed",
                true,
                None,
                Some(stderr),
            ),
        });
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(parse_git_branch_output(&stdout)),
    ))
}

fn parse_git_branch_output(stdout: &str) -> GitBranchOutput {
    let mut branches: Vec<GitBranchInfo> = Vec::new();
    let mut current_branch = String::new();

    for line in stdout.lines() {
        let is_current = line.starts_with('*');
        let trimmed = line.trim_start_matches('*').trim_start();
        let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            let name = parts[0].to_string();
            let hash = parts[1].to_string();
            let message = parts.get(2).unwrap_or(&"").to_string();
            if is_current {
                current_branch = name.clone();
            }
            branches.push(GitBranchInfo {
                name,
                hash,
                message,
                current: is_current,
            });
        }
    }

    GitBranchOutput {
        current: current_branch,
        branches,
    }
}

async fn git_stash(
    project_root: &Path,
    invocation_id: &str,
    args: Option<&str>,
) -> Result<ToolResult> {
    let mut git_args = vec!["stash"];
    let extra;
    if let Some(a) = args {
        extra = a.to_string();
        git_args.push(&extra);
    }

    let (ok, stdout, stderr) = run_git(project_root, &git_args).await?;

    Ok(ToolResult {
        invocation_id: invocation_id.to_string(),
        ok,
        output: if ok {
            json!({
                "schema_version": helpers::SPECIALIZED_SCHEMA_VERSION,
                "status": "success",
                "output": stdout.trim(),
            })
        } else {
            helpers::tool_error(
                "git_stash_failed",
                stderr.trim(),
                true,
                None,
                Some(stderr.clone()),
            )
        },
    })
}

async fn git_remote(
    project_root: &Path,
    invocation_id: &str,
    args: Option<&str>,
) -> Result<ToolResult> {
    let mut git_args = vec!["remote", "-v"];
    let extra;
    if let Some(a) = args {
        extra = a.to_string();
        git_args.push(&extra);
    }

    let (ok, stdout, stderr) = run_git(project_root, &git_args).await?;

    if !ok {
        return Ok(ToolResult {
            invocation_id: invocation_id.to_string(),
            ok: false,
            output: helpers::tool_error(
                "git_remote_failed",
                "git remote failed",
                true,
                None,
                Some(stderr),
            ),
        });
    }

    Ok(helpers::ok(
        invocation_id.to_string(),
        helpers::versioned(parse_git_remote_output(&stdout)),
    ))
}

fn parse_git_remote_output(stdout: &str) -> GitRemoteOutput {
    let mut remotes: Vec<GitRemoteInfo> = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            remotes.push(GitRemoteInfo {
                name: parts[0].to_string(),
                url: parts[1].to_string(),
                r#type: parts.get(2).unwrap_or(&"").to_string(),
            });
        }
    }

    GitRemoteOutput { remotes }
}

fn xy_status(xy: &str) -> &'static str {
    match xy {
        "M." => "modified",
        ".M" => "modified",
        "MM" => "modified",
        "A." => "added",
        ".A" => "added",
        "D." => "deleted",
        ".D" => "deleted",
        "R." => "renamed",
        ".R" => "renamed",
        "C." => "copied",
        ".C" => "copied",
        "??" => "untracked",
        "!!" => "ignored",
        _ => "changed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── xy_status ──────────────────────────────────────────────────────────

    #[test]
    fn xy_status_modified_staged() {
        assert_eq!(xy_status("M."), "modified");
    }

    #[test]
    fn xy_status_modified_unstaged() {
        assert_eq!(xy_status(".M"), "modified");
    }

    #[test]
    fn xy_status_modified_both() {
        assert_eq!(xy_status("MM"), "modified");
    }

    #[test]
    fn xy_status_added_staged() {
        assert_eq!(xy_status("A."), "added");
    }

    #[test]
    fn xy_status_added_unstaged() {
        assert_eq!(xy_status(".A"), "added");
    }

    #[test]
    fn xy_status_deleted_staged() {
        assert_eq!(xy_status("D."), "deleted");
    }

    #[test]
    fn xy_status_deleted_unstaged() {
        assert_eq!(xy_status(".D"), "deleted");
    }

    #[test]
    fn xy_status_renamed() {
        assert_eq!(xy_status("R."), "renamed");
        assert_eq!(xy_status(".R"), "renamed");
    }

    #[test]
    fn xy_status_copied() {
        assert_eq!(xy_status("C."), "copied");
        assert_eq!(xy_status(".C"), "copied");
    }

    #[test]
    fn xy_status_untracked() {
        assert_eq!(xy_status("??"), "untracked");
    }

    #[test]
    fn xy_status_ignored() {
        assert_eq!(xy_status("!!"), "ignored");
    }

    #[test]
    fn xy_status_unknown_falls_back_to_changed() {
        assert_eq!(xy_status("UU"), "changed");
        assert_eq!(xy_status("AA"), "changed");
    }

    // ── parse git status porcelain v2 inline ───────────────────────────────

    #[test]
    fn parse_status_branch_info() {
        let stdout = "# branch.head main\n# branch.ab +2 -1\n";
        let mut branch = String::new();
        let mut ahead = 0u64;
        let mut behind = 0u64;
        for line in stdout.lines() {
            if line.starts_with("# branch.head ") {
                branch = line
                    .strip_prefix("# branch.head ")
                    .unwrap_or("")
                    .to_string();
            } else if line.starts_with("# branch.ab ") {
                let ab = line.strip_prefix("# branch.ab ").unwrap_or("");
                let parts: Vec<&str> = ab.split_whitespace().collect();
                ahead = parts[0].trim_start_matches('+').parse().unwrap_or(0);
                behind = parts[1].trim_start_matches('-').parse().unwrap_or(0);
            }
        }
        assert_eq!(branch, "main");
        assert_eq!(ahead, 2);
        assert_eq!(behind, 1);
    }

    #[test]
    fn parse_status_staged_change() {
        let stdout = "1 M. N... 100644 100644 100644 abc1234 def5678 src/main.rs";
        let mut staged = Vec::new();
        for line in stdout.lines() {
            if line.starts_with("1 ") {
                let parts: Vec<&str> = line.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    staged.push(json!({
                        "file": parts[8],
                        "status": xy_status(parts[1]),
                    }));
                }
            }
        }
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0]["file"], "src/main.rs");
        assert_eq!(staged[0]["status"], "modified");
    }

    #[test]
    fn parse_status_unstaged_change() {
        let stdout = "2 .M N... 100644 100644 100644 abc1234 def5678 src/lib.rs";
        let mut modified = Vec::new();
        for line in stdout.lines() {
            if line.starts_with("2 ") {
                let parts: Vec<&str> = line.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    modified.push(json!({
                        "file": parts[8],
                        "status": xy_status(parts[1]),
                    }));
                }
            }
        }
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0]["file"], "src/lib.rs");
        assert_eq!(modified[0]["status"], "modified");
    }

    #[test]
    fn parse_status_untracked_file() {
        let stdout = "? new_file.txt\n";
        let mut untracked = Vec::new();
        for line in stdout.lines() {
            if let Some(path) = line.strip_prefix("? ") {
                untracked.push(path.to_string());
            }
        }
        assert_eq!(untracked, vec!["new_file.txt"]);
    }

    #[test]
    fn parse_status_porcelain_z_handles_paths_with_spaces() {
        let stdout = "# branch.head main\x00# branch.ab +1 -2\x001 .M N... 100644 100644 100644 abc123 def456 src/file with space.rs\x00? new file.txt\x00";
        let parsed = parse_git_status_porcelain_v2(stdout);
        assert_eq!(parsed.branch, "main");
        assert_eq!(parsed.ahead, 1);
        assert_eq!(parsed.behind, 2);
        assert_eq!(parsed.staged[0].file, "src/file with space.rs");
        assert_eq!(parsed.untracked, vec!["new file.txt"]);
    }

    #[test]
    fn parse_status_porcelain_z_handles_rename_previous_path() {
        let stdout = "2 R. N... 100644 100644 100644 abc123 def456 R100 new name.rs\0old name.rs\0";
        let parsed = parse_git_status_porcelain_v2(stdout);
        assert_eq!(parsed.modified[0].file, "new name.rs");
        assert_eq!(
            parsed.modified[0].previous_file.as_deref(),
            Some("old name.rs")
        );
        assert_eq!(parsed.modified[0].status, "renamed");
    }

    #[test]
    fn parse_status_conflict() {
        // Porcelain v2 conflict lines use spaces as field separators
        let stdout = "u UU N... 100644 100644 100644 100644 abc123 def456 789abc conflicted.rs\n";
        let mut conflicts = Vec::new();
        for line in stdout.lines() {
            if line.starts_with("u ") {
                let parts: Vec<&str> = line.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    // parts[8] contains everything after the 8th space
                    let path = parts[8].split_whitespace().last().unwrap_or(parts[8]);
                    conflicts.push(path.to_string());
                }
            }
        }
        assert_eq!(conflicts, vec!["conflicted.rs"]);
    }

    // ── parse git diff numstat inline ──────────────────────────────────────

    #[test]
    fn parse_diff_numstat() {
        let stdout = "10\t5\tsrc/main.rs\n3\t0\tREADME.md\n";
        let mut files = Vec::new();
        let mut total_add = 0u64;
        let mut total_del = 0u64;
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                let add: u64 = parts[0].parse().unwrap_or(0);
                let del: u64 = parts[1].parse().unwrap_or(0);
                total_add += add;
                total_del += del;
                files.push(json!({ "file": parts[2], "additions": add, "deletions": del }));
            }
        }
        assert_eq!(files.len(), 2);
        assert_eq!(total_add, 13);
        assert_eq!(total_del, 5);
        assert_eq!(files[0]["file"], "src/main.rs");
        assert_eq!(files[0]["additions"], 10);
        assert_eq!(files[1]["file"], "README.md");
    }

    #[test]
    fn parse_diff_numstat_marks_binary_files() {
        let parsed = parse_git_diff_numstat("-\t-\tassets/logo.png\n12\t3\tsrc/main.rs\n");
        assert_eq!(parsed.files.len(), 2);
        assert!(parsed.files[0].binary);
        assert_eq!(parsed.files[0].additions, 0);
        assert_eq!(parsed.stats.additions, 12);
        assert_eq!(parsed.stats.deletions, 3);
    }

    // ── parse git log format inline ────────────────────────────────────────

    #[test]
    fn parse_log_format() {
        let stdout = "abc1234|Alice|2025-01-15T10:30:00+00:00|feat: add new feature\ndef5678|Bob|2025-01-14T09:00:00+00:00|fix: bug fix\n";
        let mut commits = Vec::new();
        for line in stdout.lines() {
            let parts: Vec<&str> = line.splitn(4, '|').collect();
            if parts.len() >= 4 {
                commits.push(json!({
                    "hash": parts[0],
                    "author": parts[1],
                    "date": parts[2],
                    "message": parts[3],
                }));
            }
        }
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0]["hash"], "abc1234");
        assert_eq!(commits[0]["author"], "Alice");
        assert_eq!(commits[0]["message"], "feat: add new feature");
        assert_eq!(commits[1]["hash"], "def5678");
    }

    // ── parse git branch inline ────────────────────────────────────────────

    #[test]
    fn parse_branch_output() {
        let stdout = "* main        abc1234 latest commit\n  feature     def5678 wip\n";
        let mut branches = Vec::new();
        let mut current_branch = String::new();
        for line in stdout.lines() {
            let is_current = line.starts_with('*');
            let trimmed = line.trim_start_matches('*').trim_start();
            let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
            if parts.len() >= 2 {
                let name = parts[0].to_string();
                if is_current {
                    current_branch = name.clone();
                }
                branches.push(json!({
                    "name": name,
                    "hash": parts[1],
                    "message": parts.get(2).unwrap_or(&""),
                    "current": is_current,
                }));
            }
        }
        assert_eq!(current_branch, "main");
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0]["name"], "main");
        assert_eq!(branches[0]["current"], true);
        assert_eq!(branches[1]["name"], "feature");
        assert_eq!(branches[1]["current"], false);
    }

    // ── parse git remote inline ────────────────────────────────────────────

    #[test]
    fn parse_remote_output() {
        let stdout = "origin\thttps://github.com/user/repo.git (fetch)\norigin\thttps://github.com/user/repo.git (push)\n";
        let mut remotes = Vec::new();
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                remotes.push(json!({
                    "name": parts[0],
                    "url": parts[1],
                    "type": parts.get(2).unwrap_or(&""),
                }));
            }
        }
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0]["name"], "origin");
        assert_eq!(remotes[0]["url"], "https://github.com/user/repo.git");
        assert_eq!(remotes[0]["type"], "(fetch)");
    }

    // ── Mutation-killing: is_status_record ────────────────────────────────

    #[test]
    fn is_status_record_branch() {
        assert!(is_status_record("# branch.head main"));
    }

    #[test]
    fn is_status_record_staged() {
        assert!(is_status_record(
            "1 M. N... 100644 100644 100644 abc def src/main.rs"
        ));
    }

    #[test]
    fn is_status_record_unstaged() {
        assert!(is_status_record(
            "2 .M N... 100644 100644 100644 abc def src/lib.rs"
        ));
    }

    #[test]
    fn is_status_record_conflict() {
        assert!(is_status_record(
            "u UU N... 100644 100644 100644 100644 abc def 123 file.rs"
        ));
    }

    #[test]
    fn is_status_record_untracked() {
        assert!(is_status_record("? new_file.txt"));
    }

    #[test]
    fn is_status_record_ignored() {
        assert!(is_status_record("! ignored_file.txt"));
    }

    #[test]
    fn is_status_record_returns_false_for_rename_path() {
        assert!(!is_status_record("old name.rs"));
    }

    // ── Mutation-killing: parse_git_diff_numstat binary ───────────────────

    #[test]
    fn parse_git_diff_numstat_binary_file_zero_additions() {
        let parsed = parse_git_diff_numstat("-\t-\timage.png\n");
        assert_eq!(parsed.files.len(), 1);
        assert!(parsed.files[0].binary);
        assert_eq!(parsed.files[0].additions, 0);
        assert_eq!(parsed.files[0].deletions, 0);
        assert_eq!(parsed.stats.additions, 0);
        assert_eq!(parsed.stats.files_changed, 1);
    }

    // ── Mutation-killing: parse_git_log_output edge cases ─────────────────

    #[test]
    fn parse_git_log_output_ignores_short_lines() {
        let stdout = "abc1234|Alice\nvalid|Author|2025-01-01|msg\n";
        let result = parse_git_log_output(stdout);
        assert_eq!(result.commits.len(), 1);
        assert_eq!(result.commits[0].hash, "valid");
    }

    // ── Mutation-killing: parse_git_branch_output edge cases ──────────────

    #[test]
    fn parse_git_branch_output_ignores_short_lines() {
        let stdout = "* main\n  feature     def5678 wip\n";
        let result = parse_git_branch_output(stdout);
        assert_eq!(result.branches.len(), 1);
        assert_eq!(result.branches[0].name, "feature");
    }

    // ── Mutation-killing: parse_git_remote_output edge cases ──────────────

    #[test]
    fn parse_git_remote_output_ignores_short_lines() {
        let stdout = "origin\norigin\thttps://github.com/user/repo.git (fetch)\n";
        let result = parse_git_remote_output(stdout);
        assert_eq!(result.remotes.len(), 1);
        assert_eq!(result.remotes[0].name, "origin");
    }

    // ── Mutation-killing: parse_git_status_porcelain_v2 conflict path ─────

    #[test]
    fn parse_git_status_porcelain_v2_conflict_extracts_path() {
        let stdout = "u UU N... 100644 100644 100644 100644 abc123 def456 789abc conflicted.rs\n";
        let parsed = parse_git_status_porcelain_v2(stdout);
        assert_eq!(parsed.conflicts.len(), 1);
        assert_eq!(parsed.conflicts[0], "conflicted.rs");
    }
}
