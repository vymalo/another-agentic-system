//! The mode of a message (ADR 0109, `agent-mode/v1`) through the application, the dispatcher and the
//! scripted agent: what is checked before anything is written (the live card, **fail closed**), the
//! precedence message over thread over nothing, what the log records, and what a redelivery, an edit
//! and a fork's first message carry.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, Creation, FirstMessage, ForkAt, ForkRequest, Inbound, NewThread};
use orch_core::{
    Classify, ErrorClass, EventBody, EventKind, Input, ModeName, ModeOffer, Origin, ThreadId,
    ThreadState,
};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, ThreadStore};
use serde_json::json;
use support::*;

fn plan() -> ModeName {
    ModeName::parse("plan").unwrap()
}

fn build() -> ModeName {
    ModeName::parse("build").unwrap()
}

/// A world whose agent `plain` lists `agent-mode/v1` with `build` (default) and `plan`.
fn offering() -> World {
    let w = World::new();
    w.agent.set_mode(
        "plain",
        ModeOffer::from_params(&json!({
            "modes": [
                {"name": "build", "title": "Build", "description": "Make the change."},
                {"name": "plan", "title": "Plan", "description": "Read and plan."}
            ],
            "default": "build"
        })),
    );
    w
}

fn new_thread(text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: text.to_owned(),
    }
}

async fn start(w: &World, text: &str, mode: Option<ModeName>) -> Result<ThreadId, AppError> {
    let thread = ThreadId(uuid::Uuid::now_v7());
    match w
        .app()
        .create_thread_as(
            &alice(),
            thread,
            new_thread(text),
            Inbound {
                mode,
                ..Inbound::default()
            },
        )
        .await?
    {
        Creation::Created { .. } => Ok(thread),
        Creation::Exists => panic!("a fresh id"),
    }
}

fn message(text: &str, mode: Option<ModeName>) -> Input {
    Input::UserMessage {
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
        mode,
        prompt_signature: None,
    }
}

/// The mode every `user_message` of the thread recorded, oldest first.
async fn recorded(w: &World, thread: ThreadId) -> Vec<Option<ModeName>> {
    w.app()
        .export_thread(&alice(), thread)
        .await
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::UserMessage(m) => Some(m.mode.clone()),
            _ => None,
        })
        .collect()
}

fn sent(w: &World) -> Vec<Option<ModeName>> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.mode.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_mode_reaches_the_delegation_the_send_and_is_recorded_once() {
    let w = offering();
    let thread = start(&w, "think about it", Some(plan())).await.unwrap();

    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { mode, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(mode, &Some(plan()), "the row carries it for the dispatcher");

    assert_eq!(
        recorded(&w, thread).await,
        [Some(plan())],
        "the log is the state"
    );

    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    assert_eq!(sent(&w), [Some(plan())], "the send carries it");
}

#[tokio::test]
async fn a_mode_the_card_does_not_offer_is_422_and_writes_nothing() {
    let w = offering();
    let error = start(&w, "go", Some(ModeName::parse("review").unwrap()))
        .await
        .unwrap_err();
    let AppError::ModeNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(refusal.code(), "mode_not_offered");
    assert_eq!(refusal.offered(), ["build", "plan"]);
    assert_eq!(error.class(), ErrorClass::Rejected);
    // nothing was written: no thread, so nothing to send
    let listed = w
        .store
        .list_threads(&alice(), orch_ports::ThreadListing::recent(None, 50, true))
        .await
        .unwrap();
    assert!(listed.is_empty(), "{listed:?}");
    assert!(w.agent.sends().is_empty(), "nothing was sent");
}

#[tokio::test]
async fn an_agent_that_lists_nothing_refuses_a_mode_with_an_empty_offer() {
    let w = World::new();
    let error = start(&w, "go", Some(plan())).await.unwrap_err();
    let AppError::ModeNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(refusal.code(), "mode_not_offered");
    assert!(refusal.offered().is_empty());
}

#[tokio::test]
async fn a_card_that_lists_the_extension_badly_refuses_a_mode_with_an_empty_offer() {
    let w = World::new();
    w.agent.set_mode_malformed("plain");
    let error = start(&w, "go", Some(plan())).await.unwrap_err();
    let AppError::ModeNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert!(refusal.offered().is_empty());
}

#[tokio::test]
async fn a_message_that_names_no_mode_writes_the_event_it_always_did_and_reads_no_card() {
    let w = World::new();
    let thread = start(&w, "hi", None).await.unwrap();
    assert_eq!(recorded(&w, thread).await, [None]);
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let row = serde_json::to_value(&rows[0].payload).unwrap();
    assert!(row["delegate"].get("mode").is_none(), "{row}");
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    assert_eq!(sent(&w), [None]);
}

#[tokio::test]
async fn the_message_beats_the_thread_which_beats_nothing() {
    let w = offering();
    let app = w.app();
    let thread = start(&w, "first", None).await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;

    // the thread's default is checked when it is set
    let error = app
        .set_thread_mode(
            &alice(),
            thread,
            Some(Some(ModeName::parse("review").unwrap())),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::ModeNotOffered(_)), "{error:?}");
    assert_eq!(w.store.thread_mode(thread).await.unwrap(), None);
    let (_, set) = app
        .set_thread_mode(&alice(), thread, Some(Some(plan())))
        .await
        .unwrap();
    assert_eq!(set, Some(plan()));

    // a message that names none is sent with the thread's default
    app.submit(&alice(), thread, message("second", None), None)
        .await
        .unwrap();
    // one that names a mode has it over the thread's
    app.submit(&alice(), thread, message("third", Some(build())), None)
        .await
        .unwrap();
    assert_eq!(
        recorded(&w, thread).await,
        [None, Some(plan()), Some(build())]
    );

    // clearing the default leaves the next message with none
    app.set_thread_mode(&alice(), thread, Some(None))
        .await
        .unwrap();
    app.submit(&alice(), thread, message("fourth", None), None)
        .await
        .unwrap();
    assert_eq!(recorded(&w, thread).await.last(), Some(&None));
}

