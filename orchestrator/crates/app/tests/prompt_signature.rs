//! The person's prompt signature through the application and the dispatcher (ADR 0087, decision 2
//! and the owner's ruling of 2026-11-27): the envelope rides the delegation's **outbox row** and
//! then the agent's request, **byte for byte**, and it is in no event, no frame and no export.
//!
//! The presence check at intake is the surface's (`orch-surface-agui`'s `prompt_signature.rs`); what
//! this file pins is the seam between them, which is where an envelope is easiest to lose — or to
//! write into the log, which is the one thing it must never do.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{Creation, Inbound, NewThread};
use orch_core::{Origin, ThreadId, ThreadState};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, ThreadStore};
use support::*;

/// A compact JWS of three base64url parts with a detached payload: what a signing app sends.
const ENVELOPE: &str =
    "eyJhbGciOiJFZERTQSIsImtpZCI6ImRldmljZS0xI3YxIn0..c2lnbmF0dXJlLW9mLXRoZS1wcm9tcHQ";

fn new_thread(text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: text.to_owned(),
    }
}

/// A thread created with a first message that carries `prompt_signature`.
async fn start(w: &World, text: &str, prompt_signature: Option<&str>) -> ThreadId {
    let app = w.app();
    let thread = ThreadId(uuid::Uuid::now_v7());
    match app
        .create_thread_as(
            &alice(),
            thread,
            new_thread(text),
            Inbound {
                origin: Origin::Agui,
                prompt_signature: prompt_signature.map(str::to_owned),
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    {
        Creation::Created { .. } => thread,
        Creation::Exists => panic!("a fresh id"),
    }
}

/// Everything an export, a frame and a log line are made of: the thread row (with its ledger) and
/// every event, serialised.
async fn exported(w: &World, thread: ThreadId) -> String {
    let export = w.app().export_thread(&alice(), thread).await.unwrap();
    let mut text = serde_json::to_string(&export.thread).unwrap();
    for event in &export.events {
        text.push_str(&serde_json::to_string(event).unwrap());
    }
    text
}

#[tokio::test]
async fn the_envelope_rides_the_row_and_the_send_and_is_in_no_event() {
    let w = World::new();
    let thread = start(&w, "echo hi", Some(ENVELOPE)).await;

    // The commit's row carries it, byte for byte: that row is the only durable place it has.
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate {
        prompt_signature, ..
    } = &row.payload
    else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(prompt_signature.as_deref(), Some(ENVELOPE));

    // The log holds none of it: no event, and so no frame and no export.
    let export = exported(&w, thread).await;
    assert!(
        !export.contains(ENVELOPE),
        "the envelope is in the log: {export}"
    );
    assert!(
        !export.contains("promptSignature") && !export.contains("prompt_signature"),
        "the log names the envelope: {export}"
    );

    // And the dispatcher puts it on the agent's request, unchanged.
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<String>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [Some(ENVELOPE.to_owned())], "the send carries it");

    // Even after the row was worked: the log still holds nothing of it.
    let export = exported(&w, thread).await;
    assert!(!export.contains(ENVELOPE), "{export}");
}

#[tokio::test]
async fn a_message_that_carries_no_envelope_has_no_member_on_the_row() {
    let w = World::new();
    let thread = start(&w, "echo hi", None).await;
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let row = serde_json::to_value(&rows[0].payload).unwrap();
    assert!(
        row["delegate"].get("prompt_signature").is_none(),
        "no member on the row: {row}"
    );
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<String>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [None]);
}
