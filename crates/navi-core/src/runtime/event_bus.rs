use crate::event::{RuntimeEvent, RuntimeEventKind};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<RuntimeEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn stream_events(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, kind: RuntimeEventKind) {
        let _ = self.tx.send(RuntimeEvent::new(kind));
    }

    pub fn sender(&self) -> broadcast::Sender<RuntimeEvent> {
        self.tx.clone()
    }
}
