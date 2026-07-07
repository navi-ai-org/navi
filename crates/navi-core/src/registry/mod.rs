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

mod aggregator;
mod embedded;
mod fetcher;
mod store;
pub mod types;
mod update;

pub use aggregator::sync_aggregator_models;
pub use embedded::{embedded_manifest, embedded_provider_schema, embedded_providers};
pub use fetcher::{RegistryFetcher, sync_local_registry, sync_registry};
pub use store::{RegistryStore, registry_provider_to_config};
pub use types::{ModelCapability, ModelPricing, ModelProfileEntry, Profile, RankedModel};
pub use update::{
    LoadedRegistry, RegistryFetcherTrait, RegistrySource, apply_registry_update_atomically,
    check_registry_manifest, download_registry_updates, load_cached_registry,
    load_embedded_registry, load_registry, registry_check_interval_with_jitter,
    run_registry_update_check, save_registry_metadata, should_check_registry_update,
    validate_registry_hashes, validate_registry_schema,
};
