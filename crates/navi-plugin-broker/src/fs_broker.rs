use crate::error::BrokerError;
use navi_plugin_manifest::SecurityDefaults;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Audit log entry for a broker call.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub plugin_id: String,
    pub tool_id: String,
    pub capability_id: String,
    pub operation: String,
    pub target: String,
    pub result: AuditResult,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AuditResult {
    Allow,
    Deny,
}

/// Filesystem broker that mediates all plugin filesystem access.
pub struct FsBroker {
    project_root: PathBuf,
    defaults: SecurityDefaults,
    /// Total bytes read in this invocation.
    bytes_read: Arc<AtomicU64>,
    /// Sensitive path patterns (always blocked).
    sensitive_patterns: Vec<String>,
    /// Heavy directory patterns (blocked by default).
    heavy_dir_patterns: Vec<String>,
    /// Additional allowed path prefixes (from capability).
    allowed_prefixes: Vec<PathBuf>,
    /// Audit log entries.
    audit_log: Vec<AuditEntry>,
}

/// Result of a file read operation.
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub content: String,
    pub size_bytes: u64,
}

impl FsBroker {
    /// Create a new FS broker for the given project root.
    pub fn new(project_root: PathBuf, defaults: SecurityDefaults) -> Self {
        let sensitive_patterns = defaults.fs.sensitive_patterns.clone();
        let heavy_dir_patterns = defaults.fs.heavy_dir_patterns.clone();

        Self {
            project_root,
            defaults,
            bytes_read: Arc::new(AtomicU64::new(0)),
            sensitive_patterns,
            heavy_dir_patterns,
            allowed_prefixes: Vec::new(),
            audit_log: Vec::new(),
        }
    }

    /// Set additional allowed path prefixes (from capability).
    pub fn with_allowed_prefixes(mut self, prefixes: Vec<PathBuf>) -> Self {
        self.allowed_prefixes = prefixes;
        self
    }

    /// Read a file within the project directory.
    ///
    /// Authorization algorithm (from broker-contracts.md):
    /// 1. Resolve project root.
    /// 2. Join relative path with project root.
    /// 3. Canonicalize using realpath.
    /// 4. Resolve ALL symlinks.
    /// 5. Check: final path under project root.
    /// 6. Check: final path not in denylist.
    /// 7. Check: no null bytes.
    /// 8. Check: no `..` after canonicalization.
    /// 9. Check: file size within limit.
    /// 10. Check: total bytes within invocation budget.
    /// 11. Read as UTF-8.
    /// 12. Return content.
    pub fn read_project_file(
        &mut self,
        plugin_id: &str,
        tool_id: &str,
        capability_id: &str,
        requested_path: &str,
    ) -> Result<ReadResult, BrokerError> {
        // Step 1-3: Resolve and canonicalize
        let resolved = self.resolve_and_validate(requested_path)?;

        // Step 5: Check under project root
        self.check_under_project_root(&resolved).inspect_err(|_| {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("outside project"),
            );
        })?;

