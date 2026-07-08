//! Plan Mode — a collaboration phase where the agent proposes a plan before execution.
//!
//! Inspired by Codex's Plan Mode, this separates the "think and plan" phase
//! from the "execute" phase. In Plan mode:
//! - Only read-only tools are available (read_file, grep, fs_browser, ast_search, etc.)
//! - The model emits `<proposed_plan>` tags in its text stream
//! - The parser extracts the plan in real-time
//! - When the turn completes, a `PlanProposed` event is emitted
//! - The UI shows a confirmation popup: "Implement this plan?"
//! - If confirmed, a Goal with checklist is created and the mode switches to Default

use crate::tool::ToolKind;
use serde::{Deserialize, Serialize};

/// The collaboration mode of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// Normal execution mode — all tools available, full agentic loop.
    Default,
    /// Plan mode — only read-only tools, model proposes a plan via text tags.
    Plan,
}

impl AgentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
        }
    }

    /// Returns true if this mode restricts tool access.
    pub fn restricts_tools(&self) -> bool {
        matches!(self, Self::Plan)
    }
}

impl Default for AgentMode {
    fn default() -> Self {
        Self::Default
    }
}

impl std::fmt::Display for AgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A proposed plan extracted from the model's text stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedPlan {
    /// Title/summary of the plan.
    pub title: String,
    /// Ordered list of steps to execute.
    pub steps: Vec<String>,
}

impl ProposedPlan {
    pub fn new(title: String, steps: Vec<String>) -> Self {
        Self { title, steps }
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.title.is_empty()
    }
}

/// Parses `<proposed_plan>` blocks from streaming text in real-time.
///
/// The model emits plans like:
/// ```text
/// <proposed_plan title="Fix the bug">
/// 1. Read the file
/// 2. Fix the function
/// 3. Run tests
/// </proposed_plan>
/// ```
///
/// The parser accumulates text chunks and extracts the plan when the
/// closing tag is found.
#[derive(Debug, Default)]
pub struct ProposedPlanParser {
    buffer: String,
    in_plan: bool,
    plan_title: Option<String>,
    plan_body: String,
    completed_plans: Vec<ProposedPlan>,
}

