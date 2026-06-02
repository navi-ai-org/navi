pub mod approval;
pub mod marketplace;
pub mod classifier;
pub mod defaults;
pub mod error;
pub mod hash;
pub mod lockfile;
pub mod parser;
pub mod registry;
pub mod risk;
pub mod signature;
pub mod store;
pub mod types;
pub mod validator;

pub use approval::verify_approved_capabilities;
pub use classifier::classify_tool_risk;
pub use defaults::*;
pub use error::{ManifestError, ValidationError};
pub use hash::{compute_content_hash, compute_wasm_hash, verify_wasm_hash};
pub use marketplace::{
    DEFAULT_REGISTRY_URL, MarketplaceError, PluginCatalog, PluginCatalogEntry, fetch_catalog,
    find_catalog_entry, parse_catalog, plugin_staging_dir, registry_url, search_catalog,
    stage_plugin_by_id, stage_plugin_from_catalog,
};
pub use lockfile::{LockEntry, Lockfile};
pub use parser::parse_manifest;
pub use registry::{RegisteredTool, RegistryError, ToolRegistry, sanitize_description};
pub use risk::{RiskAssessment, RiskLevel};
pub use signature::{
    compute_hash_bundle, sign_plugin_manifest, sign_plugin_manifest_for_tests,
    verify_manifest_signature, verify_plugin_signature,
};
pub use store::{
    AGGREGATE_LOCKFILE_NAME, INSTALLED_PLUGINS_SUBDIR, aggregate_lockfile_path,
    capabilities_hash_from_manifest, installed_plugin_dir, installed_plugins_dir,
    lock_entry_from_manifest, migrate_legacy_per_plugin_lockfiles, remove_aggregate_lock_entry,
    tools_hash_from_manifest, upsert_aggregate_lock_entry,
};
pub use types::*;
pub use validator::validate;
