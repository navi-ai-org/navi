//! Tiny in-memory document DB: pages, WAL, snapshots.
//! Bugs span modules — fixing only one file is usually not enough.

mod page;
mod wal;
mod snapshot;
mod engine;

pub use engine::{Db, DbError};
pub use page::PageId;
