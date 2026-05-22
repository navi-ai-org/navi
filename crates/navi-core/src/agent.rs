use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentMessage {
    pub from: String,
    pub to: String,
    pub content: String,
}

#[derive(Clone, Default)]
pub struct AgentControl {
    senders: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<AgentMessage>>>>,
}

impl AgentControl {
    pub fn new() -> Self {
        Self {
            senders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_agent(&self, id: String, sender: mpsc::UnboundedSender<AgentMessage>) {
        let mut senders = self.senders.lock().unwrap();
        senders.insert(id, sender);
    }

    pub fn unregister_agent(&self, id: &str) {
        let mut senders = self.senders.lock().unwrap();
        senders.remove(id);
    }

    pub fn send_message(&self, message: AgentMessage) -> Result<(), String> {
        let senders = self.senders.lock().unwrap();
        if let Some(sender) = senders.get(&message.to) {
            sender.send(message).map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err(format!("Agent with ID {} not found", message.to))
        }
    }
}
