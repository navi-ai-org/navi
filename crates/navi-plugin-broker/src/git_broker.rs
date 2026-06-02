use crate::error::BrokerError;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Git broker that mediates read-only git operations.
pub struct GitBroker {
    project_root: PathBuf,
    timeout: Duration,
    max_diff_bytes: u64,
}

/// Result of a git status operation.
#[derive(Debug, Clone)]
pub struct GitStatus {
    pub raw: String,
    pub entries: Vec<StatusEntry>,
}

/// A single entry from git status --porcelain.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub status: String,
    pub path: String,
}

impl GitBroker {
    /// Create a new Git broker for the given project root.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            timeout: Duration::from_secs(5),
            max_diff_bytes: 256 * 1024, // 256 KB
        }
    }

    /// Override the subprocess timeout (intended for tests).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Get git status of the project.
    ///
    /// REQ-GIT-001: Project-scoped.
    /// REQ-GIT-002: Supports status.
    /// REQ-GIT-003: Read-only.
    /// REQ-GIT-004: Returns structured output.
    /// REQ-GIT-006: Executes in subprocess.
    pub fn status(&self) -> Result<GitStatus, BrokerError> {
        let output = self.run_git(&["status", "--porcelain"])?;

        let entries = output
            .lines()
            .filter(|line| line.len() >= 3)
            .map(|line| {
                let status = line[..2].trim().to_string();
                let path = line[3..].to_string();
                StatusEntry { status, path }
            })
            .collect();

        Ok(GitStatus {
            raw: output,
            entries,
        })
    }

    /// Get git diff of the project.
    ///
    /// REQ-GIT-001: Project-scoped.
    /// REQ-GIT-002: Supports diff.
    /// REQ-GIT-003: Read-only.
    /// REQ-GIT-004: Returns structured output.
    /// REQ-GIT-006: Executes in subprocess.
    pub fn diff(&self) -> Result<String, BrokerError> {
        let output = self.run_git(&["diff"])?;

        // REQ: Cap output at 256 KB
        if output.len() as u64 > self.max_diff_bytes {
            return Err(BrokerError::TooLarge {
                size_bytes: output.len() as u64,
                limit_bytes: self.max_diff_bytes,
            });
        }

        Ok(output)
    }

    /// Get git log (read-only, post-MVP).
    pub fn log(&self, max_count: u32) -> Result<String, BrokerError> {
        let count_arg = format!("-{}", max_count);
        self.run_git(&["log", &count_arg, "--oneline"])
    }

    /// Get current branch name.
    pub fn branch(&self) -> Result<String, BrokerError> {
        self.run_git(&["branch", "--show-current"])
    }

    /// Get git remote URLs.
    pub fn remote(&self) -> Result<String, BrokerError> {
        self.run_git(&["remote", "-v"])
    }

    // --- Internal helpers ---

    /// Run a git command with the project root as working directory.
    ///
    /// REQ-GIT-001: Restricts to project root.
    /// REQ-GIT-003: Only read-only commands are called.
    /// REQ-GIT-005: Returns String, not process handle.
    /// REQ-GIT-006: Executes in subprocess.
    fn run_git(&self, args: &[&str]) -> Result<String, BrokerError> {
        // Verify project root is a git repository
        let git_dir = self.project_root.join(".git");
        if !git_dir.exists() {
            return Err(BrokerError::AccessDenied {
                reason: "not a git repository".into(),
            });
        }

        let mut child = Command::new("git")
            .args(args)
            .current_dir(&self.project_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(BrokerError::Io)?;

        let started = Instant::now();
        loop {
            match child.try_wait().map_err(BrokerError::Io)? {
                Some(status) => {
                    if !status.success() {
                        let stderr = child
                            .stderr
                            .take()
                            .map(|mut s| {
                                let mut buf = String::new();
                                let _ = std::io::Read::read_to_string(&mut s, &mut buf);
                                buf
                            })
                            .unwrap_or_default();
                        return Err(BrokerError::AccessDenied {
                            reason: format!("git error: {}", stderr.trim()),
                        });
                    }
                    let mut stdout = String::new();
                    if let Some(mut pipe) = child.stdout.take() {
                        std::io::Read::read_to_string(&mut pipe, &mut stdout)
                            .map_err(BrokerError::Io)?;
                    }
                    return Ok(stdout);
                }
                None if started.elapsed() >= self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(BrokerError::Timeout {
                        timeout_ms: self.timeout.as_millis() as u64,
                    });
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_repo() -> (TempDir, GitBroker) {
        let tmp = TempDir::new().unwrap();

        // Initialize a git repo
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .output()
            .expect("git init");

        // Configure git for testing
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .output()
            .expect("git config email");

        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .output()
            .expect("git config name");

        let broker = GitBroker::new(tmp.path().to_path_buf());
        (tmp, broker)
    }

    #[test]
    fn status_empty_repo() {
        let (_tmp, broker) = setup_git_repo();
        let result = broker.status();
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.entries.is_empty());
    }

    #[test]
    fn status_with_untracked_file() {
        let (tmp, broker) = setup_git_repo();
        fs::write(tmp.path().join("new.txt"), "content").unwrap();

        let result = broker.status();
        assert!(result.is_ok());
        let status = result.unwrap();
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].path, "new.txt");
        assert_eq!(status.entries[0].status, "??");
    }

    #[test]
    fn status_with_staged_file() {
        let (tmp, broker) = setup_git_repo();
        fs::write(tmp.path().join("staged.txt"), "content").unwrap();

        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(tmp.path())
            .output()
            .expect("git add");

        let result = broker.status();
        assert!(result.is_ok());
        let status = result.unwrap();
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].path, "staged.txt");
        assert_eq!(status.entries[0].status, "A");
    }

    #[test]
    fn status_with_modified_file() {
        let (tmp, broker) = setup_git_repo();
        fs::write(tmp.path().join("tracked.txt"), "initial").unwrap();

        Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(tmp.path())
            .output()
            .expect("git add");

        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(tmp.path())
            .output()
            .expect("git commit");

        fs::write(tmp.path().join("tracked.txt"), "modified").unwrap();

        let result = broker.status();
        assert!(result.is_ok());
        let status = result.unwrap();
        assert_eq!(status.entries.len(), 1);
        assert_eq!(status.entries[0].path, "tracked.txt");
        // Unstaged modification: " M" trimmed to "M"
        assert_eq!(status.entries[0].status, "M");
    }

    #[test]
    fn diff_empty_repo() {
        let (_tmp, broker) = setup_git_repo();
        let result = broker.diff();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn diff_with_changes() {
        let (tmp, broker) = setup_git_repo();
        fs::write(tmp.path().join("file.txt"), "line1\nline2\n").unwrap();

        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .output()
            .expect("git add");

        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(tmp.path())
            .output()
            .expect("git commit");

        fs::write(tmp.path().join("file.txt"), "line1\nmodified\n").unwrap();

        let result = broker.diff();
        assert!(result.is_ok());
        let diff = result.unwrap();
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }

    #[test]
    fn not_a_git_repo() {
        let tmp = TempDir::new().unwrap();
        let broker = GitBroker::new(tmp.path().to_path_buf());
        let result = broker.status();
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn branch_name() {
        let (_tmp, broker) = setup_git_repo();
        let result = broker.branch();
        assert!(result.is_ok());
        let branch = result.unwrap().trim().to_string();
        // Default branch is usually "main" or "master"
        assert!(branch == "main" || branch == "master" || branch.is_empty());
    }

    #[test]
    fn log_empty_repo() {
        let (_tmp, broker) = setup_git_repo();
        let result = broker.log(10);
        // May fail if there are no commits
        // That's ok for an empty repo
        let _ = result;
    }

    #[test]
    fn log_with_commits() {
        let (tmp, broker) = setup_git_repo();
        fs::write(tmp.path().join("file.txt"), "content").unwrap();

        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .output()
            .expect("git add");

        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(tmp.path())
            .output()
            .expect("git commit");

        let result = broker.log(5);
        assert!(result.is_ok());
        let log = result.unwrap();
        assert!(log.contains("initial commit"));
    }

    #[test]
    fn remote_empty() {
        let (_tmp, broker) = setup_git_repo();
        let result = broker.remote();
        assert!(result.is_ok());
        // No remotes configured
        assert!(result.unwrap().trim().is_empty());
    }
}
