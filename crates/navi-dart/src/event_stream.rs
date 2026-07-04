// Event subscription handle for navi-dart.

use tokio::task::JoinHandle;

/// Opaque event subscription handle.
///
/// Created by `navi_engine_subscribe_events`, freed by `navi_event_subscription_free`.
pub struct NaviEventSubscription {
    pub _task: JoinHandle<()>,
}