impl ProposedPlanParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a text delta into the parser.
    /// Returns any plans that were completed by this chunk.
    pub fn push_text(&mut self, text: &str) -> Vec<ProposedPlan> {
        self.buffer.push_str(text);
        self.drain_completed()
    }

    /// Drain any pending plans (call at end of turn).
    pub fn drain(&mut self) -> Vec<ProposedPlan> {
        // Move any remaining buffer content into plan_body if inside a plan.
        if self.in_plan {
            self.plan_body.push_str(&self.buffer);
            self.buffer.clear();
            if !self.plan_body.is_empty() {
                let plan = self.finalize_plan();
                self.completed_plans.push(plan);
            }
        }
        std::mem::take(&mut self.completed_plans)
    }

    /// Returns true if the parser is currently inside a `<proposed_plan>` block.
    pub fn is_in_plan(&self) -> bool {
        self.in_plan
    }

    /// Returns the partial plan body being accumulated (for live UI preview).
    pub fn partial_body(&self) -> &str {
        if self.in_plan {
            &self.plan_body
        } else {
            ""
        }
    }

    /// Returns the parsed title of the current plan (for live UI preview).
    pub fn partial_title(&self) -> Option<&str> {
        if self.in_plan {
            self.plan_title.as_deref()
        } else {
            None
        }
    }

    fn drain_completed(&mut self) -> Vec<ProposedPlan> {
        loop {
            if self.in_plan {
                // Look for closing tag in buffer
                if let Some(pos) = self.buffer.find("</proposed_plan>") {
                    self.plan_body.push_str(&self.buffer[..pos]);
                    self.buffer = self.buffer[pos + "</proposed_plan>".len()..].to_string();
                    self.in_plan = false;
                    let plan = self.finalize_plan();
                    self.completed_plans.push(plan);
                } else {
                    // No closing tag yet. Move all buffer content into plan_body
                    // except a suffix that could be the start of "</proposed_plan>".
                    let tag = "</proposed_plan>";
                    if self.buffer.len() >= tag.len() {
                        let safe = self.buffer.len() - tag.len() + 1;
                        self.plan_body.push_str(&self.buffer[..safe]);
                        self.buffer = self.buffer[safe..].to_string();
                    } else {
                        // Buffer is shorter than the tag — move everything.
                        self.plan_body.push_str(&self.buffer);
                        self.buffer.clear();
                    }
                    break;
                }
            } else {
                // Look for opening tag
                if let Some(tag_info) = self.find_opening_tag() {
                    self.buffer = self.buffer[tag_info.consume_len..].to_string();
                    self.in_plan = true;
                    self.plan_title = tag_info.title;
                    self.plan_body.clear();
                } else {
                    // No opening tag found. Keep only a suffix that could be
                    // the start of "<proposed_plan".
                    let tag = "<proposed_plan";
                    let safe = self.buffer.len().saturating_sub(tag.len() - 1);
                    self.buffer = self.buffer[safe..].to_string();
                    break;
                }
            }
        }
        std::mem::take(&mut self.completed_plans)
    }

    fn find_tag(&self, tag: &str) -> Option<usize> {
        self.buffer.find(tag).map(|pos| pos + tag.len())
    }

    fn finalize_plan(&mut self) -> ProposedPlan {
        let title = self.plan_title.take().unwrap_or_default();
        let body = std::mem::take(&mut self.plan_body);
        let steps = parse_plan_steps(&body);
        ProposedPlan::new(title, steps)
    }

    fn find_opening_tag(&self) -> Option<OpeningTagInfo> {
        let start = self.buffer.find("<proposed_plan")?;
        let rest = &self.buffer[start..];
        let tag_end = rest.find('>')?;
        let tag_content = &rest[..tag_end];
        let consume_len = start + tag_end + 1;

        let title = tag_content
            .find("title=\"")
            .and_then(|t_pos| {
                let value_start = t_pos + "title=\"".len();
                tag_content[value_start..]
                    .find('"')
                    .map(|end| tag_content[value_start..value_start + end].to_string())
            })
            .or_else(|| {
                tag_content
                    .find("title='")
                    .and_then(|t_pos| {
                        let value_start = t_pos + "title='".len();
                        tag_content[value_start..]
                            .find('\'')
                            .map(|end| tag_content[value_start..value_start + end].to_string())
                    })
            });

        Some(OpeningTagInfo { consume_len, title })
    }
}

struct OpeningTagInfo {
    consume_len: usize,
    title: Option<String>,
}

/// Parses plan body text into steps.
/// Supports numbered lists, bullet lists, and plain lines.
fn parse_plan_steps(body: &str) -> Vec<String> {
    body.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            // Strip leading markers: "1. ", "1) ", "- ", "* ", "• "
            let stripped = if let Some(rest) = line.strip_prefix("- ") {
                rest.to_string()
            } else if let Some(rest) = line.strip_prefix("* ") {
                rest.to_string()
            } else if let Some(rest) = line.strip_prefix("• ") {
                rest.to_string()
            } else {
                // Strip numbered prefix: "1. ", "1) ", "10. ", etc.
                let chars: Vec<char> = line.chars().collect();
                let mut idx = 0;
                while idx < chars.len() && chars[idx].is_ascii_digit() {
                    idx += 1;
                }
                if idx > 0 && idx < chars.len() && (chars[idx] == '.' || chars[idx] == ')') {
                    idx += 1;
                    while idx < chars.len() && chars[idx] == ' ' {
                        idx += 1;
                    }
                    line[idx..].to_string()
                } else {
                    line.to_string()
                }
            };
            if stripped.is_empty() {
                line.to_string()
            } else {
                stripped
            }
        })
        .collect()
}

