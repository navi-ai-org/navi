use thiserror::Error;

#[derive(Debug, Error)]
pub enum BrokerError {
    #[error("access denied: {reason}")]
    AccessDenied { reason: String },

    #[error("not found: {path}")]
    NotFound { path: String },

    #[error("too large: {size_bytes} bytes exceeds limit of {limit_bytes} bytes")]
    TooLarge { size_bytes: u64, limit_bytes: u64 },

    #[error(
        "budget exceeded: total bytes read ({total_bytes}) exceeds invocation budget ({budget_bytes})"
    )]
    BudgetExceeded { total_bytes: u64, budget_bytes: u64 },

    #[error("outside project: path escapes project root")]
    OutsideProject,

    #[error("invalid utf-8: file content is not valid UTF-8")]
    InvalidUtf8,

    #[error("symlink cycle: exceeded {max_hops} hops resolving symlinks")]
    SymlinkCycle { max_hops: u32 },

    #[error("invalid url: {url}")]
    InvalidUrl { url: String },

    #[error("host not allowed: {host}")]
    HostNotAllowed { host: String },

    #[error("ip blocked: {ip} ({reason})")]
    IpBlocked { ip: String, reason: String },

    #[error("rate limited")]
    RateLimited,

    #[error("git command timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
