//! Virtual path resolver with mounts, `..`, symlinks (logical), and sandbox root.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct Vfs {
    /// mounts in registration order (BUG: should pick longest prefix match)
    mounts: Vec<(String, String)>,
    /// symlink path → target path (may be relative)
    links: HashMap<String, String>,
    root: String,
}

impl Vfs {
    pub fn new(root: &str) -> Self {
        Self {
            mounts: Vec::new(),
            links: HashMap::new(),
            root: normalize_abs(root),
        }
    }

    pub fn mount(&mut self, at: &str, target: &str) {
        self.mounts
            .push((normalize_abs(at), normalize_abs(target)));
    }

    pub fn symlink(&mut self, link: &str, target: &str) {
        self.links.insert(normalize_abs(link), target.to_string());
    }

    /// Resolve `path` (absolute or relative to `base`) into a normalized absolute
    /// path under root, applying mounts and a single level of symlink.
    pub fn resolve(&self, base: &str, path: &str) -> Result<String, String> {
        let abs = if path.starts_with('/') {
            path.to_string()
        } else {
            join_path(base, path)
        };
        let mut cur = normalize_abs(&abs);

        if let Some(t) = self.links.get(&cur) {
            if t.starts_with('/') {
                cur = normalize_abs(t);
            } else {
                // TODO(fix): resolves relative symlink against filesystem root, not link parent
                cur = join_path("/", t);
                cur = normalize_abs(&cur);
            }
        }

        // TODO(fix): first matching mount wins (registration order) instead of longest prefix
        for (mnt, tgt) in &self.mounts {
            if cur == *mnt || cur.starts_with(&(mnt.clone() + "/")) {
                let rest = &cur[mnt.len()..];
                cur = if rest.is_empty() {
                    tgt.clone()
                } else {
                    format!("{tgt}{rest}")
                };
                cur = normalize_abs(&cur);
                break;
            }
        }

        if !cur.starts_with(&self.root) {
            // TODO(fix): allows escape if cur is root without trailing considerations
            return Err(format!("escapes sandbox: {cur}"));
        }
        Ok(cur)
    }
}

fn join_path(base: &str, rel: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{rel}")
    } else {
        format!("{base}/{rel}")
    }
}

/// Normalize absolute path: collapse `.` and `..`, remove duplicate `/`.
fn normalize_abs(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                // TODO(fix): always pop; at root this can erase the only component and
                // then continue, allowing paths that should stay at "/".
                // Correct: only pop if stack non-empty (already), but must not
                // allow normalize("/a/../..") to leave empty incorrectly —
                // actually empty stack → "/". Real bug: treat ".." as literal
                // when stack empty by pushing ".." then later failing sandbox.
                if stack.is_empty() {
                    stack.push("..");
                } else {
                    stack.pop();
                }
            }
            p => stack.push(p),
        }
    }
    format!("/{}", stack.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dotdot_stops_at_root() {
        assert_eq!(normalize_abs("/a/../.."), "/");
        assert_eq!(normalize_abs("/a/b/../c"), "/a/c");
    }

    #[test]
    fn resolve_relative() {
        let vfs = Vfs::new("/home/proj");
        let p = vfs.resolve("/home/proj/src", "../README.md").unwrap();
        assert_eq!(p, "/home/proj/README.md");
    }

    #[test]
    fn symlink_relative_to_link_parent() {
        let mut vfs = Vfs::new("/home/proj");
        vfs.symlink("/home/proj/link", "target.txt");
        let p = vfs.resolve("/home/proj", "link").unwrap();
        assert_eq!(p, "/home/proj/target.txt");
    }

    #[test]
    fn mount_prefix() {
        let mut vfs = Vfs::new("/");
        vfs.mount("/mnt/data", "/var/data");
        let p = vfs.resolve("/", "/mnt/data/x").unwrap();
        assert_eq!(p, "/var/data/x");
    }

    #[test]
    fn nested_mount_longest_prefix() {
        let mut vfs = Vfs::new("/");
        vfs.mount("/mnt", "/A");
        vfs.mount("/mnt/sub", "/B");
        let p = vfs.resolve("/", "/mnt/sub/f").unwrap();
        assert_eq!(p, "/B/f");
    }

    #[test]
    fn sandbox_escape_rejected() {
        let vfs = Vfs::new("/home/proj");
        assert!(vfs.resolve("/home/proj", "../../etc/passwd").is_err());
    }
}
