//! Fuzz target: feed arbitrary bytes as a JSON Trajectory, deserialize it,
//! re-serialize, and confirm idempotency. Any input that deserializes must
//! round-trip byte-identically.
//!
//! Run with:
//!   cargo +nightly fuzz run roundtrip_json

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let t: navi_core::Trajectory = match serde_json::from_str(s) {
        Ok(t) => t,
        Err(_) => return,
    };

    let j1 = navi_core::atif_to_json(&t).unwrap();
    let back: navi_core::Trajectory = serde_json::from_str(&j1).unwrap();
    let j2 = navi_core::atif_to_json(&back).unwrap();

    assert_eq!(j1, j2, "JSON roundtrip is not idempotent");
});
