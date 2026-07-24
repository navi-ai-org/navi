use serde::{Deserialize, Serialize};

/// Represents the structured working state of the current session.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCheckpoint {
    pub intent: String,
    pub next_action: String,
    pub constraints: String,
    pub task_tree: String,
    pub current_work: String,
    pub involved_files: String,
    pub discoveries: String,
    pub errors_fixes: String,
    pub runtime_state: String,
    pub decisions: String,
    pub misc: String,
}

impl SessionCheckpoint {
    /// Parses a `SessionCheckpoint` from its Markdown string representation.
    pub fn from_markdown(text: &str) -> Self {
        let mut checkpoint = Self::default();
        let mut current_section: Option<String> = None;
        let mut current_content = String::new();

        let commit_section = |checkpoint: &mut Self, section: Option<&str>, content: &str| {
            let trimmed = content.trim().to_string();
            if let Some(sec) = section {
                match sec {
                    "Current Intent" => checkpoint.intent = trimmed,
                    "Next Action" => checkpoint.next_action = trimmed,
                    "Working Constraints" => checkpoint.constraints = trimmed,
                    "Task Tree" => checkpoint.task_tree = trimmed,
                    "Current Work" => checkpoint.current_work = trimmed,
                    "Involved Files" => checkpoint.involved_files = trimmed,
                    "Cross-Task Discoveries" => checkpoint.discoveries = trimmed,
                    "Errors and Fixes" => checkpoint.errors_fixes = trimmed,
                    "Runtime State" => checkpoint.runtime_state = trimmed,
                    "Design Decisions" => checkpoint.decisions = trimmed,
                    "Miscellaneous Notes" => checkpoint.misc = trimmed,
                    _ => {}
                }
            }
        };

        for line in text.lines() {
            if line.starts_with("## ") {
                commit_section(
                    &mut checkpoint,
                    current_section.as_deref(),
                    &current_content,
                );
                current_section = Some(line["## ".len()..].trim().to_string());
                current_content.clear();
            } else if line.starts_with("# ") {
                // Ignore main title "# Session Checkpoint"
            } else {
                if current_section.is_some() {
                    current_content.push_str(line);
                    current_content.push('\n');
                }
            }
        }
        commit_section(
            &mut checkpoint,
            current_section.as_deref(),
            &current_content,
        );
        checkpoint
    }

    /// Renders the checkpoint to its standard Markdown format.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Session Checkpoint\n\n");
        out.push_str("## Current Intent\n");
        out.push_str(if self.intent.is_empty() {
            ""
        } else {
            &self.intent
        });
        out.push_str("\n\n## Next Action\n");
        out.push_str(if self.next_action.is_empty() {
            ""
        } else {
            &self.next_action
        });
        out.push_str("\n\n## Working Constraints\n");
        out.push_str(if self.constraints.is_empty() {
            ""
        } else {
            &self.constraints
        });
        out.push_str("\n\n## Task Tree\n");
        out.push_str(if self.task_tree.is_empty() {
            ""
        } else {
            &self.task_tree
        });
        out.push_str("\n\n## Current Work\n");
        out.push_str(if self.current_work.is_empty() {
            ""
        } else {
            &self.current_work
        });
        out.push_str("\n\n## Involved Files\n");
        out.push_str(if self.involved_files.is_empty() {
            ""
        } else {
            &self.involved_files
        });
        out.push_str("\n\n## Cross-Task Discoveries\n");
        out.push_str(if self.discoveries.is_empty() {
            ""
        } else {
            &self.discoveries
        });
        out.push_str("\n\n## Errors and Fixes\n");
        out.push_str(if self.errors_fixes.is_empty() {
            ""
        } else {
            &self.errors_fixes
        });
        out.push_str("\n\n## Runtime State\n");
        out.push_str(if self.runtime_state.is_empty() {
            ""
        } else {
            &self.runtime_state
        });
        out.push_str("\n\n## Design Decisions\n");
        out.push_str(if self.decisions.is_empty() {
            ""
        } else {
            &self.decisions
        });
        out.push_str("\n\n## Miscellaneous Notes\n");
        out.push_str(if self.misc.is_empty() { "" } else { &self.misc });
        out.push('\n');
        out
    }
}
