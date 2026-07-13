use std::future::Future;
use std::pin::pin;
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
    ///
    /// Keeps the shared [`Notify`] so clones (session turn context, UI
    /// canceller) remain connected after reset. Replacing the notify Arc would
    /// strand waiters that still hold an older clone.
    pub fn reset(&mut self) {
        self.requested.store(false, Ordering::SeqCst);
    }

    /// Returns a future that completes when cancellation is requested.
    ///
    /// Completes immediately if cancellation was already requested. The waiter
    /// is registered before the flag is re-checked so a concurrent `cancel()`
    /// cannot be missed between the check and the wait.
    pub fn notified(&self) -> impl Future<Output = ()> + '_ {
        async {
            if self.is_requested() {
                return;
            }
            let mut notified = pin!(self.notify.notified());
            // Register before re-checking the flag (see method docs).
            notified.as_mut().enable();
            if self.is_requested() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn notified_completes_when_already_cancelled() {
        let token = CancelToken::new();
        token.cancel();
        tokio::time::timeout(Duration::from_millis(50), token.notified())
            .await
            .expect("notified must complete immediately when already cancelled");
    }

    #[tokio::test]
    async fn notified_wakes_on_cancel() {
        let token = CancelToken::new();
        let waiter = token.clone();
        let handle = tokio::spawn(async move {
            waiter.notified().await;
        });
        // Yield so the waiter can register before we cancel.
        tokio::task::yield_now().await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("notified should wake promptly")
            .expect("waiter task");
    }

    #[tokio::test]
    async fn reset_clears_flag_but_keeps_clones_connected() {
        let mut token = CancelToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_requested());
        token.reset();
        assert!(!token.is_requested());
        assert!(!clone.is_requested());

        let waiter = clone.clone();
        let handle = tokio::spawn(async move {
            waiter.notified().await;
        });
        tokio::task::yield_now().await;
        clone.cancel();
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("clone cancel must still wake waiters after reset")
            .expect("waiter task");
    }
}
