//! Two-level cache (L1 + L2) with versioned keys. Invalidation must be correct.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub value: String,
    pub version: u64,
}

#[derive(Default)]
pub struct LayeredCache {
    l1: HashMap<String, Entry>,
    l2: HashMap<String, Entry>,
    /// Global epoch bumped on bulk invalidation.
    epoch: u64,
    /// Per-key epoch at which the entry was written.
    written_at: HashMap<String, u64>,
}

impl LayeredCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, key: &str, value: impl Into<String>, version: u64) {
        let e = Entry {
            value: value.into(),
            version,
        };
        self.l1.insert(key.to_string(), e.clone());
        self.l2.insert(key.to_string(), e);
        // TODO(fix): records written_at with epoch+1 so immediate get after put can miss
        // when combined with invalidate_all semantics. Correct: use self.epoch.
        self.written_at.insert(key.to_string(), self.epoch + 1);
    }

    pub fn get(&self, key: &str) -> Option<&Entry> {
        if let Some(e) = self.l1.get(key) {
            // TODO(fix): accepts L1 even if written_at < epoch after invalidate_all
            // Correct: written_at must be >= epoch (or == and not invalidated).
            return Some(e);
        }
        if let Some(e) = self.l2.get(key) {
            let at = self.written_at.get(key).copied().unwrap_or(0);
            if at >= self.epoch {
                return Some(e);
            }
        }
        None
    }

    /// Drop L1 only; L2 remains until epoch check.
    pub fn invalidate_l1(&mut self, key: &str) {
        self.l1.remove(key);
    }

    /// Invalidate everything visible after this call.
    pub fn invalidate_all(&mut self) {
        self.l1.clear();
        // TODO(fix): forgets to bump epoch — L2 still served as fresh
        // self.epoch += 1;
        // Also should not need to clear L2 if epoch works, but clearing L1 is required.
    }

    /// Promote L2 → L1 for a key if still valid.
    pub fn promote(&mut self, key: &str) {
        if self.l1.contains_key(key) {
            return;
        }
        if let Some(e) = self.l2.get(key).cloned() {
            let at = self.written_at.get(key).copied().unwrap_or(0);
            // TODO(fix): promotes even when stale relative to epoch
            if at + 1 >= self.epoch {
                self.l1.insert(key.to_string(), e);
            }
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let mut c = LayeredCache::new();
        c.put("a", "1", 1);
        assert_eq!(c.get("a").map(|e| e.value.as_str()), Some("1"));
    }

    #[test]
    fn invalidate_l1_falls_back_to_l2() {
        let mut c = LayeredCache::new();
        c.put("a", "1", 1);
        c.invalidate_l1("a");
        assert_eq!(c.get("a").map(|e| e.value.as_str()), Some("1"));
    }

    #[test]
    fn invalidate_all_must_hide_old_values() {
        let mut c = LayeredCache::new();
        c.put("a", "old", 1);
        c.invalidate_all();
        assert!(c.get("a").is_none(), "stale L2 must not be visible after invalidate_all");
    }

    #[test]
    fn put_after_invalidate_all_is_visible() {
        let mut c = LayeredCache::new();
        c.put("a", "old", 1);
        c.invalidate_all();
        c.put("a", "new", 2);
        assert_eq!(c.get("a").map(|e| e.value.as_str()), Some("new"));
    }

    #[test]
    fn promote_does_not_resurrect_stale() {
        let mut c = LayeredCache::new();
        c.put("a", "old", 1);
        c.invalidate_all();
        c.promote("a");
        assert!(c.get("a").is_none());
    }

    #[test]
    fn version_is_preserved() {
        let mut c = LayeredCache::new();
        c.put("k", "v", 9);
        assert_eq!(c.get("k").map(|e| e.version), Some(9));
    }
}
