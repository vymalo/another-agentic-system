//! The person's prompt signature through the application and the dispatcher (ADR 0087, decision 2
//! and the owner's ruling of 2026-11-27): the envelope rides the delegation's **outbox row** and
//! then the agent's request, **byte for byte**, and it is in no event, no frame and no export. The
//! one message a Stop & send holds keeps its envelope in the job ledger until it gets its row
//! (ANR-1376), and the export takes it out of there.
//!
//! The presence check at intake is the surface's (`orch-surface-agui`'s `prompt_signature.rs`); what
//! this file pins is the seam between them, which is where an envelope is easiest to lose — or to
//! write into the log, which is the one thing it must never do.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, ApplyOutcome, Creation, Inbound, NewThread, StartChildSession};
use orch_core::{
    ChildSessionsOffer, Input, KnownExtension, Origin, PromptClaims, SignedPrompt, ThreadId,
    ThreadState,
};
use orch_ports::memory::Call;
use orch_ports::{AgentError, OutboxPayload, ThreadStore};
use serde_json::json;
use support::*;

/// A compact JWS of three base64url parts with a detached payload: what a signing app sends.
const ENVELOPE: &str =
    "eyJhbGciOiJFZERTQSIsImtpZCI6ImRldmljZS0xI3YxIn0..c2lnbmF0dXJlLW9mLXRoZS1wcm9tcHQ";

/// A signed prompt as the run route makes it, with claims, so that "byte for byte" below is about
/// every member.
fn signed(envelope: &str) -> SignedPrompt {
    SignedPrompt {
        envelope: envelope.to_owned().into(),
        claims: Some(PromptClaims {
            aud: "https://aud.test.invalid".into(),
            thread: "thread-as-sent".into(),
            agent: "agent-as-sent".into(),
            msg: "message-id-as-sent".into(),
            content_sha256: "content-hash".into(),
            props_sha256: "props-hash".into(),
        }),
    }
}

fn new_thread(text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: text.to_owned(),
    }
}

/// A thread created with a first message that carries `signed_prompt`.
async fn start(w: &World, text: &str, signed_prompt: Option<&str>) -> ThreadId {
    let app = w.app();
    let thread = ThreadId(uuid::Uuid::now_v7());
    match app
        .create_thread_as(
            &alice(),
            thread,
            new_thread(text),
            Inbound {
                origin: Origin::Agui,
                signed_prompt: signed_prompt.map(signed),
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

/// Everything an export, a frame and a log line are made of: the thread row, its job ledger (which
/// the row's own serialisation only summarises, and the export document writes whole) and every
/// event, serialised.
async fn exported(w: &World, thread: ThreadId) -> String {
    let export = w.app().export_thread(&alice(), thread).await.unwrap();
    let mut text = serde_json::to_string(&export.thread).unwrap();
    text.push_str(&serde_json::to_string(&export.thread.job).unwrap());
    for event in &export.events {
        text.push_str(&serde_json::to_string(event).unwrap());
    }
    text
}

#[tokio::test]
async fn the_envelope_rides_the_row_and_the_send_and_is_in_no_event() {
    let w = World::new();
    let thread = start(&w, "echo hi", Some(ENVELOPE)).await;

    // The commit's row carries it, byte for byte, so that a redelivery sends the same bytes.
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { signed_prompt, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(*signed_prompt, Some(signed(ENVELOPE)));
    let shown = format!("{:?}", row.payload);
    assert!(!shown.contains(ENVELOPE), "{shown}");

    // The log holds none of it: no event, and so no frame and no export.
    let export = exported(&w, thread).await;
    assert!(
        !export.contains(ENVELOPE),
        "the envelope is in the log: {export}"
    );
    assert!(
        !export.to_lowercase().contains("signedprompt") && !export.contains("signed_prompt"),
        "the log names the envelope: {export}"
    );

    // And the dispatcher puts it on the agent's request, unchanged.
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<SignedPrompt>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.signed_prompt.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [Some(signed(ENVELOPE))], "the send carries it");

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
        row["delegate"].get("signed_prompt").is_none(),
        "no member on the row: {row}"
    );
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<SignedPrompt>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.signed_prompt.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [None]);
}

/// A Stop & send of `alice`'s that a signing app signed.
fn signed_stop(text: &str, envelope: &str) -> Input {
    Input::StopAndSend {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        signed_prompt: Some(signed(envelope)),
    }
}

#[tokio::test]
async fn a_signed_stop_and_send_reaches_the_next_job_byte_for_byte() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "slow job").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    app.submit(
        &alice(),
        t.id,
        signed_stop("echo do X instead", ENVELOPE),
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;

    let sent: Vec<(String, Option<SignedPrompt>)> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some((call.text.clone(), call.signed_prompt.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        [
            ("slow job".to_owned(), None),
            ("echo do X instead".to_owned(), Some(signed(ENVELOPE))),
        ]
    );
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.after_stop_signed_prompt, None);
    assert!(!exported(&w, t.id).await.contains(ENVELOPE));
}

