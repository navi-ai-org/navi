/// Maximum number of retries for a flaky operation.
///
/// BUG: this is intentionally wrong so agents must find and fix it.
/// Correct value is 3.
pub const MAX_RETRIES: u32 = 1;
