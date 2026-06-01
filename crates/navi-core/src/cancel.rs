use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

/// A cooperative cancellation token that signals the current turn to stop.
/// Cloneable so it can be shared across the runtime and UI layers.
#[derive(Clone)]
pub struct CancelToken {
    pub(crate) requested: Arc<AtomicBool>,
    pub(crate) notify: Arc<Notify>,
}

impl CancelToken {
    /// Creates a new, un-cancelled token.
    pub fn new() -> Self {
        Self {
            requested: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Returns `true` if cancellation has been requested.
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Requests cancellation and wakes all waiters.
    pub fn cancel(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Resets the token to an un-cancelled state.
    pub fn reset(&mut self) {
        self.requested.store(false, Ordering::SeqCst);
        self.notify = Arc::new(Notify::new());
    }

    /// Returns a future that completes when cancellation is requested.
    pub fn notified(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.notify.notified()
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}
