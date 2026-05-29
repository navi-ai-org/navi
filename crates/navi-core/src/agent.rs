use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// The active agent mode, controlling system prompt behavior, allowed tools,
/// output style, and approval policy.
///
/// Modes are real runtime state, not just slash-command prefixes. They affect
/// how the agent reasons about and responds to user tasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AgentMode {
    /// Inspect and reason before proposing action; outline steps and constraints.
    Plan,
    /// Make targeted project changes using available tools, then verify them.
    Edit,
    /// Prioritize bugs, regressions, risks, and missing tests.
    Review,
    /// Help the user learn actively without taking over.
    Tutor,
    /// Ask focused questions that expose assumptions and guide reasoning.
    Socratic,
    /// Prompt retrieval practice and check understanding.
    Recall,
    /// Keep attention on the current focus thread; defer unrelated ideas.
    Focus,
}

impl AgentMode {
    /// Returns a lowercase display label for this mode (e.g. `"plan"`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Edit => "edit",
            Self::Review => "review",
            Self::Tutor => "tutor",
            Self::Socratic => "socratic",
            Self::Recall => "recall",
            Self::Focus => "focus",
        }
    }

    /// Returns the slash-command string for this mode (e.g. `"/plan"`).
    pub fn command(self) -> &'static str {
        match self {
            Self::Plan => "/plan",
            Self::Edit => "/edit",
            Self::Review => "/review",
            Self::Tutor => "/tutor",
            Self::Socratic => "/socratic",
            Self::Recall => "/recall",
            Self::Focus => "/focus",
        }
    }

    /// Cycles to the next code-focused mode (Plan -> Edit -> Review -> Plan).
    ///
    /// Non-code modes (Tutor, Socratic, Recall, Focus) wrap back to Plan.
    pub fn next_code_mode(self) -> Self {
        match self {
            Self::Plan => Self::Edit,
            Self::Edit => Self::Review,
            _ => Self::Plan,
        }
    }

    /// Parses a slash-command string (e.g. `"/plan"`) into the corresponding mode.
    ///
    /// Returns `None` for unrecognized commands.
    pub fn from_command(command: &str) -> Option<Self> {
        match command {
            "/plan" => Some(Self::Plan),
            "/edit" => Some(Self::Edit),
            "/review" => Some(Self::Review),
            "/tutor" => Some(Self::Tutor),
            "/socratic" => Some(Self::Socratic),
            "/recall" => Some(Self::Recall),
            "/focus" => Some(Self::Focus),
            _ => None,
        }
    }

    /// Extracts an agent mode from the first word of prompt text if it starts
    /// with a recognized slash-command.
    pub fn from_prompt_text(text: &str) -> Option<Self> {
        text.split_whitespace().next().and_then(Self::from_command)
    }

    /// Prepends this mode's slash-command to the prompt text unless the text
    /// already starts with a recognized mode command.
    pub fn apply_to_prompt(self, text: &str) -> String {
        let text = text.trim();
        if text.is_empty() || Self::from_prompt_text(text).is_some() {
            text.to_string()
        } else {
            format!("{} {text}", self.command())
        }
    }

    /// Returns mode-specific instructions injected into the system prompt to
    /// guide the agent's behavior for this mode.
    pub fn runtime_instructions(self) -> &'static str {
        match self {
            Self::Plan => {
                "Agent mode: Plan. Inspect and reason before proposing action. Prefer outlining steps and constraints before editing."
            }
            Self::Edit => {
                "Agent mode: Edit. Make targeted project changes using available tools, then verify them."
            }
            Self::Review => {
                "Agent mode: Review. Prioritize bugs, regressions, risks, and missing tests. Findings lead."
            }
            Self::Tutor => {
                "Agent mode: Tutor. Help the user learn actively. Explain enough to guide thinking without taking over."
            }
            Self::Socratic => {
                "Agent mode: Socratic. Ask focused questions that expose assumptions and guide the next reasoning step."
            }
            Self::Recall => {
                "Agent mode: Recall. Prompt retrieval practice and check understanding before adding new material."
            }
            Self::Focus => {
                "Agent mode: Focus. Keep attention on the current focus thread and defer unrelated ideas."
            }
        }
    }
}

/// A message exchanged between named agents in a multi-agent setup.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentMessage {
    /// Identifier of the sending agent.
    pub from: String,
    /// Identifier of the target agent.
    pub to: String,
    /// Message content.
    pub content: String,
}

/// A registry of named agent mailboxes for inter-agent communication.
#[derive(Clone, Default)]
pub struct AgentControl {
    senders: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<AgentMessage>>>>,
}

impl AgentControl {
    /// Creates an empty agent control with no registered agents.
    pub fn new() -> Self {
        Self {
            senders: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Registers an agent with the given id, allowing it to receive messages.
    pub fn register_agent(&self, id: String, sender: mpsc::UnboundedSender<AgentMessage>) {
        let mut senders = self.senders.lock().unwrap_or_else(|e| e.into_inner());
        senders.insert(id, sender);
    }

    /// Removes the agent with the given id from the registry.
    pub fn unregister_agent(&self, id: &str) {
        let mut senders = self.senders.lock().unwrap_or_else(|e| e.into_inner());
        senders.remove(id);
    }

    /// Sends a message to the agent identified by `message.to`.
    ///
    /// Returns an error if the target agent is not registered.
    pub fn send_message(&self, message: AgentMessage) -> Result<(), String> {
        let senders = self.senders.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(sender) = senders.get(&message.to) {
            sender.send(message).map_err(|e| e.to_string())?;
            Ok(())
        } else {
            Err(format!("Agent with ID {} not found", message.to))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentMode;

    #[test]
    fn agent_mode_applies_command_without_duplication() {
        assert_eq!(
            AgentMode::Review.apply_to_prompt("check the diff"),
            "/review check the diff"
        );
        assert_eq!(
            AgentMode::Review.apply_to_prompt("/plan inspect first"),
            "/plan inspect first"
        );
    }

    #[test]
    fn code_mode_cycle_stays_in_code_agents() {
        assert_eq!(AgentMode::Plan.next_code_mode(), AgentMode::Edit);
        assert_eq!(AgentMode::Edit.next_code_mode(), AgentMode::Review);
        assert_eq!(AgentMode::Review.next_code_mode(), AgentMode::Plan);
        assert_eq!(AgentMode::Tutor.next_code_mode(), AgentMode::Plan);
    }
}
