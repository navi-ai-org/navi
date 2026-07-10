//! Public API that depends on `config`.

mod config;

pub use config::MAX_RETRIES;

pub fn should_retry(attempt: u32) -> bool {
    attempt < MAX_RETRIES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_up_to_limit() {
        assert!(should_retry(0));
        assert!(should_retry(2));
        assert!(!should_retry(3));
    }
}
