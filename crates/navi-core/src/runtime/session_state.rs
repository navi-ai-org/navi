use crate::event::AgentEvent;
use crate::session::{
    SessionId, SessionRuntime, SessionStore, current_unix_timestamp, session_title_from_events,
};
use anyhow::Result;
use tokio::sync::mpsc;

pub struct SessionState {
    id: SessionId,
    created_at: u64,
    updated_at: u64,
    title: Option<String>,
    turn_sequence: u64,
    started: bool,
    requested_id: Option<SessionId>,
    runtime: Option<SessionRuntime>,
    event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
    events: Vec<AgentEvent>,
}

impl SessionState {
    pub fn new(requested_id: Option<SessionId>) -> Self {
        let now = current_unix_timestamp();
        Self {
            id: requested_id.clone().unwrap_or_else(SessionStore::create_id),
            created_at: now,
            updated_at: now,
            title: None,
            turn_sequence: 0,
            started: false,
            requested_id,
            runtime: None,
            event_rx: None,
            events: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        self.id = self
            .requested_id
            .take()
            .unwrap_or_else(SessionStore::create_id);
        self.created_at = current_unix_timestamp();
        self.updated_at = self.created_at;
        self.title = None;
        self.turn_sequence = 0;
        self.started = true;
        self.events.clear();
        self.runtime = None;
        self.event_rx = None;
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }

    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    pub fn updated_at(&self) -> u64 {
        self.updated_at
    }

    pub fn set_updated_at(&mut self, ts: u64) {
        self.updated_at = ts;
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn runtime(&self) -> Option<&SessionRuntime> {
        self.runtime.as_ref()
    }

    pub fn set_runtime(
        &mut self,
        runtime: SessionRuntime,
        event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    ) {
        self.runtime = Some(runtime);
        self.event_rx = Some(event_rx);
    }

    pub fn take_event_rx(&mut self) -> Option<mpsc::UnboundedReceiver<AgentEvent>> {
        self.event_rx.take()
    }

    pub fn set_event_rx(&mut self, rx: mpsc::UnboundedReceiver<AgentEvent>) {
        self.event_rx = Some(rx);
    }

    pub fn next_turn_id(&mut self) -> String {
        self.turn_sequence += 1;
        format!("{}-turn-{}", self.id.as_str(), self.turn_sequence)
    }

    pub fn push_event(&mut self, event: AgentEvent) {
        self.events.push(event);
        self.updated_at = current_unix_timestamp();
        self.title = session_title_from_events(&self.events);
    }

    pub fn update_title_from_events(&mut self) {
        self.title = session_title_from_events(&self.events);
    }

    pub fn snapshot(
        &self,
        project_dir: &std::path::PathBuf,
        session_store: &SessionStore,
        event_bus: &crate::runtime::EventBus,
    ) -> Result<crate::session::SessionSnapshot> {
        // SessionStore I/O is intentionally blocking, but callers invoke this
        // from the Tokio runtime. Use `block_in_place` so the runtime can
        // reschedule other tasks on the current thread while the filesystem
        // work completes.
        let snapshot = tokio::task::block_in_place(|| {
            let memory = session_store.load_memory(project_dir);
            let snap = crate::session::SessionSnapshot {
                version: crate::session::SessionSnapshot::CURRENT_VERSION,
                id: self.id.clone(),
                title: self.title.clone(),
                project: project_dir.clone(),
                created_at: self.created_at,
                updated_at: current_unix_timestamp(),
                events: self.events.clone(),
                memory,
            };
            session_store.save(&snap)?;
            Ok::<_, anyhow::Error>(snap)
        })?;
        event_bus.publish(crate::event::RuntimeEventKind::SessionSaved {
            session_id: snapshot.id.as_str().to_string(),
        });
        Ok(snapshot)
    }
}