        // Step 6: Check denylist
        self.check_denylist(&resolved).inspect_err(|_| {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("denylist"),
            );
        })?;

        // Step 8: Check for .. after canonicalization
        self.check_no_traversal(&resolved).inspect_err(|_| {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("traversal"),
            );
        })?;

        // Step 6: Check denylist
        self.check_denylist(&resolved).inspect_err(|_| {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("denylist"),
            );
        })?;

        // Step 8: Check for .. after canonicalization
        self.check_no_traversal(&resolved).inspect_err(|_| {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("traversal"),
            );
        })?;

        // Check allowed prefixes if configured
        if !self.allowed_prefixes.is_empty() {
            let rel = resolved
                .strip_prefix(&self.project_root)
                .map_err(|_| BrokerError::OutsideProject)?;
            let allowed = self
                .allowed_prefixes
                .iter()
                .any(|prefix| rel.starts_with(prefix));
            if !allowed {
                self.log_audit(
                    plugin_id,
                    tool_id,
                    capability_id,
                    "read",
                    requested_path,
                    AuditResult::Deny,
                    Some("path not in capability allowed prefixes"),
                );
                return Err(BrokerError::AccessDenied {
                    reason: "path not in capability allowed prefixes".into(),
                });
            }
        }

        // Step 9: Check file size
        let metadata = std::fs::metadata(&resolved)?;
        let size = metadata.len();
        let max_file = self.defaults.fs.max_file_read_bytes;
        if size > max_file {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("file too large"),
            );
            return Err(BrokerError::TooLarge {
                size_bytes: size,
                limit_bytes: max_file,
            });
        }

        // Step 10: Check invocation budget
        let current_total = self.bytes_read.load(Ordering::Relaxed);
        let max_total = self.defaults.fs.max_total_read_bytes;
        if current_total + size > max_total {
            self.log_audit(
                plugin_id,
                tool_id,
                capability_id,
                "read",
                requested_path,
                AuditResult::Deny,
                Some("invocation budget exceeded"),
            );
            return Err(BrokerError::BudgetExceeded {
                total_bytes: current_total + size,
                budget_bytes: max_total,
            });
        }

        // Step 11: Read as UTF-8
        let content = std::fs::read_to_string(&resolved).map_err(|e| {
            if e.kind() == std::io::ErrorKind::InvalidData {
                BrokerError::InvalidUtf8
            } else {
                BrokerError::Io(e)
            }
        })?;

        // Update bytes read
        self.bytes_read.fetch_add(size, Ordering::Relaxed);

        // Step 12: Log and return
        self.log_audit(
            plugin_id,
            tool_id,
            capability_id,
            "read",
            requested_path,
            AuditResult::Allow,
            None,
        );

        Ok(ReadResult {
            content,
            size_bytes: size,
        })
    }

    /// List entries in a project directory.
    pub fn list_project_dir(
        &mut self,
        plugin_id: &str,
        tool_id: &str,
        capability_id: &str,
        requested_path: &str,
    ) -> Result<Vec<String>, BrokerError> {
        // Resolve and validate
        let resolved = self.resolve_and_validate(requested_path)?;

        // Check under project root
        self.check_under_project_root(&resolved)?;

        // Check denylist
        self.check_denylist(&resolved)?;

        // Check for traversal
        self.check_no_traversal(&resolved)?;

        // Check allowed prefixes
        if !self.allowed_prefixes.is_empty() {
            let rel = resolved
                .strip_prefix(&self.project_root)
                .map_err(|_| BrokerError::OutsideProject)?;
            let allowed = self
                .allowed_prefixes
                .iter()
                .any(|prefix| rel.starts_with(prefix));
            if !allowed {
                self.log_audit(
                    plugin_id,
                    tool_id,
                    capability_id,
                    "list",
                    requested_path,
                    AuditResult::Deny,
                    Some("path not in capability allowed prefixes"),
                );
                return Err(BrokerError::AccessDenied {
                    reason: "path not in capability allowed prefixes".into(),
                });
            }
        }

        // Check it's a directory
        if !resolved.is_dir() {
            return Err(BrokerError::NotFound {
                path: requested_path.into(),
            });
        }

        // Read entries
        let entries = std::fs::read_dir(&resolved)?;
        let mut names: Vec<String> = Vec::new();

        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            // Check if the entry's resolved path matches denylist
            let entry_path = resolved.join(&name);
            if self.check_denylist(&entry_path).is_err() {
                continue;
            }

            names.push(name);
        }

        names.sort();

        self.log_audit(
            plugin_id,
            tool_id,
            capability_id,
            "list",
            requested_path,
            AuditResult::Allow,
            None,
        );

        Ok(names)
    }

    /// Get the audit log entries.
    pub fn audit_log(&self) -> &[AuditEntry] {
        &self.audit_log
    }

    /// Get total bytes read in this invocation.
    pub fn total_bytes_read(&self) -> u64 {
        self.bytes_read.load(Ordering::Relaxed)
    }

    // --- Internal helpers ---

    /// Resolve a path against the project root and canonicalize.
    fn resolve_and_validate(&self, requested_path: &str) -> Result<PathBuf, BrokerError> {
        // Check for null bytes
        if requested_path.contains('\0') {
            return Err(BrokerError::AccessDenied {
                reason: "path contains null byte".into(),
            });
        }

        // Check for backslashes (non-Windows)
        if requested_path.contains('\\') {
            return Err(BrokerError::AccessDenied {
                reason: "path contains backslash".into(),
            });
        }

        // Check for leading/trailing whitespace
        if requested_path != requested_path.trim() {
            return Err(BrokerError::AccessDenied {
                reason: "path has leading or trailing whitespace".into(),
            });
        }

        // Resolve against project root
        let path = Path::new(requested_path);
        let full_path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.project_root.join(path)
        };

        // Canonicalize (resolves symlinks and ..)
        let canonical = full_path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                BrokerError::NotFound {
                    path: requested_path.into(),
                }
            } else {
                BrokerError::Io(e)
            }
        })?;

        Ok(canonical)
    }

    /// Check that a resolved path is under the project root.
    fn check_under_project_root(&self, resolved: &Path) -> Result<(), BrokerError> {
        let project_canonical = self.project_root.canonicalize().map_err(BrokerError::Io)?;

        if !resolved.starts_with(&project_canonical) {
            return Err(BrokerError::OutsideProject);
        }

        Ok(())
    }

    /// Check that a resolved path does not match the denylist.
    fn check_denylist(&self, resolved: &Path) -> Result<(), BrokerError> {
        let path_str = resolved.to_string_lossy();

        // Check sensitive patterns
        for pattern in &self.sensitive_patterns {
            if path_matches_pattern(&path_str, pattern, resolved) {
                return Err(BrokerError::AccessDenied {
                    reason: format!("path matches sensitive pattern: {}", pattern),
                });
            }
        }

        // Check heavy dir patterns
        for pattern in &self.heavy_dir_patterns {
            if path_matches_pattern(&path_str, pattern, resolved) {
                return Err(BrokerError::AccessDenied {
                    reason: format!("path matches heavy directory pattern: {}", pattern),
                });
            }
        }

        // REQ-FS-010: Reject NAVI private storage
        if let Some(home) = dirs_home() {
            let navi_config = home.join(".config/navi");
            let navi_data = home.join(".local/share/navi");
            if resolved.starts_with(&navi_config) || resolved.starts_with(&navi_data) {
                return Err(BrokerError::AccessDenied {
                    reason: "path is in NAVI private storage".into(),
                });
            }
        }

        Ok(())
    }

    /// Check for path traversal after canonicalization.
    fn check_no_traversal(&self, resolved: &Path) -> Result<(), BrokerError> {
        for component in resolved.components() {
            if let Component::ParentDir = component {
                return Err(BrokerError::AccessDenied {
                    reason: "path contains '..' after canonicalization".into(),
                });
            }
        }
        Ok(())
    }

    /// Log an audit entry.
    #[allow(clippy::too_many_arguments)]
    fn log_audit(
        &mut self,
        plugin_id: &str,
        tool_id: &str,
        capability_id: &str,
        operation: &str,
        target: &str,
        result: AuditResult,
        reason: Option<&str>,
    ) {
        self.audit_log.push(AuditEntry {
            plugin_id: plugin_id.into(),
            tool_id: tool_id.into(),
            capability_id: capability_id.into(),
            operation: operation.into(),
            target: target.into(),
            result,
            reason: reason.map(|s| s.into()),
        });
    }
}

