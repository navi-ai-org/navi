//! Consumer-Driven Contract test for the ATIF v1.7 trajectory schema.
//!
//! Validates that every trajectory produced by `build_trajectory` conforms to
//! a JSON Schema encoding the ATIF v1.7 contract. This is the Rust-idiomatic
//! equivalent of a Pact contract: the schema is the consumer's expectation of
//! the producer's output shape, and this test enforces it on every export.
//!
//! Uses the `jsonschema` crate already in `navi-core`'s dependency tree.

use navi_core::{
    AgentEvent, AtifExportOptions, ContentPart, SessionId, SessionSnapshot, atif_to_json,
    build_trajectory,
};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The ATIF v1.7 JSON Schema, expressed as a `serde_json::Value`.
///
/// This encodes the consumer contract: the fields a downstream SFT/RL pipeline
/// relies on, their types, and required-ness. Optional fields use
/// `["string","null"]` / `["object","null"]` to match the `skip_serializing_if`
/// behavior in the Rust structs.
const ATIF_SCHEMA: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "ATIF v1.7 Trajectory",
  "type": "object",
  "required": ["schema_version", "agent", "steps"],
  "additionalProperties": true,
  "properties": {
    "schema_version": { "type": "string", "const": "ATIF-v1.7" },
    "session_id": { "type": ["string", "null"] },
    "agent": {
      "type": "object",
      "required": ["name", "version"],
      "additionalProperties": true,
      "properties": {
        "name": { "type": "string" },
        "version": { "type": "string" },
        "model_name": { "type": ["string", "null"] },
        "extra": { "type": ["object", "null"] }
      }
    },
    "steps": {
      "type": "array",
      "items": { "$ref": "#/$defs/step" }
    },
    "notes": { "type": ["string", "null"] },
    "final_metrics": { "$ref": "#/$defs/final_metrics" },
    "extra": { "type": ["object", "null"] }
  },
  "$defs": {
    "step": {
      "type": "object",
      "required": ["step_id", "source", "message"],
      "additionalProperties": true,
      "properties": {
        "step_id": { "type": "integer", "minimum": 1 },
        "timestamp": { "type": ["string", "null"] },
        "source": { "enum": ["system", "user", "agent"] },
        "message": { "$ref": "#/$defs/message" },
        "model_name": { "type": ["string", "null"] },
        "reasoning_content": { "type": ["string", "null"] },
        "tool_calls": {
          "type": "array",
          "items": { "$ref": "#/$defs/tool_call" }
        },
        "observation": { "$ref": "#/$defs/observation" },
        "metrics": { "$ref": "#/$defs/metrics" },
        "extra": { "type": ["object", "null"] }
      }
    },
    "message": {
      "oneOf": [
        { "type": "string" },
        {
          "type": "array",
          "items": { "$ref": "#/$defs/content_part" }
        }
      ]
    },
    "content_part": {
      "type": "object",
      "required": ["type"],
      "properties": {
        "type": { "enum": ["text", "image"] }
      },
      "allOf": [
        {
          "if": { "properties": { "type": { "const": "text" } } },
          "then": { "required": ["text"], "properties": { "text": { "type": "string" } } }
        },
        {
          "if": { "properties": { "type": { "const": "image" } } },
          "then": {
            "required": ["media_type", "data"],
            "properties": {
              "media_type": { "type": "string" },
              "data": { "type": "string" }
            }
          }
        }
      ]
    },
    "tool_call": {
      "type": "object",
      "required": ["tool_call_id", "function_name", "arguments"],
      "additionalProperties": true,
      "properties": {
        "tool_call_id": { "type": "string" },
        "function_name": { "type": "string" },
        "arguments": { "type": "object" },
        "extra": { "type": ["object", "null"] }
      }
    },
    "observation": {
      "type": ["object", "null"],
      "required": ["results"],
      "properties": {
        "results": {
          "type": "array",
          "items": { "$ref": "#/$defs/observation_result" }
        }
      }
    },
    "observation_result": {
      "type": "object",
      "additionalProperties": true,
      "properties": {
        "source_call_id": { "type": ["string", "null"] },
        "content": { "$ref": "#/$defs/message" },
        "extra": { "type": ["object", "null"] }
      }
    },
    "metrics": {
      "type": ["object", "null"],
      "additionalProperties": true,
      "properties": {
        "prompt_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "completion_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "cached_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "extra": { "type": ["object", "null"] }
      }
    },
    "final_metrics": {
      "type": ["object", "null"],
      "additionalProperties": true,
      "properties": {
        "total_prompt_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "total_completion_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "total_cached_tokens": { "type": ["integer", "null"], "minimum": 0 },
        "total_cost_usd": { "type": ["number", "null"], "minimum": 0 },
        "total_steps": { "type": ["integer", "null"], "minimum": 0 },
        "extra": { "type": ["object", "null"] }
      }
    }
  }
}"##;

fn schema() -> jsonschema::Validator {
    let schema_json: Value = serde_json::from_str(ATIF_SCHEMA).expect("schema is valid JSON");
    jsonschema::Validator::new(&schema_json).expect("schema compiles")
}

