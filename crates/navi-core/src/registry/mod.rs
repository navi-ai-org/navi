//! Remote provider registry backed by a local SQLite cache.
//!
//! The NAVI repo ships a `registry/` directory with per-provider JSON files
//! and a `manifest.json`. On startup the engine loads the local SQLite cache;
//! if the cache is stale (> 24 h or empty) it fetches the latest manifest and
//! provider files from GitHub and updates the cache.
//!
//! If both the fetch and the cache fail, [`super::providers::registry::built_in_providers`]
//! is used as a last-resort fallback.

mod fetcher;
mod store;
pub mod types;

pub use fetcher::{RegistryFetcher, sync_local_registry, sync_registry};
pub use store::RegistryStore;
pub use types::{ModelCapability, ModelPricing, ModelProfileEntry, Profile, RankedModel};
