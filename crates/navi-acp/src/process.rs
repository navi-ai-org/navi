use std::collections::BTreeMap;
use std::path::PathBuf;

/// How to launch an ACP agent server subprocess.
#[derive(Debug, Clone, Default)]
pub struct AcpProcessConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
}