/// Returns true if a tool kind is allowed in Plan mode.
/// Only read-only and search tools are permitted.
pub fn is_tool_allowed_in_plan_mode(kind: ToolKind) -> bool {
    matches!(
        kind,
        ToolKind::Read
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_extracts_simple_plan() {
        let mut parser = ProposedPlanParser::new();
        let plans = parser.push_text(
            "Let me analyze this.\n\
             <proposed_plan title=\"Fix the bug\">\n\
             1. Read the file\n\
             2. Fix the function\n\
             3. Run tests\n\
             </proposed_plan>\n\
             Done.",
        );
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].title, "Fix the bug");
        assert_eq!(plans[0].steps.len(), 3);
        assert_eq!(plans[0].steps[0], "Read the file");
        assert_eq!(plans[0].steps[1], "Fix the function");
        assert_eq!(plans[0].steps[2], "Run tests");
    }

    #[test]
    fn parser_extracts_plan_without_title() {
        let mut parser = ProposedPlanParser::new();
        let plans = parser.push_text(
            "<proposed_plan>\n\
             - Step one\n\
             - Step two\n\
             </proposed_plan>",
        );
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].title, "");
        assert_eq!(plans[0].steps, vec!["Step one", "Step two"]);
    }

    #[test]
    fn parser_handles_chunked_stream() {
        let mut parser = ProposedPlanParser::new();
        let chunks = [
            "Let me think.\n<propos",
            "ed_plan title=\"My Plan\">\n",
            "1. First step\n2. Second ",
            "step\n</proposed_",
            "plan>\nDone.",
        ];

        let mut all_plans = Vec::new();
        for chunk in &chunks {
            all_plans.extend(parser.push_text(chunk));
        }

        assert_eq!(all_plans.len(), 1);
        assert_eq!(all_plans[0].title, "My Plan");
        assert_eq!(all_plans[0].steps, vec!["First step", "Second step"]);
    }

    #[test]
    fn parser_handles_multiple_plans() {
        let mut parser = ProposedPlanParser::new();
        let plans = parser.push_text(
            "<proposed_plan title=\"Plan A\">\n1. A1\n</proposed_plan>\n\
             <proposed_plan title=\"Plan B\">\n1. B1\n</proposed_plan>",
        );
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].title, "Plan A");
        assert_eq!(plans[1].title, "Plan B");
    }

    #[test]
    fn parser_drain_unclosed_plan() {
        let mut parser = ProposedPlanParser::new();
        parser.push_text("<proposed_plan title=\"Unclosed\">\n1. Step\n");
        let plans = parser.drain();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].title, "Unclosed");
        assert_eq!(plans[0].steps, vec!["Step"]);
    }

    #[test]
    fn parser_partial_preview() {
        let mut parser = ProposedPlanParser::new();
        parser.push_text("<proposed_plan title=\"Live\">\n1. First");
        assert!(parser.is_in_plan());
        assert_eq!(parser.partial_title(), Some("Live"));
        assert!(parser.partial_body().contains("First"));
    }

    #[test]
    fn parser_no_plan_in_regular_text() {
        let mut parser = ProposedPlanParser::new();
        let plans = parser.push_text("Just regular text without any plan tags.");
        assert!(plans.is_empty());
        assert!(!parser.is_in_plan());
    }

    #[test]
    fn parse_plan_steps_strips_numbering() {
        let steps = parse_plan_steps("1. First\n2. Second\n3. Third");
        assert_eq!(steps, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn parse_plan_steps_strips_bullets() {
        let steps = parse_plan_steps("- Alpha\n* Beta");
        assert_eq!(steps, vec!["Alpha", "Beta"]);
    }

    #[test]
    fn agent_mode_default_is_default() {
        assert_eq!(AgentMode::default(), AgentMode::Default);
    }

    #[test]
    fn agent_mode_plan_restricts_tools() {
        assert!(AgentMode::Plan.restricts_tools());
        assert!(!AgentMode::Default.restricts_tools());
    }

    #[test]
    fn only_read_tools_allowed_in_plan_mode() {
        assert!(is_tool_allowed_in_plan_mode(ToolKind::Read));
        assert!(!is_tool_allowed_in_plan_mode(ToolKind::Write));
        assert!(!is_tool_allowed_in_plan_mode(ToolKind::Command));
        assert!(!is_tool_allowed_in_plan_mode(ToolKind::Custom));
    }
}
