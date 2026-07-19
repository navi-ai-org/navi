use thiserror::Error;

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("ACP JSON-RPC error {code}: {message}")]
    Rpc { code: i64, message: String },

    #[error("ACP transport closed")]
    TransportClosed,

    #[error("ACP request timed out")]
    Timeout,

    #[error("ACP protocol error: {0}")]
    Protocol(String),

    #[error("failed to spawn ACP agent `{command}`: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, AcpError>;