#[tokio::test]
async fn a_refused_cancel_steers_the_held_message_with_its_envelope() {
    let w = World::new();
    w.agent.set_extensions("plain", &[KnownExtension::Steer]);
    // the agent cannot be stopped, for good: the held message goes into the running task
    w.agent.fail_next_cancels(1, || {
        AgentError::NotCancelable("it cannot be stopped".to_owned())
    });
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate refactor the parser").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    app.submit(&alice(), t.id, signed_stop("do X instead", ENVELOPE), None)
        .await
        .unwrap();
    let steered = eventually("the held message was steered", || async {
        let steered: Vec<(String, Option<SignedPrompt>)> = w
            .agent
            .sends()
            .into_iter()
            .filter_map(|call| match call {
                Call::Send(call) if call.steer => {
                    Some((call.text.clone(), call.signed_prompt.clone()))
                }
                _ => None,
            })
            .collect();
        (!steered.is_empty()).then_some(steered)
    })
    .await;
    assert_eq!(
        steered,
        [("do X instead".to_owned(), Some(signed(ENVELOPE)))]
    );
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.after_stop_signed_prompt, None);

    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
    assert!(!exported(&w, t.id).await.contains(ENVELOPE));
}

#[tokio::test]
async fn the_ledger_holds_a_stops_envelope_and_an_export_does_not() {
    // no dispatcher: the stop stays pending
    let w = World::new();
    let thread = start(&w, "echo hi", None).await;
    w.app()
        .submit(&alice(), thread, signed_stop("do X", ENVELOPE), None)
        .await
        .unwrap();

    // the positive control: the ledger as the store keeps it has it
    let stored = w.store.get_thread(None, thread).await.unwrap().unwrap();
    assert_eq!(stored.job.after_stop_signed_prompt, Some(signed(ENVELOPE)));
    let shown = format!("{stored:?}");
    assert!(!shown.contains(ENVELOPE), "{shown}");

    let export = exported(&w, thread).await;
    assert!(export.contains("\"afterStop\""), "not stopping: {export}");
    // the whole value is taken out: no claim of it either
    for value in [
        "aud.test.invalid",
        "thread-as-sent",
        "content-hash",
        "props-hash",
    ] {
        assert!(!export.contains(value), "{value}: {export}");
    }
    assert!(!export.contains(ENVELOPE), "{export}");
    assert!(!export.to_lowercase().contains("signedprompt"), "{export}");
}

#[tokio::test]
async fn a_repeat_of_a_signed_stop_is_a_duplicate_and_another_message_is_a_conflict() {
    // `submit` twice under one key is what two copies of one request that race come to: neither
    // looks the key up before the transition, which by now sees the first one's message held
    let w = World::new();
    let app = w.app();
    let thread = start(&w, "echo hi", None).await;
    let first = app
        .submit(
            &alice(),
            thread,
            signed_stop("do X", ENVELOPE),
            Some("k-stop".to_owned()),
        )
        .await
        .unwrap();
    assert!(matches!(first, ApplyOutcome::Applied { .. }), "{first:?}");
    let before = w.store.get_thread(None, thread).await.unwrap().unwrap();

    let repeat = app
        .submit(
            &alice(),
            thread,
            signed_stop("do X", ENVELOPE),
            Some("k-stop".to_owned()),
        )
        .await
        .unwrap();
    assert!(matches!(repeat, ApplyOutcome::Duplicate), "{repeat:?}");

    for key in [Some("k-other".to_owned()), None] {
        let refused = app
            .submit(&alice(), thread, signed_stop("and Y", ENVELOPE), key)
            .await;
        assert!(
            matches!(
                refused,
                Err(AppError::Blocked {
                    code: "stop_pending",
                    ..
                })
            ),
            "{refused:?}"
        );
    }
    // an unsigned one too: the held message is signed
    let refused = app.post_message(&alice(), thread, "and Z".to_owned()).await;
    assert!(
        matches!(
            refused,
            Err(AppError::Blocked {
                code: "stop_pending",
                ..
            })
        ),
        "{refused:?}"
    );
    let after = w.store.get_thread(None, thread).await.unwrap().unwrap();
    assert_eq!(after, before, "nothing was written");
}

