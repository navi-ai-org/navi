use crate::event::AgentEvent;
use crate::session::{
    SessionId, SessionRuntime, SessionStore, current_unix_timestamp, session_title_from_events,
};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

type EventReceiverSlot = Arc<Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>>;

pub(crate) struct SessionEventReceiver {
    slot: EventReceiverSlot,
    rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,
}

impl SessionEventReceiver {
    pub(crate) async fn recv(&mut self) -> Option<AgentEvent> {
        match self.rx.as_mut() {
            Some(rx) => rx.recv().await,
            None => None,
        }
    }

    pub(crate) fn try_recv(&mut self) -> Result<AgentEvent, mpsc::error::TryRecvError> {
        match self.rx.as_mut() {
            Some(rx) => rx.try_recv(),
            None => Err(mpsc::error::TryRecvError::Disconnected),
        }
    }
}

impl Drop for SessionEventReceiver {
    fn drop(&mut self) {
        let Some(rx) = self.rx.take() else {
            return;
        };
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(rx);
        } else {
            tracing::warn!("session event stream receiver was replaced before checkout returned");
        }
    }
}

pub struct SessionState {
    id: SessionId,
    created_at: u64,
    updated_at: u64,
    title: Option<String>,
    turn_sequence: u64,
    started: bool,
    requested_id: Option<SessionId>,
    runtime: Option<SessionRuntime>,
    event_rx: EventReceiverSlot,
    events: Vec<AgentEvent>,
    initial_events: Vec<AgentEvent>,
    initial_created_at: Option<u64>,
    initial_updated_at: Option<u64>,
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
            event_rx: Arc::new(Mutex::new(None)),
            events: Vec::new(),
            initial_events: Vec::new(),
            initial_created_at: None,
            initial_updated_at: None,
        }
    }

    pub fn new_with_history(
        requested_id: Option<SessionId>,
        events: Vec<AgentEvent>,
        created_at: Option<u64>,
        updated_at: Option<u64>,
    ) -> Self {
        Self {
            initial_events: events,
            initial_created_at: created_at,
            initial_updated_at: updated_at,
            ..Self::new(requested_id)
        }
    }

    pub fn start(&mut self) {
        let now = current_unix_timestamp();
        let initial_events = std::mem::take(&mut self.initial_events);
        self.id = self
            .requested_id
            .take()
            .unwrap_or_else(SessionStore::create_id);
        self.created_at = self.initial_created_at.take().unwrap_or(now);
        self.updated_at = self.initial_updated_at.take().unwrap_or(self.created_at);
        self.title = session_title_from_events(&initial_events);
        self.turn_sequence = initial_events
            .iter()
            .filter(|event| matches!(event, AgentEvent::UserTaskSubmitted { .. }))
            .count() as u64;
        self.started = true;
        self.events = initial_events;
        self.runtime = None;
        *self.event_rx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Set an explicit session title (provisional user-derived or model-named).
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title.filter(|t| !t.trim().is_empty());
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
        *self.event_rx.lock().unwrap_or_else(|e| e.into_inner()) = Some(event_rx);
    }

    pub(crate) fn take_event_rx(&mut self) -> Option<SessionEventReceiver> {
        let rx = self
            .event_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()?;
        Some(SessionEventReceiver {
            slot: Arc::clone(&self.event_rx),
            rx: Some(rx),
        })
    }

    pub fn next_turn_id(&mut self) -> String {
        self.turn_sequence += 1;
        format!("{}-turn-{}", self.id.as_str(), self.turn_sequence)
    }

    pub fn push_event(&mut self, event: AgentEvent) {
        // Title is derived lazily via `update_title_from_events` (end of turn /
        // snapshot) rather than walking the full event list on every push.
        self.events.push(event);
        self.updated_at = current_unix_timestamp();
    }

    /// Drop events from the `(keep_user_turns + 1)`-th `UserTaskSubmitted` onward.
    ///
    /// Used when rewinding the live session after the UI edits a past user message.
    pub fn truncate_events_to_user_turns(&mut self, keep_user_turns: usize) {
        let mut seen = 0usize;
        let mut cut = self.events.len();
        for (i, event) in self.events.iter().enumerate() {
            if matches!(event, AgentEvent::UserTaskSubmitted { .. }) {
                if seen == keep_user_turns {
                    cut = i;
                    break;
                }
                seen += 1;
            }
        }
        self.events.truncate(cut);
        self.turn_sequence = keep_user_turns as u64;
        self.updated_at = current_unix_timestamp();
        self.force_update_title_from_events();
    }

    /// Derive a title from events only when none is set yet.
    ///
    /// Keeps provisional / model-assigned titles stable across mid-turn updates
    /// and end-of-turn snapshots.
    pub fn update_title_from_events(&mut self) {
        if self.title.is_some() {
            return;
        }
        self.title = session_title_from_events(&self.events);
    }

    /// Force re-derive from events (e.g. after rewind).
    pub fn force_update_title_from_events(&mut self) {
        self.title = session_title_from_events(&self.events);
    }

    pub fn snapshot(
        &self,
        project_dir: &std::path::Path,
        session_store: &SessionStore,
        event_bus: &crate::runtime::EventBus,
        goal: Option<crate::goal::types::SessionGoal>,
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
                project: project_dir.to_path_buf(),
                created_at: self.created_at,
                updated_at: current_unix_timestamp(),
                events: self.events.clone(),
                memory,
                goal,
                usage: None,
            };
            session_store.save(&snap)?;
            Ok::<_, anyhow::Error>(snap)
        })?;
        event_bus.publish(crate::event::RuntimeEventKind::SessionSaved {
            session_id: snapshot.id.as_str().to_string(),
        });
        Ok(snapshot)
    }

    pub async fn snapshot_async(
        &self,
        project_dir: &std::path::Path,
        session_store: &SessionStore,
        event_bus: &crate::runtime::EventBus,
        goal: Option<crate::goal::types::SessionGoal>,
    ) -> Result<crate::session::SessionSnapshot> {
        let memory = session_store
            .load_memory_async(project_dir.to_path_buf())
            .await;
        let snapshot = crate::session::SessionSnapshot {
            version: crate::session::SessionSnapshot::CURRENT_VERSION,
            id: self.id.clone(),
            title: self.title.clone(),
            project: project_dir.to_path_buf(),
            created_at: self.created_at,
            updated_at: current_unix_timestamp(),
            events: self.events.clone(),
            memory,
            goal,
            usage: None,
        };
        session_store.save_async(snapshot.clone()).await?;
        event_bus.publish(crate::event::RuntimeEventKind::SessionSaved {
            session_id: snapshot.id.as_str().to_string(),
        });
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AgentEvent;

    #[test]
    fn push_event_does_not_recompute_title_eagerly() {
        let mut state = SessionState::new(None);
        state.start();
        state.push_event(AgentEvent::UserTaskSubmitted {
            text: "build a dashboard".to_string(),
            content_parts: vec![],
            submitted_at: None,
        });
        assert!(
            state.title().is_none(),
            "title should stay lazy until update_title_from_events"
        );
        state.update_title_from_events();
        assert_eq!(state.title(), Some("build a dashboard"));
    }

    #[test]
    fn update_title_prefers_model_heading_over_user_task() {
        let mut state = SessionState::new(None);
        state.start();
        state.push_event(AgentEvent::UserTaskSubmitted {
            text: "build a dashboard".to_string(),
            content_parts: vec![],
            submitted_at: None,
        });
        state.push_event(AgentEvent::ModelOutput {
            text: "## Analytics Board\n\nDone.".to_string(),
            thinking: None,
        });
        state.update_title_from_events();
        assert_eq!(state.title(), Some("Analytics Board"));
    }
}
