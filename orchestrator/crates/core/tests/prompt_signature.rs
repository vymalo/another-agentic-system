//! The person's prompt signature through the core (ADR 0087, decision 2, and the owner's ruling of
//! 2026-11-27): a message carries it to its delegation or steer and to **no event**. The log is the
//! state, but the envelope is not state: it is a proof that travels with the request and is checked
//! at intake, so nothing of it may be replayed, exported or projected.
//!
//! A Stop & send that holds its text for the next job does **not** hold the envelope: a signature is
//! not ledger state. A redelivery carries what its outbox row holds, which is the same seam.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use ThreadState::Working;

const ENVELOPE: &str = "eyJhbGciOiJFZERTQSJ9..c2lnbmF0dXJl";

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn said(text: &str, prompt_signature: Option<&str>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        prompt_signature: prompt_signature.map(str::to_owned),
    }
}

fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}

/// The envelope of the one delegation or steer in `cmds`, if there is one.
fn sent(cmds: &[Command]) -> Option<String> {
    cmds.iter().find_map(|c| match c {
        Command::Delegate {
            prompt_signature, ..
        }
        | Command::Steer {
            prompt_signature, ..
        } => Some(prompt_signature.clone()),
        _ => None,
    })?
}

/// What the transition wrote of the message, serialised: the `user_message` an event, a frame and an
/// export are made of (the one event that could hold the envelope).
fn written(cmds: &[Command]) -> String {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(draft) => match &draft.body {
                EventBody::UserMessage(message) => Some(serde_json::to_string(message).unwrap()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<String>>()
        .join("")
}

#[test]
fn a_first_message_carries_its_envelope_to_the_delegation_and_to_no_event() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("hi", Some(ENVELOPE))).unwrap();
    assert_eq!(sent(&cmds), Some(ENVELOPE.to_owned()));
    let log = written(&cmds);
    assert!(!log.contains(ENVELOPE), "the envelope is in the log: {log}");
    assert!(
        !log.contains("promptSignature") && !log.contains("prompt_signature"),
        "the log names the envelope: {log}"
    );
}

#[test]
fn a_message_to_a_running_job_is_steered_with_its_envelope() {
    let snapshot = Snapshot::queued(GatePolicy::default());
    let (_, cmds) = step(&snapshot, &said("hi", Some(ENVELOPE)));
    assert_eq!(sent(&cmds), Some(ENVELOPE.to_owned()));
    assert!(!written(&cmds).contains(ENVELOPE));
}

#[test]
fn a_message_that_carries_none_writes_what_it_always_did() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("hi", None)).unwrap();
    assert_eq!(sent(&cmds), None);
}

#[test]
fn a_stop_and_send_holds_no_envelope_for_the_next_job() {
    // The text is held in the ledger, and the ledger is exported: a signature may not go with it, so
    // the next job's delegation (and a stop the agent refuses that becomes a steer) carries none.
    let mut snapshot = Snapshot::queued(GatePolicy::default());
    snapshot.state = Working;
    let stop = Input::StopAndSend {
        user: user(),
        text: "hi".into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        prompt_signature: Some(ENVELOPE.to_owned()),
    };
    let (after, cmds) = step(&snapshot, &stop);
    assert!(
        cmds.iter()
            .all(|c| !matches!(c, Command::Delegate { .. } | Command::Steer { .. }))
    );
    assert!(!written(&cmds).contains(ENVELOPE));
    // the held text starts the next job when the task ends, and it carries no envelope
    let (_, cmds) = step(&after, &Input::CancelledBeforeStart);
    assert_eq!(sent(&cmds), None);
    assert!(!written(&cmds).contains(ENVELOPE));
}

#[test]
fn a_redelivery_carries_what_its_row_holds() {
    let snapshot = Snapshot::queued(GatePolicy::default());
    let (_, cmds) = step(
        &snapshot,
        &Input::Redeliver {
            text: "hi".into(),
            sent: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick: None,
            prompt_signature: Some(ENVELOPE.to_owned()),
        },
    );
    assert_eq!(sent(&cmds), Some(ENVELOPE.to_owned()));
    assert!(!written(&cmds).contains(ENVELOPE));
}