/// A helper start's signed prompt (ANR-1441): the envelope **and** the claims ADR 0087 decided for
/// it (the note of 2026-11-28) reach round 1's outbox row and the dispatcher's request unchanged,
/// and neither is in the log. The claims are what the route builds; the application only carries
/// them, so this pins the seam between the two.
#[tokio::test]
async fn a_helper_starts_signed_prompt_rides_the_row_and_the_send_with_its_claims() {
    let w = World::new();
    w.agent.set_child_sessions(
        "plain",
        Some(
            ChildSessionsOffer::from_params(&json!({
                "agents": [{"name": "planner", "description": "Plans"}],
                "maxOpen": 16
            }))
            .unwrap(),
        ),
    );
    let app = w.app();
    // the parent answers once, so the agent has assigned it the context a session starts in
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    eventually("the context a session starts in", || async {
        w.store
            .get_binding(parent.id)
            .await
            .unwrap()
            .and_then(|binding| binding.context_id)
    })
    .await;
    run.shutdown().await;

    const HELPER_PROMPT: &str = "echo list every banana";
    app.start_child_session(
        &alice(),
        parent.id,
        StartChildSession {
            request: "req-claims".to_owned(),
            agent: "planner".to_owned(),
            description: Some("Find the writers".to_owned()),
            text: HELPER_PROMPT.to_owned(),
            keep_open: false,
            tier: None,
            signed_prompt: Some(signed(ENVELOPE)),
        },
    )
    .await
    .unwrap();

    // The row carries the value, envelope and every claim, byte for byte.
    let rows = w.store.list_open_outbox(parent.id).await.unwrap();
    let row = rows
        .iter()
        .find(|row| matches!(&row.payload, OutboxPayload::SessionStart { .. }))
        .expect("the start's row");
    let OutboxPayload::SessionStart { signed_prompt, .. } = &row.payload else {
        unreachable!("found above")
    };
    assert_eq!(*signed_prompt, Some(signed(ENVELOPE)));
    let shown = format!("{:?}", row.payload);
    assert!(!shown.contains(ENVELOPE), "{shown}");

    // The dispatcher puts it on round 1's request, unchanged.
    let run = spawn_dispatcher(&app, fast(), "d2");
    let sent = eventually("round 1 of the helper", || async {
        let sent: Vec<Option<SignedPrompt>> = w
            .agent
            .sends()
            .into_iter()
            .filter_map(|call| match call {
                Call::Send(call) if call.text == HELPER_PROMPT => Some(call.signed_prompt.clone()),
                _ => None,
            })
            .collect();
        (!sent.is_empty()).then_some(sent)
    })
    .await;
    let child = eventually("the helper's thread", || async {
        w.store
            .child_threads(&alice(), parent.id)
            .await
            .unwrap()
            .first()
            .map(|record| record.id)
    })
    .await;
    run.shutdown().await;
    assert_eq!(sent, [Some(signed(ENVELOPE))], "the send carries it");

    // The log holds neither: no event, and so no frame and no export, for the parent or the child.
    for thread in [parent.id, child] {
        let export = exported(&w, thread).await;
        assert!(
            !export.contains(ENVELOPE),
            "the envelope is in the log: {export}"
        );
        assert!(
            !export.to_lowercase().contains("signedprompt") && !export.contains("signed_prompt"),
            "the log names the envelope: {export}"
        );
        for value in [
            "aud.test.invalid",
            "thread-as-sent",
            "agent-as-sent",
            "message-id-as-sent",
            "content-hash",
            "props-hash",
        ] {
            assert!(!export.contains(value), "a claim is in the log: {value}");
        }
    }
}