fn snapshot(events: Vec<AgentEvent>) -> SessionSnapshot {
    SessionSnapshot {
        version: 1,
        id: SessionId::new("contract-session".to_string()),
        title: Some("contract".to_string()),
        project: PathBuf::from("/tmp"),
        created_at: 1_700_000_000,
        updated_at: 1_700_000_010,
        events,
        memory: None,
        goal: None,
        usage: None,
    }
}

fn opts() -> AtifExportOptions<'static> {
    AtifExportOptions {
        agent_version: "0.0.0-contract",
        model_name: "contract-model",
        redact_secrets: false,
    }
}

#[test]
fn empty_trajectory_conforms_to_schema() {
    let validator = schema();
    let t = build_trajectory(&snapshot(vec![]), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let result = validator.validate(&value);
    assert!(
        result.is_ok(),
        "empty trajectory failed schema validation: {:?}",
        result.err()
    );
}

#[test]
fn user_then_model_trajectory_conforms_to_schema() {
    let validator = schema();
    let events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "hello".to_string(),
            content_parts: vec![],
            submitted_at: Some(1_700_000_000),
        },
        AgentEvent::ModelOutput {
            text: "hi there".to_string(),
            thinking: Some("planning".to_string()),
        },
    ];
    let t = build_trajectory(&snapshot(events), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let result = validator.validate(&value);
    assert!(
        result.is_ok(),
        "user+model trajectory failed schema: {:?}",
        result.err()
    );
}

#[test]
fn tool_call_and_result_trajectory_conforms_to_schema() {
    let validator = schema();
    let events = vec![
        AgentEvent::UserTaskSubmitted {
            text: "read the file".to_string(),
            content_parts: vec![],
            submitted_at: None,
        },
        AgentEvent::ModelOutput {
            text: "on it".to_string(),
            thinking: None,
        },
        AgentEvent::ToolRequested(navi_core::ToolInvocation {
            id: "call-1".to_string(),
            tool_name: "read_file".to_string(),
            input: json!({ "path": "/tmp/x" }),
        }),
        AgentEvent::ToolCompleted(navi_core::ToolResult {
            invocation_id: "call-1".to_string(),
            ok: true,
            output: json!("file contents"),
        }),
        AgentEvent::UsageReported {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 0,
            cache_read_tokens: 10,
        },
    ];
    let t = build_trajectory(&snapshot(events), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let result = validator.validate(&value);
    assert!(
        result.is_ok(),
        "tool call trajectory failed schema: {:?}",
        result.err()
    );
}

#[test]
fn multimodal_trajectory_conforms_to_schema() {
    let validator = schema();
    let events = vec![AgentEvent::UserTaskSubmitted {
        text: "look at this".to_string(),
        content_parts: vec![
            ContentPart::Text {
                text: "see image".to_string(),
            },
            ContentPart::Image {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".to_string(),
            },
        ],
        submitted_at: None,
    }];
    let t = build_trajectory(&snapshot(events), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let result = validator.validate(&value);
    assert!(
        result.is_ok(),
        "multimodal trajectory failed schema: {:?}",
        result.err()
    );
}

#[test]
fn system_events_trajectory_conforms_to_schema() {
    let validator = schema();
    let events = vec![
        AgentEvent::AutoCompactCompleted {
            tokens_saved: 500,
            summary: "compacted".to_string(),
            kept_recent_messages: 2,
        },
        AgentEvent::Error {
            message: "something broke".to_string(),
        },
        AgentEvent::HarnessStopped {
            reason: "stop".to_string(),
            message: "done".to_string(),
            tool_name: None,
        },
    ];
    let t = build_trajectory(&snapshot(events), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let result = validator.validate(&value);
    assert!(
        result.is_ok(),
        "system events trajectory failed schema: {:?}",
        result.err()
    );
}

#[test]
fn schema_version_is_pinned_to_atif_v1_7() {
    let validator = schema();
    let t = build_trajectory(&snapshot(vec![]), &opts());
    let json = atif_to_json(&t).unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema_version"], "ATIF-v1.7");
    // A wrong version must FAIL validation (negative test).
    let mut bad = value.clone();
    bad["schema_version"] = json!("ATIF-v1.6");
    assert!(
        validator.validate(&bad).is_err(),
        "schema must reject wrong version"
    );
}

#[test]
fn non_object_arguments_fail_schema() {
    let validator = schema();
    // Manually craft a trajectory with non-object arguments to confirm the
    // schema catches it (the fold coerces to object, so this is a negative test
    // against the schema itself, not the fold).
    let bad = json!({
        "schema_version": "ATIF-v1.7",
        "agent": { "name": "navi", "version": "0.0.0" },
        "steps": [{
            "step_id": 1,
            "source": "agent",
            "message": "hi",
            "tool_calls": [{
                "tool_call_id": "x",
                "function_name": "f",
                "arguments": "not-an-object"
            }]
        }]
    });
    assert!(
        validator.validate(&bad).is_err(),
        "schema must reject non-object arguments"
    );
}
