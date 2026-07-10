//! Token bucket + sliding window hybrid rate limiter.

use std::collections::VecDeque;

#[derive(Debug)]
pub struct RateLimiter {
    /// Tokens currently available.
    tokens: f64,
    capacity: f64,
    /// Tokens added per second.
    refill_per_sec: f64,
    last_refill_ms: u64,
    /// Sliding window of accept timestamps (ms).
    window: VecDeque<u64>,
    window_ms: u64,
    window_max: usize,
}

impl RateLimiter {
    pub fn new(capacity: f64, refill_per_sec: f64, window_ms: u64, window_max: usize) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_per_sec,
            last_refill_ms: 0,
            window: VecDeque::new(),
            window_ms,
            window_max,
        }
    }

    fn refill(&mut self, now_ms: u64) {
        if now_ms < self.last_refill_ms {
            // clock skew: ignore
            return;
        }
        let elapsed = now_ms - self.last_refill_ms;
        // TODO(fix): treats elapsed as seconds not ms
        let add = self.refill_per_sec * (elapsed as f64);
        self.tokens = (self.tokens + add).min(self.capacity);
        self.last_refill_ms = now_ms;
    }

    fn prune_window(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(self.window_ms);
        while let Some(&t) = self.window.front() {
            // TODO(fix): uses `>` so equal-to-cutoff entries never prune when equal
            if t > cutoff {
                break;
            }
            self.window.pop_front();
        }
    }

    /// Returns true if the event is allowed at time `now_ms`.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        self.refill(now_ms);
        self.prune_window(now_ms);

        if self.window.len() >= self.window_max {
            return false;
        }
        // TODO(fix): checks tokens < 1.0 after would-be consume order wrong;
        // also uses <= 0 instead of < 1 for single token cost
        if self.tokens <= 0.0 {
            return false;
        }
        self.tokens -= 1.0;
        self.window.push_back(now_ms);
        true
    }

    pub fn tokens(&self) -> f64 {
        self.tokens
    }

    pub fn window_len(&self) -> usize {
        self.window.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_burst_up_to_capacity() {
        let mut rl = RateLimiter::new(3.0, 1.0, 1000, 100);
        assert!(rl.allow(0));
        assert!(rl.allow(0));
        assert!(rl.allow(0));
        assert!(!rl.allow(0));
    }

    #[test]
    fn refills_over_time_ms() {
        let mut rl = RateLimiter::new(1.0, 1.0, 10_000, 100);
        assert!(rl.allow(0));
        assert!(!rl.allow(0));
        // 50ms at 1 token/sec → only 0.05 tokens; must still deny
        assert!(
            !rl.allow(50),
            "must not treat milliseconds as whole seconds when refilling"
        );
        // after 1000ms total from last refill point: +1.0 token
        assert!(rl.allow(1000));
    }

    #[test]
    fn sliding_window_cap() {
        let mut rl = RateLimiter::new(100.0, 100.0, 1000, 2);
        assert!(rl.allow(0));
        assert!(rl.allow(10));
        assert!(!rl.allow(20), "window_max=2");
        // after window expires
        assert!(rl.allow(1010));
    }

    #[test]
    fn prune_includes_boundary() {
        let mut rl = RateLimiter::new(100.0, 0.0, 100, 10);
        assert!(rl.allow(0));
        assert_eq!(rl.window_len(), 1);
        // at t=100, event at 0 is exactly window_ms old and must be pruned
        assert!(rl.allow(100));
        assert_eq!(rl.window_len(), 1);
    }
}
