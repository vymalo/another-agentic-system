//! Rows an older build wrote (ADR 0087, a rolling upgrade): the member that held a bare envelope,
//! `prompt_signature`, is **ignored**. It does not become the signed prompt of today and the row
//! still reads, so a message queued across an upgrade goes out unsigned.
#![allow(clippy::unwrap_used, missing_docs)]

use orch_ports::OutboxPayload;
use serde_json::{Value, json};

fn read(row: Value) -> OutboxPayload {
    serde_json::from_value(row).unwrap()
}

#[test]
fn a_row_with_the_old_member_reads_as_unsigned() {
    const OLD: &str = "eyJhbGciOiJFZERTQSJ9..c2ln";
    let delegate = read(json!({"delegate": {
        "text": "hi", "release": null, "prompt_signature": OLD
    }}));
    let steer = read(json!({"steer": {
        "text": "hi", "release": null, "prompt_signature": OLD
    }}));
    let start = read(json!({"session_start": {
        "request": "r", "agent": "planner", "description": "d", "text": "hi",
        "keep_open": false, "prompt_signature": OLD
    }}));
    for row in [delegate, steer, start] {
        let (OutboxPayload::Delegate { signed_prompt, .. }
        | OutboxPayload::Steer { signed_prompt, .. }
        | OutboxPayload::SessionStart { signed_prompt, .. }) = &row
        else {
            panic!("{row:?}")
        };
        assert_eq!(*signed_prompt, None, "{row:?}");
        // and it is not written back
        let written = serde_json::to_string(&row).unwrap();
        assert!(!written.contains(OLD), "{written}");
    }
}
