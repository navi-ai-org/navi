use crate::page::{Page, PageId};
use crate::snapshot::Snapshot;
use crate::wal::{Wal, WalOp};
use std::collections::HashMap;

#[derive(Debug)]
pub enum DbError {
    MissingPage(PageId),
    Conflict { expected_rev: u64, actual_rev: u64 },
    Other(String),
}

pub struct Db {
    pages: HashMap<PageId, Page>,
    wal: Wal,
    seq: u64,
    snapshot: Option<Snapshot>,
}

impl Db {
    pub fn open() -> Self {
        Self {
            pages: HashMap::new(),
            wal: Wal::new(),
            seq: 0,
            snapshot: None,
        }
    }

    pub fn ensure_page(&mut self, id: PageId) {
        self.pages.entry(id).or_insert_with(|| Page::new(id));
    }

    pub fn put(&mut self, page: PageId, key: &str, value: &str) -> Result<(), DbError> {
        self.ensure_page(page);
        self.wal.append(WalOp::Put {
            page,
            key: key.to_string(),
            value: value.to_string(),
        });
        let p = self.pages.get_mut(&page).unwrap();
        p.put(key, value);
        self.seq += 1;
        Ok(())
    }

    pub fn delete(&mut self, page: PageId, key: &str) -> Result<bool, DbError> {
        self.ensure_page(page);
        self.wal.append(WalOp::Delete {
            page,
            key: key.to_string(),
        });
        let p = self.pages.get_mut(&page).unwrap();
        let removed = p.delete(key);
        if removed {
            self.seq += 1;
        }
        Ok(removed)
    }

    pub fn get(&self, page: PageId, key: &str) -> Option<&str> {
        self.pages.get(&page).and_then(|p| p.get(key))
    }

    pub fn rev(&self, page: PageId) -> Option<u64> {
        self.pages.get(&page).map(|p| p.rev)
    }

    pub fn commit(&mut self) {
        self.wal.fence();
    }

    pub fn checkpoint(&mut self) {
        self.snapshot = Some(Snapshot::capture(&self.pages, self.seq));
    }

    /// Recover as-of last commit (durable WAL), discarding unfenced ops in memory
    /// by rebuilding from empty + durable WAL. Then re-apply nothing after fence.
    pub fn recover_durable(&mut self) {
        let mut pages = HashMap::new();
        self.wal.replay_durable(&mut pages);
        self.pages = pages;
        // TODO(fix): does not reset seq from snapshot/pages; leaves seq high
        // Also should truncate undurable tail — call truncate_after_fence
    }

    pub fn restore_checkpoint(&mut self) -> Result<(), DbError> {
        let Some(snap) = self.snapshot.clone() else {
            return Err(DbError::Other("no checkpoint".into()));
        };
        self.seq = snap.restore_into(&mut self.pages);
        Ok(())
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Compare-and-set style write: only if page rev matches expected.
    pub fn put_cas(
        &mut self,
        page: PageId,
        key: &str,
        value: &str,
        expected_rev: u64,
    ) -> Result<(), DbError> {
        self.ensure_page(page);
        let actual = self.pages.get(&page).map(|p| p.rev).unwrap_or(0);
        // TODO(fix): uses != inverted — allows write when rev does NOT match
        if actual == expected_rev {
            return Err(DbError::Conflict {
                expected_rev,
                actual_rev: actual,
            });
        }
        self.put(page, key, value)
    }
}