#[tokio::test]
async fn a_default_the_card_has_dropped_refuses_the_message_and_never_falls_back() {
    let w = offering();
    let app = w.app();
    let thread = start(&w, "first", None).await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    app.set_thread_mode(&alice(), thread, Some(Some(plan())))
        .await
        .unwrap();
    // the agent is redeployed without the mode
    w.agent.set_mode("plain", None);
    let before = recorded(&w, thread).await.len();
    let error = app
        .submit(&alice(), thread, message("go on", None), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::ModeNotOffered(_)), "{error:?}");
    assert_eq!(
        recorded(&w, thread).await.len(),
        before,
        "nothing was written"
    );
}

#[tokio::test]
async fn a_thread_that_is_not_the_persons_has_no_default_to_set() {
    let w = offering();
    let thread = start(&w, "first", None).await.unwrap();
    let error = w
        .app()
        .set_thread_mode(&bob(), thread, Some(Some(plan())))
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::NotFound), "{error:?}");
}

#[tokio::test]
async fn an_edit_and_a_forks_first_message_carry_the_mode_and_check_it() {
    let w = offering();
    let app = w.app();
    let thread = start(&w, "write it", Some(plan())).await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let events = app.export_thread(&alice(), thread).await.unwrap().events;
    let user_seq = events
        .iter()
        .find(|e| e.kind() == EventKind::UserMessage)
        .unwrap()
        .seq;

    // an edit that names none continues the replaced message's
    let edit = ForkRequest {
        at: ForkAt::Replace {
            seq: user_seq,
            text: "write it again".into(),
            message_id: None,
            tier: None,
            locale: None,
            pick: None,
            mode: None,
        },
        target: None,
        id: None,
    };
    let forked = app.fork_thread(&alice(), thread, edit).await.unwrap();
    assert_eq!(recorded(&w, forked.thread.id).await, [Some(plan())]);

    // a fork's first message names its own, checked: a mode the card does not list is refused
    let refused = ForkRequest {
        at: ForkAt::AfterTurn {
            seq: events.last().unwrap().seq,
            first: Some(FirstMessage {
                text: "next".into(),
                message_id: None,
                run_id: None,
                origin: Origin::Agui,
                ui_catalog: None,
                mentions: Vec::new(),
                tier: None,
                locale: None,
                pick: None,
                mode: Some(ModeName::parse("review").unwrap()),
                prompt_signature: None,
            }),
        },
        target: None,
        id: None,
    };
    let error = app
        .fork_thread(&alice(), thread, refused)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::ModeNotOffered(_)), "{error:?}");
}

#[tokio::test]
async fn a_thread_has_no_default_until_one_is_set_and_the_card_is_read_then() {
    let w = offering();
    let thread = start(&w, "first", None).await.unwrap();
    assert_eq!(w.store.thread_mode(thread).await.unwrap(), None);
    let (_, set) = w
        .app()
        .set_thread_mode(&alice(), thread, Some(Some(build())))
        .await
        .unwrap();
    assert_eq!(set, Some(build()));
    assert_eq!(w.store.thread_mode(thread).await.unwrap(), Some(build()));
}
