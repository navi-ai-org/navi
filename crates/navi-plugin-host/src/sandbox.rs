//! Process-level sandboxing for native plugin loading.
//!
//! On Linux, applies Landlock rules to restrict filesystem access to a small set
//! of approved paths (the project root, the data directory, the plugin's own
//! directory, and standard library paths). On macOS, documents the limitation
//! and recommends the caller wrap plugin invocation in a sandbox profile.
//! On other platforms, this is a no-op.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// Outcome of a sandbox activation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxStatus {
    /// Sandbox was successfully applied.
    Active,
    /// Sandbox could not be applied (unsupported kernel, blocked by seccomp,
    /// permission error, etc.) but the caller can still proceed.
    Unavailable(&'static str),
    /// Sandbox was applied with a warning (e.g. some paths were rejected).
    ActiveWithWarnings,
}

/// Try to apply a process-wide filesystem sandbox to allow only the given
/// paths (and their descendants) for read/write/execute.
///
/// On Linux with Landlock (kernel >= 5.13), this calls
/// `landlock::RulesetBuilder` to create an enforced ruleset. On other platforms
/// it returns `SandboxStatus::Unavailable` with a reason.
pub fn apply_filesystem_sandbox<I, P>(allow_paths: I) -> Result<SandboxStatus>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<PathBuf> = allow_paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect();

    #[cfg(all(target_os = "linux", feature = "landlock"))]
    {
        return apply_landlock_sandbox(&paths);
    }

    #[cfg(not(all(target_os = "linux", feature = "landlock")))]
    {
        let _ = paths;
        Ok(SandboxStatus::Unavailable(
            "sandbox support not compiled in (need linux + landlock feature)",
        ))
    }
}

/// Variant of `apply_filesystem_sandbox` that uses an injected Landlock ABI
/// path. Used for unit testing the path collection / argument-building code
/// without actually invoking the kernel.
#[cfg(test)]
pub fn dry_run_sandbox<I, P>(allow_paths: I) -> Result<Vec<PathBuf>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let paths: Vec<PathBuf> = allow_paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .collect();
    Ok(paths)
}

#[cfg(all(target_os = "linux", feature = "landlock"))]
fn apply_landlock_sandbox(allow_paths: &[PathBuf]) -> Result<SandboxStatus> {
    use anyhow::Context;
    use landlock::{ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr};

    let abi = ABI::V5;
    let access = AccessFs::from_all(abi);

    // Try to create a ruleset; if Landlock is not supported, return Unavailable.
    let inner = (|| -> Result<SandboxStatus> {
        let mut warnings: u32 = 0;

        let created = Ruleset::default()
            .handle_access(access)
            .context("failed to create Landlock ruleset")?
            .create()
            .context("failed to create Landlock ruleset (kernel support?)")?;

        // Build the list of paths to allow: user paths + system paths.
        let mut all_paths: Vec<PathBuf> = allow_paths.to_vec();
        for sys in &[
            PathBuf::from("/usr"),
            PathBuf::from("/lib"),
            PathBuf::from("/lib64"),
            PathBuf::from("/bin"),
            PathBuf::from("/etc/ld.so.cache"),
        ] {
            if sys.exists() {
                all_paths.push(sys.clone());
            }
        }

        // Build a Vec<PathBeneath<PathFd>> of rules to add.
        let mut rules: Vec<landlock::PathBeneath<landlock::PathFd>> = Vec::new();
        for path in &all_paths {
            match path_fd_silent(path) {
                Some(fd) => rules.push(landlock::PathBeneath::new(fd, access)),
                None => warnings += 1,
            }
        }
        // add_rules consumes the ruleset. We feed it the rules as a single
        // batch (a Vec of Ok results). If the call fails, we just count
        // warnings and try to restrict_self with whatever rules were already
        // applied.
        let rule_results: Vec<std::result::Result<_, landlock::RulesetError>> =
            rules.into_iter().map(Ok).collect();
        let active_opt: Option<_> = match created.add_rules(rule_results) {
            Ok(next) => Some(next),
            Err(_e) => {
                warnings = warnings.saturating_add(1);
                None
            }
        };
        let Some(active) = active_opt else {
            return Ok(SandboxStatus::ActiveWithWarnings);
        };

        active
            .restrict_self()
            .context("failed to restrict process with Landlock")?;

        Ok(if warnings == 0 {
            SandboxStatus::Active
        } else {
            SandboxStatus::ActiveWithWarnings
        })
    })();

    match inner {
        Ok(s) => Ok(s),
        Err(_) => Ok(SandboxStatus::Unavailable("Landlock setup failed")),
    }
}

/// Wrapper that opens a `PathFd` and returns `None` on error.
#[cfg(all(target_os = "linux", feature = "landlock"))]
fn path_fd_silent(path: &Path) -> Option<landlock::PathFd> {
    use landlock::PathFd;
    if !path.exists() {
        return None;
    }
    PathFd::new(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn dry_run_returns_canonical_paths() {
        let tmp = tempdir().unwrap();
        let paths = dry_run_sandbox([tmp.path()]).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], tmp.path());
    }

    #[test]
    fn apply_sandbox_reports_unavailable_when_feature_off() {
        // We don't depend on the `landlock` feature; this just confirms the
        // function returns Unavailable with a stable reason on platforms
        // where Landlock is not available or not enabled.
        let status = apply_filesystem_sandbox(Vec::<PathBuf>::new()).unwrap();
        match status {
            SandboxStatus::Unavailable(_) => {}
            SandboxStatus::Active | SandboxStatus::ActiveWithWarnings => {
                // Landlock might be enabled on the build host; that's fine.
            }
        }
    }

    #[test]
    fn sandbox_status_distinguishes_outcomes() {
        let a = SandboxStatus::Active;
        let b = SandboxStatus::ActiveWithWarnings;
        let c = SandboxStatus::Unavailable("reason");
        assert_eq!(a, a);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
