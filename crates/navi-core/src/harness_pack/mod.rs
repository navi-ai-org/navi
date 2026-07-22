//! Harness packs: local compilation of skills into loop/graph specs.
//!
//! Storage: `{data_dir}/harnesses/<skill-id>/` with `loop.toml` and optional
//! `graph.toml`. Distinct from the product "harness profile" (`small`/`medium`).

mod apply;
mod capability;
mod inventory_build;
mod materialize;
mod store;
mod types;

pub use apply::{
    HarnessApplyResult, apply_harness_for_skills, effective_allow_tools_for_pack, merge_allow_tools,
};
pub use capability::{
    CapabilityInventory, capability_card, filter_tools_to_inventory, inventory_from_tool_names,
};
pub use inventory_build::{build_capability_inventory, exposure_list_from_metadata};
pub use materialize::{MaterializeOptions, materialize_from_skill};
pub use store::{HarnessPackStore, harness_pack_dir, list_harness_ids, load_pack, write_pack};
pub use types::{
    GraphEdge, GraphNode, GraphSpec, HarnessPack, LoopSpec, LoopStop, VerifierKind, VerifierSpec,
};
