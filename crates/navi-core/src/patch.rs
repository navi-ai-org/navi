use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchProposal {
    pub id: String,
    pub summary: String,
    pub files: Vec<PathBuf>,
    pub unified_diff: String,
}
