use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("WASM engine error: {0}")]
    Engine(#[from] wasmtime::Error),

    #[error("tool '{tool_name}' not found in plugin")]
    ToolNotFound { tool_name: String },

    #[error("fuel exhausted: plugin consumed all allocated fuel")]
    FuelExhausted,

    #[error("memory limit exceeded: plugin tried to allocate beyond {limit_mb} MB")]
    MemoryLimitExceeded { limit_mb: u64 },

    #[error("timeout: plugin exceeded {timeout_secs}s wall-clock limit")]
    Timeout { timeout_secs: u64 },

    #[error("output too large: {size_bytes} bytes exceeds {limit_bytes} bytes")]
    OutputTooLarge {
        size_bytes: usize,
        limit_bytes: usize,
    },

    #[error("invalid input JSON: {0}")]
    InvalidInput(String),

    #[error("plugin returned error: {0}")]
    PluginError(String),
}
