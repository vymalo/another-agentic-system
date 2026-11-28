//! The person's prompt signature through the core (ADR 0087, decision 2, and the owner's ruling of
//! 2026-11-27): a message carries it to its delegation or steer and to **no event**. The log is the
//! state, but the envelope is not state: it is a proof that travels with the request and is checked
//! at intake, so nothing of it may be replayed, exported or projected.
//!
//! A Stop & send that holds its text for the next job holds the envelope beside it
//! (`Job::after_stop_prompt_signature`, ANR-1376), and hands it on with that very text. A signature
//! covers one message, so nothing signed is ever joined: while a message is held, a second one is
//! refused when either is signed. A redelivery carries what its outbox row holds.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use ThreadState::{Queued, Working};

const ENVELOPE: &str = "eyJhbGciOiJFZERTQSJ9..c2lnbmF0dXJl";
const OTHER: &str = "eyJhbGciOiJFZERTQSJ9..b3RoZXI";

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
        prompt_signature: prompt_signature.map(signature),
    }
}

fn signature(envelope: &str) -> SignedPrompt {
    SignedPrompt::envelope_only(envelope.to_owned())
}

/// A run's claims, each member a word no other part of a command could be mistaken for.
fn claims() -> PromptClaims {
    PromptClaims {
        aud: "https://aud.test.invalid".into(),
        thread: "thread-as-sent".into(),
        agent: "agent-as-sent".into(),
        msg: "message-id-as-sent".into(),
        content_sha256: "content-hash".into(),
        props_sha256: "props-hash".into(),
    }
}

fn signed_with_claims() -> SignedPrompt {
    SignedPrompt {
        claims: Some(claims()),
        ..signature(ENVELOPE)
    }
}

fn stop(text: &str, prompt_signature: Option<&str>) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        prompt_signature: prompt_signature.map(signature),
    }
}

/// A job that runs, stopped by one message.
fn stopping(text: &str, prompt_signature: Option<&str>) -> (Snapshot, Vec<Command>) {
    let mut snapshot = Snapshot::queued(GatePolicy::default());
    snapshot.state = Working;
    step(&snapshot, &stop(text, prompt_signature))
}

fn held(snap: &Snapshot) -> Option<&str> {
    snap.job
        .after_stop_prompt_signature
        .as_ref()
        .map(|signed| signed.envelope.as_str())
}

fn task_canceled() -> Input {
    Input::Agent {
        agent: AgentId::new("coder"),
        revision: None,
        update: AgentUpdate::Status {
            state: AgentTaskState::Canceled,
            detail: None,
            tier: None,
        },
    }
}

fn cancel_rejected(retryable: bool) -> Input {
    Input::CancelRejected {
        agent: AgentId::new("coder"),
        reason: "no".into(),
        retryable,
    }
}

/// The three ways a stop ends that start the next job, with a delegation.
fn next_job_endings() -> [(&'static str, Input); 3] {
    [
        ("the task ends", task_canceled()),
        ("cancelled before start", Input::CancelledBeforeStart),
        (
            "delivery failed",
            Input::DeliveryFailed {
                reason: "gave up".into(),
                retryable: false,
            },
        ),
    ]
}

/// The text and the envelope of the one delegation or steer in `cmds`, and which it is.
fn order(cmds: &[Command]) -> (&'static str, String, Option<String>) {
    let mut orders = cmds.iter().filter_map(|c| {
        let (kind, text, envelope) = match c {
            Command::Delegate {
                text,
                prompt_signature,
                ..
            } => ("delegate", text, prompt_signature),
            Command::Steer {
                text,
                prompt_signature,
                ..
            } => ("steer", text, prompt_signature),
            _ => return None,
        };
        let envelope = envelope.as_ref().map(|e| e.envelope.as_str().to_owned());
        Some((kind, text.clone(), envelope))
    });
    let one = orders.next().expect("a delegation or a steer");
    assert!(orders.next().is_none(), "{cmds:?}");
    one
}

fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}

