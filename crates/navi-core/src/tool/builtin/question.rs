use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;

use super::helpers;
use crate::tool::{Tool, ToolDefinition, ToolInvocation, ToolKind, ToolResult};

pub(crate) struct QuestionTool;

#[async_trait]
impl Tool for QuestionTool {
    fn definition(&self) -> ToolDefinition {
        helpers::definition(
            "question",
            "Ask the user to choose from a short list of options. Use this only when user input is needed to proceed.",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question shown to the user."
                    },
                    "options": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "oneOf": [
                                { "type": "string" },
                                {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    },
                                    "required": ["label"],
                                    "additionalProperties": false
                                }
                            ]
                        },
                        "description": "Options the user can choose from. Prefer concise labels with optional descriptions."
                    },
                    "multiple": {
                        "type": "boolean",
                        "description": "When true, the user may select more than one option. Defaults to false."
                    },
                    "custom": {
                        "type": "boolean",
                        "description": "When true, allow the user to type a custom answer. Defaults to false."
                    }
                },
                "required": ["question", "options"],
                "additionalProperties": false,
            }),
        )
    }

    async fn invoke(&self, invocation: ToolInvocation) -> Result<ToolResult> {
        Ok(ToolResult {
            invocation_id: invocation.id,
            ok: false,
            output: helpers::tool_error(
                "interactive_question_unavailable",
                "question requires an interactive client",
                true,
                Some(
                    "Run this turn from the TUI or another client that supports question resolution.",
                ),
                None,
            ),
        })
    }
}
