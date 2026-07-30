//! Fuzz target: feed arbitrary bytes as a JSON array of AgentEvents to
//! `build_trajectory`. Any input that deserializes must not panic.
//!
//! Run with:
//!   cargo +nightly fuzz run fold_arbitrary_events

#![no_main]

use libfuzzer_sys::fuzz_target;
use navi_core::{AtifExportOptions, SessionId, SessionSnapshot, build_trajectory};
use std::path::PathBuf;

fuzz_target!(|data: &[u8]| {
    // Try to deserialize the bytes as a Vec<AgentEvent>.
    let Ok(events): Result<Vec<navi_core::AgentEvent>, _> = serde_json::from_slice(data) else {
        return;
    };

    let snap = SessionSnapshot {
        version: 1,
        id: SessionId::new("fuzz".to_string()),
        title: None,
        project: PathBuf::from("/tmp"),
        created_at: 0,
        updated_at: 0,
        events,
        memory: None,
        goal: None,
        usage: None,
    };

    let opts = AtifExportOptions {
        agent_version: "fuzz",
        model_name: "fuzz-model",
        redact_secrets: false,
    };

    let t = build_trajectory(&snap, &opts);
    let _ = navi_core::atif_to_json(&t);

    // Also exercise redaction path.
    let mut redacted_opts = opts;
    redacted_opts.redact_secrets = true;
    let _ = build_trajectory(&snap, &redacted_opts);
});
