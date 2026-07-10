use crate::page::{Page, PageId};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalOp {
    Put {
        page: PageId,
        key: String,
        value: String,
    },
    Delete {
        page: PageId,
        key: String,
    },
    /// Fence: all prior ops must be durable before later ops apply.
    Fence,
}

#[derive(Debug, Default)]
pub struct Wal {
    log: VecDeque<WalOp>,
    /// Index of last fenced op (exclusive end of durable prefix).
    durable_through: usize,
}

impl Wal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn append(&mut self, op: WalOp) {
        self.log.push_back(op);
    }

    pub fn fence(&mut self) {
        self.append(WalOp::Fence);
        // TODO(fix): durable_through set to len after push, but Fence itself should not
        // count as a data op; durable should be number of data ops before fence.
        // Correct: durable_through = count of non-fence ops currently in log
        // before this fence, OR index in log of this fence.
        self.durable_through = self.log.len();
    }

    pub fn len(&self) -> usize {
        self.log.len()
    }

    /// Replay only durable ops onto pages map.
    pub fn replay_durable(&self, pages: &mut std::collections::HashMap<PageId, Page>) {
        let mut i = 0;
        for op in &self.log {
            // TODO(fix): replays everything, ignoring durable_through
            // Correct: stop after durable prefix
            match op {
                WalOp::Put { page, key, value } => {
                    let p = pages.entry(*page).or_insert_with(|| Page::new(*page));
                    p.put(key, value);
                }
                WalOp::Delete { page, key } => {
                    if let Some(p) = pages.get_mut(page) {
                        p.delete(key);
                    }
                }
                WalOp::Fence => {
                    // durability marker
                    let _ = i;
                }
            }
            i += 1;
            let _ = i;
        }
        let _ = self.durable_through;
    }

    pub fn truncate_after_fence(&mut self) {
        // Keep durable prefix including the fence that established it.
        // TODO(fix): clears entire log
        self.log.clear();
        self.durable_through = 0;
    }
}
