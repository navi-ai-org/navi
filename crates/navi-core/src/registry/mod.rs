//! Remote provider registry backed by a local SQLite cache.
//!
//! The NAVI repo ships a `registry-snapshot/` directory with per-provider JSON
//! files and a `manifest.json`, embedded into the binary at build time. On
//! startup the engine loads the local SQLite cache; if the cache is stale
//! (> 24 h or empty) it fetches the latest manifest and provider files from the
//! remote registry database repo and updates the cache.
//!
//! If both the fetch and the cache fail, the embedded snapshot is used as a
//! last-resort fallback.

mod embedded;
mod fetcher;
mod store;
pub mod types;

pub use embedded::{embedded_manifest, embedded_providers};
pub use fetcher::{RegistryFetcher, sync_local_registry, sync_registry};
pub use store::{RegistryStore, registry_provider_to_config};
pub use types::{ModelCapability, ModelPricing, ModelProfileEntry, Profile, RankedModel};
