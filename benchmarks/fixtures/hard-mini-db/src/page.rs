use std::collections::BTreeMap;

pub type PageId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub id: PageId,
    pub rev: u64,
    pub data: BTreeMap<String, String>,
}

impl Page {
    pub fn new(id: PageId) -> Self {
        Self {
            id,
            rev: 0,
            data: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
        // TODO(fix): rev should increment on every put; currently only on first write
        if self.rev == 0 {
            self.rev = 1;
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }

    pub fn delete(&mut self, key: &str) -> bool {
        let removed = self.data.remove(key).is_some();
        if removed {
            // TODO(fix): forgets to bump rev on delete
        }
        removed
    }
}