/// The envelope of the first delegation or steer in `cmds`, if there is one.
fn sent(cmds: &[Command]) -> Option<String> {
    cmds.iter().find_map(|c| match c {
        Command::Delegate {
            prompt_signature, ..
        }
        | Command::Steer {
            prompt_signature, ..
        } => Some(
            prompt_signature
                .as_ref()
                .map(|e| e.envelope.as_str().to_owned()),
        ),
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
fn a_stop_and_send_holds_its_envelope_and_sends_nothing_yet() {
    let (after, cmds) = stopping("do X", Some(ENVELOPE));
    assert_eq!(held(&after), Some(ENVELOPE));
    assert!(
        cmds.iter()
            .all(|c| !matches!(c, Command::Delegate { .. } | Command::Steer { .. }))
    );
    assert!(!written(&cmds).contains(ENVELOPE));
}

#[test]
fn the_held_envelope_goes_with_its_text_to_the_next_job_however_the_stop_ends() {
    for (why, ending) in next_job_endings() {
        let (stopped, _) = stopping("do X", Some(ENVELOPE));
        let (after, cmds) = step(&stopped, &ending);
        assert_eq!(
            order(&cmds),
            ("delegate", "do X".to_owned(), Some(ENVELOPE.to_owned())),
            "{why}"
        );
        assert_eq!(after.job.number, 2, "{why}");
        assert_eq!(held(&after), None, "{why}: it is not kept");
        assert!(!written(&cmds).contains(ENVELOPE), "{why}");
    }
}

#[test]
fn a_stop_the_agent_refuses_for_good_steers_the_held_message_with_its_envelope() {
    let (stopped, _) = stopping("do X", Some(ENVELOPE));
    // a refusal that may pass changes nothing: the stop still waits, with its envelope
    let (waiting, _) = step(&stopped, &cancel_rejected(true));
    assert_eq!(held(&waiting), Some(ENVELOPE));
    let (after, cmds) = step(&waiting, &cancel_rejected(false));
    assert_eq!(
        order(&cmds),
        ("steer", "do X".to_owned(), Some(ENVELOPE.to_owned()))
    );
    assert_eq!(after.job.after_stop, None);
    assert_eq!(held(&after), None);
    assert!(!written(&cmds).contains(ENVELOPE));
}

#[test]
fn pressing_stop_drops_the_held_envelope_with_the_text() {
    let (stopped, _) = stopping("do X", Some(ENVELOPE));
    let (after, cmds) = step(&stopped, &Input::Cancel { user: user() });
    assert_eq!(after.job.after_stop, None);
    assert_eq!(held(&after), None);
    assert!(!format!("{cmds:?}").contains("Delegate"));
    // and an unsigned stop after it holds, and sends, no envelope
    let (again, _) = step(&after, &stop("do Y", None));
    assert_eq!(held(&again), None);
    let (_, cmds) = step(&again, &task_canceled());
    assert_eq!(order(&cmds), ("delegate", "do Y".to_owned(), None));
}

#[test]
fn nothing_signed_is_ever_joined_to_a_held_message() {
    for (why, first, second) in [
        ("signed then signed", Some(ENVELOPE), Some(OTHER)),
        ("signed then unsigned", Some(ENVELOPE), None),
        ("unsigned then signed", None, Some(OTHER)),
    ] {
        // a Stop & send, and a plain Send while the stop lands: both would join
        for again in [stop("and Y", second), said("and Y", second)] {
            let (stopped, _) = stopping("do X", first);
            let before = stopped.clone();
            // an error is no snapshot and no command
            assert_eq!(
                transition(&stopped, &again),
                Err(TransitionError::StopPending),
                "{why}"
            );
            assert_eq!(stopped, before, "{why}");
            // what was held goes on as it was
            let (_, cmds) = step(&stopped, &task_canceled());
            assert_eq!(
                order(&cmds),
                ("delegate", "do X".to_owned(), first.map(str::to_owned)),
                "{why}"
            );
        }
    }
}

#[test]
fn unsigned_messages_are_still_joined() {
    let (stopped, _) = stopping("do X", None);
    for again in [stop("and Y", None), said("and Y", None)] {
        let (joined, _) = step(&stopped, &again);
        assert_eq!(joined.job.after_stop.as_deref(), Some("do X\n\nand Y"));
        assert_eq!(held(&joined), None);
        let (_, cmds) = step(&joined, &task_canceled());
        assert_eq!(order(&cmds), ("delegate", "do X\n\nand Y".to_owned(), None));
    }
}

#[test]
fn no_later_job_carries_an_envelope_that_was_spent() {
    let (stopped, _) = stopping("do X", Some(ENVELOPE));
    let (next, _) = step(&stopped, &task_canceled());
    assert_eq!(next.state, Queued);
    // the next job is stopped by an unsigned message
    let (again, _) = step(&next, &stop("do Y", None));
    assert_eq!(held(&again), None);
    let (_, cmds) = step(&again, &task_canceled());
    assert_eq!(order(&cmds), ("delegate", "do Y".to_owned(), None));
    // and a job made from one that still held it starts without
    assert_eq!(stopped.job.next().after_stop_prompt_signature, None);
}

#[test]
fn the_ledger_keeps_the_envelope_under_its_own_name_and_an_old_one_has_none() {
    let (stopped, _) = stopping("do X", Some(ENVELOPE));
    let stored = serde_json::to_value(&stopped.job).unwrap();
    assert_eq!(
        stored["afterStopPromptSignature"],
        serde_json::json!({"envelope": ENVELOPE}),
        "{stored}"
    );
    let back: Job = serde_json::from_value(stored).unwrap();
    assert_eq!(back, stopped.job);
    let (unsigned, _) = stopping("do X", None);
    let stored = serde_json::to_value(&unsigned.job).unwrap();
    assert!(stored.get("afterStopPromptSignature").is_none(), "{stored}");
    let old: Job = serde_json::from_value(serde_json::json!({"afterStop": "do X"})).unwrap();
    assert_eq!(old.after_stop_prompt_signature, None);
}

#[test]
fn debug_shows_the_length_of_an_envelope_and_never_its_value() {
    let input = stop("do X", Some(ENVELOPE));
    let (stopped, _) = stopping("do X", Some(ENVELOPE));
    let (_, delegated) = step(&stopped, &task_canceled());
    let (_, steered) = step(&stopped, &cancel_rejected(false));
    let length = format!("<{} bytes>", ENVELOPE.len());
    for (what, shown) in [
        ("an input", format!("{input:?}")),
        ("a job", format!("{:?}", stopped.job)),
        ("a delegation", format!("{delegated:?}")),
        ("a steer", format!("{steered:?}")),
    ] {
        assert!(!shown.contains(ENVELOPE), "{what}: {shown}");
        assert!(shown.contains(&length), "{what}: {shown}");
    }
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
            prompt_signature: Some(signature(ENVELOPE)),
        },
    );
    assert_eq!(sent(&cmds), Some(ENVELOPE.to_owned()));
    assert!(!written(&cmds).contains(ENVELOPE));
}

#[test]
fn the_whole_value_reaches_the_delegation_the_ledger_and_the_steer_unchanged() {
    // ADR 0087: one value on the whole path, so no member can be dropped on the way.
    let value = signed_with_claims();
    let with = |mut input: Input| {
        if let Input::UserMessage {
            prompt_signature, ..
        }
        | Input::StopAndSend {
            prompt_signature, ..
        } = &mut input
        {
            *prompt_signature = Some(signed_with_claims());
        }
        input
    };
    let carried = |cmds: &[Command]| {
        cmds.iter()
            .find_map(|c| match c {
                Command::Delegate {
                    prompt_signature, ..
                }
                | Command::Steer {
                    prompt_signature, ..
                } => Some(prompt_signature.clone()),
                _ => None,
            })
            .expect("a delegation or a steer")
    };

    let (_, cmds) = start_thread(GatePolicy::default(), &with(said("hi", None))).unwrap();
    assert_eq!(carried(&cmds), Some(value.clone()));

    let mut working = Snapshot::queued(GatePolicy::default());
    working.state = Working;
    let (stopped, _) = step(&working, &with(stop("do X", None)));
    assert_eq!(stopped.job.after_stop_prompt_signature, Some(value.clone()));
    // and survives the ledger's own serialisation
    let stored = serde_json::to_value(&stopped.job).unwrap();
    let back: Job = serde_json::from_value(stored).unwrap();
    assert_eq!(back.after_stop_prompt_signature, Some(value.clone()));
    for ending in [task_canceled(), cancel_rejected(false)] {
        let (_, cmds) = step(&stopped, &ending);
        assert_eq!(carried(&cmds), Some(value.clone()));
        assert!(!written(&cmds).contains("content-hash"));
    }
}

#[test]
fn debug_shows_the_length_of_every_claim_and_never_its_value() {
    let shown = format!("{:?}", signed_with_claims());
    let c = claims();
    for (member, value) in [
        ("envelope", ENVELOPE.to_owned()),
        ("aud", c.aud),
        ("thread", c.thread),
        ("agent", c.agent),
        ("msg", c.msg),
        ("content_sha256", c.content_sha256),
        ("props_sha256", c.props_sha256),
    ] {
        assert!(!shown.contains(&value), "{member}: {shown}");
        assert!(
            shown.contains(&format!("{member}: <{} bytes>", value.len())),
            "{member}: {shown}"
        );
    }
}