/// Match a path against a pattern.
fn path_matches_pattern(path: &str, pattern: &str, resolved: &Path) -> bool {
    // Exact match
    if path == pattern {
        return true;
    }

    // Check if path ends with the pattern (with or without leading separator)
    if path.ends_with(&format!("/{}", pattern)) || path.ends_with(pattern) {
        return true;
    }

    // Directory prefix match (pattern ends with /)
    if let Some(prefix) = pattern.strip_suffix('/') {
        // Check if any ancestor directory matches
        for ancestor in resolved.ancestors() {
            let a_str = ancestor.to_string_lossy();
            if a_str.ends_with(prefix) {
                return true;
            }
        }
    }

    // Extension match (pattern starts with *)
    if let Some(suffix) = pattern.strip_prefix('*')
        && path.ends_with(suffix)
    {
        return true;
    }

    // Wildcard in middle (e.g., ".env.*")
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.splitn(2, '*').collect();
        if parts.len() == 2 {
            // Check filename component
            if let Some(name) = resolved.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with(parts[0]) && name_str.ends_with(parts[1]) {
                    return true;
                }
            }
        }
    }

    false
}

/// Get the home directory.
fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, FsBroker) {
        let tmp = TempDir::new().unwrap();
        let defaults = SecurityDefaults::default();
        let broker = FsBroker::new(tmp.path().to_path_buf(), defaults);
        (tmp, broker)
    }

    #[test]
    fn read_basic_file() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("hello.txt"), "world").unwrap();
        let result = broker.read_project_file("p", "t", "c", "hello.txt");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "world");
    }

    #[test]
    fn read_subdirectory_file() {
        let (tmp, mut broker) = setup();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        let result = broker.read_project_file("p", "t", "c", "src/main.rs");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().content, "fn main() {}");
    }

    #[test]
    fn read_nonexistent_file() {
        let (_tmp, mut broker) = setup();
        let result = broker.read_project_file("p", "t", "c", "missing.txt");
        assert!(matches!(result, Err(BrokerError::NotFound { .. })));
    }

    #[test]
    fn reject_null_byte() {
        let (_tmp, mut broker) = setup();
        let result = broker.read_project_file("p", "t", "c", "file\0.txt");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_backslash() {
        let (_tmp, mut broker) = setup();
        let result = broker.read_project_file("p", "t", "c", "path\\file");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_whitespace() {
        let (_tmp, mut broker) = setup();
        let result = broker.read_project_file("p", "t", "c", " file.txt");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_path_traversal() {
        let (tmp, mut broker) = setup();
        // Create a file outside project root
        let parent = tmp.path().parent().unwrap();
        fs::write(parent.join("secret.txt"), "secret").unwrap();
        let result = broker.read_project_file("p", "t", "c", "../secret.txt");
        // Should fail: either OutsideProject or NotFound (depending on canonicalize)
        assert!(result.is_err());
    }

    #[test]
    fn reject_symlink_escape() {
        let (tmp, mut broker) = setup();
        // Create a symlink pointing outside project
        let outside = tmp.path().parent().unwrap().join("outside.txt");
        fs::write(&outside, "outside content").unwrap();
        let link = tmp.path().join("escape_link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let result = broker.read_project_file("p", "t", "c", "escape_link");
        assert!(matches!(result, Err(BrokerError::OutsideProject)));
    }

    #[test]
    fn reject_dotgit() {
        let (tmp, mut broker) = setup();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".git/config"), "[core]").unwrap();
        let result = broker.read_project_file("p", "t", "c", ".git/config");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_dotenv() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join(".env"), "SECRET=abc").unwrap();
        let result = broker.read_project_file("p", "t", "c", ".env");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_dotenv_variant() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join(".env.local"), "SECRET=abc").unwrap();
        let result = broker.read_project_file("p", "t", "c", ".env.local");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_pem_file() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("server.pem"), "---CERT---").unwrap();
        let result = broker.read_project_file("p", "t", "c", "server.pem");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_key_file() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("id_rsa.key"), "---KEY---").unwrap();
        let result = broker.read_project_file("p", "t", "c", "id_rsa.key");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_node_modules() {
        let (tmp, mut broker) = setup();
        fs::create_dir_all(tmp.path().join("node_modules/pkg")).unwrap();
        fs::write(tmp.path().join("node_modules/pkg/index.js"), "x").unwrap();
        let result = broker.read_project_file("p", "t", "c", "node_modules/pkg/index.js");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn reject_target_dir() {
        let (tmp, mut broker) = setup();
        fs::create_dir_all(tmp.path().join("target/debug")).unwrap();
        fs::write(tmp.path().join("target/debug/app"), "x").unwrap();
        let result = broker.read_project_file("p", "t", "c", "target/debug/app");
        assert!(matches!(result, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn file_size_cap() {
        let (tmp, mut broker) = setup();
        // Create a file larger than the default limit (2MB)
        let big_content = "x".repeat(3 * 1024 * 1024);
        fs::write(tmp.path().join("big.txt"), &big_content).unwrap();
        let result = broker.read_project_file("p", "t", "c", "big.txt");
        assert!(matches!(result, Err(BrokerError::TooLarge { .. })));
    }

    #[test]
    fn invocation_budget() {
        let (tmp, mut broker) = setup();
        // Create multiple files that together exceed the budget (16MB)
        // Each file must be under the per-file limit (2MB)
        let content = "x".repeat(1500 * 1024); // ~1.5MB each
        fs::write(tmp.path().join("a.txt"), &content).unwrap();
        fs::write(tmp.path().join("b.txt"), &content).unwrap();
        fs::write(tmp.path().join("c.txt"), &content).unwrap();
        fs::write(tmp.path().join("d.txt"), &content).unwrap();
        fs::write(tmp.path().join("e.txt"), &content).unwrap();
        fs::write(tmp.path().join("f.txt"), &content).unwrap();
        fs::write(tmp.path().join("g.txt"), &content).unwrap();
        fs::write(tmp.path().join("h.txt"), &content).unwrap();
        fs::write(tmp.path().join("i.txt"), &content).unwrap();
        fs::write(tmp.path().join("j.txt"), &content).unwrap();
        fs::write(tmp.path().join("k.txt"), &content).unwrap(); // ~16.5MB total

        // First 10 should succeed (~15MB)
        for c in 'a'..='j' {
            let r = broker.read_project_file("p", "t", "c", &format!("{}.txt", c));
            assert!(r.is_ok(), "reading {}.txt should succeed: {:?}", c, r.err());
        }

        // 11th should fail (budget exceeded)
        let r = broker.read_project_file("p", "t", "c", "k.txt");
        assert!(matches!(r, Err(BrokerError::BudgetExceeded { .. })));
    }

    #[test]
    fn list_directory() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        fs::write(tmp.path().join("c.rs"), "c").unwrap();
        let result = broker.list_project_dir("p", "t", "c", ".");
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries, vec!["a.txt", "b.txt", "c.rs"]);
    }

    #[test]
    fn list_directory_filters_denylist() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("good.txt"), "ok").unwrap();
        fs::write(tmp.path().join(".env"), "SECRET").unwrap();
        fs::create_dir_all(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".git/config"), "x").unwrap();
        let result = broker.list_project_dir("p", "t", "c", ".");
        assert!(result.is_ok());
        let entries = result.unwrap();
        assert_eq!(entries, vec!["good.txt"]);
    }

    #[test]
    fn allowed_prefixes() {
        let (tmp, _broker) = setup();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::create_dir_all(tmp.path().join("lib")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("lib/util.rs"), "pub fn util() {}").unwrap();

        let mut broker = FsBroker::new(tmp.path().to_path_buf(), SecurityDefaults::default())
            .with_allowed_prefixes(vec![PathBuf::from("src")]);

        let r1 = broker.read_project_file("p", "t", "c", "src/main.rs");
        assert!(r1.is_ok());

        let r2 = broker.read_project_file("p", "t", "c", "lib/util.rs");
        assert!(matches!(r2, Err(BrokerError::AccessDenied { .. })));
    }

    #[test]
    fn audit_log_records_entries() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("test.txt"), "data").unwrap();
        fs::write(tmp.path().join(".env"), "SECRET=abc").unwrap();
        let _ = broker.read_project_file("my-plugin", "search", "fs_read", "test.txt");
        let _ = broker.read_project_file("my-plugin", "search", "fs_read", ".env");

        let log = broker.audit_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].plugin_id, "my-plugin");
        assert!(matches!(log[0].result, AuditResult::Allow));
        assert!(matches!(log[1].result, AuditResult::Deny));
    }

    #[test]
    fn bytes_read_tracking() {
        let (tmp, mut broker) = setup();
        fs::write(tmp.path().join("a.txt"), "hello").unwrap(); // 5 bytes
        fs::write(tmp.path().join("b.txt"), "world!").unwrap(); // 6 bytes

        let _ = broker.read_project_file("p", "t", "c", "a.txt");
        assert_eq!(broker.total_bytes_read(), 5);

        let _ = broker.read_project_file("p", "t", "c", "b.txt");
        assert_eq!(broker.total_bytes_read(), 11);
    }
}
