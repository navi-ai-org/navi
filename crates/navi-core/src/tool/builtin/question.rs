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
            "Ask the user a question when input is needed to proceed.\n\
             - Multiple choice: pass `options` (and optional multiple/custom).\n\
             - Free-form: omit `options` and set `freeform` true (or pass title/description for long prompts).\n\
             Prefer this over request_user_input (hidden alias).",
            ToolKind::Read,
            json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question shown to the user. Also accepts free-form prompts when options are omitted."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional short title (free-form / request_user_input compatibility)."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional longer description (free-form compatibility). Used when question is empty."
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
                        "description": "Options for multiple choice. Omit for free-form text input."
                    },
                    "multiple": {
                        "type": "boolean",
                        "description": "When true, the user may select more than one option. Defaults to false."
                    },
                    "custom": {
                        "type": "boolean",
                        "description": "When true, allow the user to type a custom answer alongside options. Defaults to false."
                    },
                    "freeform": {
                        "type": "boolean",
                        "description": "When true (or when options are omitted), request free-form text input instead of a fixed choice list."
                    },
                    "required": {
                        "type": "boolean",
                        "description": "Whether the user must answer before continuing (free-form). Defaults to true."
                    }
                },
                "required": [],
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
