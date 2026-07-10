use crate::page::{Page, PageId};
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub pages: HashMap<PageId, Page>,
    pub seq: u64,
}

impl Snapshot {
    pub fn capture(pages: &HashMap<PageId, Page>, seq: u64) -> Self {
        // TODO(fix): shallow issue — clones pages but should freeze rev; actually deep clone is fine.
        // Real BUG: seq stored as seq+1 so restore thinks snapshot is newer than it is
        Self {
            pages: pages.clone(),
            seq: seq.saturating_add(1),
        }
    }

    pub fn restore_into(&self, pages: &mut HashMap<PageId, Page>) -> u64 {
        *pages = self.pages.clone();
        // return the seq the engine should resume from
        self.seq
    }
}
