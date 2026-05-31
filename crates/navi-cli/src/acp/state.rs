use navi_sdk::NaviEngine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub(crate) struct AcpState {
    pub(crate) engine: NaviEngine,
    pub(crate) default_project_dir: PathBuf,
    pub(crate) sessions: Arc<Mutex<HashMap<String, AcpSession>>>,
}

pub(crate) struct AcpSession {
    pub(crate) project_dir: PathBuf,
    pub(crate) sdk_started: bool,
    pub(crate) task: Option<ActivePrompt>,
}

pub(crate) struct ActivePrompt {
    pub(crate) cancel_tx: tokio::sync::oneshot::Sender<()>,
}

impl AcpState {
    pub(crate) fn with_sessions<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&HashMap<String, AcpSession>) -> T,
    {
        let sessions = self.sessions.lock().expect("session lock poisoned");
        f(&sessions)
    }

    pub(crate) fn with_sessions_mut<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut HashMap<String, AcpSession>) -> T,
    {
        let mut sessions = self.sessions.lock().expect("session lock poisoned");
        f(&mut sessions)
    }
}
