//! The model and reasoning effort of a message (ADR 0086, `model-pick/v1`) through the application,
//! the dispatcher and the scripted agent: what is checked before anything is written (the live
//! card, **fail closed**), the precedence message over thread over the agent's own default, what
//! the log records, and what a redelivery and an edit carry.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, Creation, FirstMessage, ForkAt, ForkRequest, Inbound, NewThread};
use orch_core::{
    Classify, EffortOffer, ErrorClass, EventBody, EventKind, Input, ModelId, ModelPick,
    ModelPickOffer, Origin, ReasoningEffort, ThreadId, ThreadState,
};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, ThreadStore};
use serde_json::json;
use support::*;

fn deep_high() -> ModelPick {
    ModelPick {
        model: Some(ModelId::parse("deep").unwrap()),
        reasoning_effort: Some(ReasoningEffort::High),
    }
}

fn fast_pick() -> ModelPick {
    ModelPick::model("fast").unwrap()
}

/// A world whose agent `plain` lists `model-pick/v1` (`fast`, `deep`) and `reasoning/v1`
/// (`low`, `high`).
fn offering() -> World {
    let w = World::new();
    w.agent.set_pick(
        "plain",
        ModelPickOffer::from_params(&json!({"choices": [{"id": "fast", "label": "Fast"}, {"id": "deep"}], "default": "fast"})),
        EffortOffer::from_params(&json!({"efforts": ["low", "high"]})),
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

async fn start(w: &World, text: &str, pick: Option<ModelPick>) -> Result<ThreadId, AppError> {
    let thread = ThreadId(uuid::Uuid::now_v7());
    match w
        .app()
        .create_thread_as(
            &alice(),
            thread,
            new_thread(text),
            Inbound {
                pick,
                ..Inbound::default()
            },
        )
        .await?
    {
        Creation::Created { .. } => Ok(thread),
        Creation::Exists => panic!("a fresh id"),
    }
}

fn message(text: &str, pick: Option<ModelPick>) -> Input {
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
        pick,
    }
}

/// The pick every `user_message` of the thread recorded, oldest first.
async fn recorded(w: &World, thread: ThreadId) -> Vec<Option<ModelPick>> {
    w.app()
        .export_thread(&alice(), thread)
        .await
        .unwrap()
        .events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::UserMessage(m) => Some(m.pick.clone()),
            _ => None,
        })
        .collect()
}

fn sent(w: &World) -> Vec<Option<ModelPick>> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.pick.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_pick_reaches_the_delegation_and_the_send_and_is_recorded_once() {
    let w = offering();
    let thread = start(&w, "think about it", Some(deep_high()))
        .await
        .unwrap();

    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { pick, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(
        pick,
        &Some(deep_high()),
        "the row carries it for the dispatcher"
    );

    assert_eq!(
        recorded(&w, thread).await,
        [Some(deep_high())],
        "the log is the state"
    );

    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    assert_eq!(sent(&w), [Some(deep_high())], "the send carries it");
}

#[tokio::test]
async fn a_pick_the_card_does_not_offer_is_422_and_writes_nothing() {
    let w = offering();
    // a model the card does not list
    let error = start(&w, "go", Some(ModelPick::model("huge").unwrap()))
        .await
        .unwrap_err();
    let AppError::PickNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(refusal.code(), "model_not_offered");
    assert_eq!(refusal.offered(), ["fast", "deep"]);
    assert_eq!(error.class(), ErrorClass::Rejected);
    // an effort the card does not list
    let error = start(&w, "go", Some(ModelPick::effort(ReasoningEffort::Max)))
        .await
        .unwrap_err();
    let AppError::PickNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(refusal.code(), "reasoning_not_offered");
    assert_eq!(refusal.offered(), ["low", "high"]);
    // nothing was written: no thread, so nothing to send
    let listed = w
        .store
        .list_threads(&alice(), orch_ports::ThreadListing::recent(None, 50, true))
        .await
        .unwrap();
    assert!(listed.is_empty(), "{listed:?}");
}

#[tokio::test]
async fn an_agent_that_lists_nothing_refuses_a_pick_with_an_empty_offer() {
    let w = World::new();
    let error = start(&w, "go", Some(fast_pick())).await.unwrap_err();
    let AppError::PickNotOffered(refusal) = &error else {
        panic!("{error:?}")
    };
    assert_eq!(refusal.code(), "model_not_offered");
    assert!(refusal.offered().is_empty());
}

#[tokio::test]
async fn a_message_that_names_no_pick_writes_the_event_it_always_did_and_reads_no_card() {
    let w = World::new();
    let thread = start(&w, "hi", None).await.unwrap();
    assert_eq!(recorded(&w, thread).await, [None]);
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let row = serde_json::to_value(&rows[0].payload).unwrap();
    assert!(row["delegate"].get("pick").is_none(), "{row}");
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    assert_eq!(sent(&w), [None]);
}

#[tokio::test]
async fn the_message_beats_the_thread_which_beats_the_agents_own_default() {
    let w = offering();
    let app = w.app();
    let thread = start(&w, "first", None).await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;

    // the thread's default is checked when it is set
    let error = app
        .set_thread_pick(&alice(), thread, Some(ModelId::parse("huge").ok()), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::PickNotOffered(_)), "{error:?}");
    assert_eq!(
        w.store.thread_pick(thread).await.unwrap(),
        ModelPick::default()
    );
    let (_, set) = app
        .set_thread_pick(
            &alice(),
            thread,
            Some(ModelId::parse("fast").ok()),
            Some(Some(ReasoningEffort::Low)),
        )
        .await
        .unwrap();
    assert_eq!(set.model.as_ref().map(ModelId::as_str), Some("fast"));

    // a message that names none is sent with the thread's default
    app.submit(&alice(), thread, message("second", None), None)
        .await
        .unwrap();
    // one that names a model has it over the thread's, the thread's effort for the other axis
    app.submit(
        &alice(),
        thread,
        message("third", Some(ModelPick::model("deep").unwrap())),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        recorded(&w, thread).await,
        [
            None,
            Some(ModelPick {
                model: Some(ModelId::parse("fast").unwrap()),
                reasoning_effort: Some(ReasoningEffort::Low),
            }),
            Some(ModelPick {
                model: Some(ModelId::parse("deep").unwrap()),
                reasoning_effort: Some(ReasoningEffort::Low),
            }),
        ]
    );

    // clearing the default leaves the next message with none
    app.set_thread_pick(&alice(), thread, Some(None), Some(None))
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
    app.set_thread_pick(&alice(), thread, Some(ModelId::parse("deep").ok()), None)
        .await
        .unwrap();
    // the agent is redeployed without the choice
    w.agent.set_pick("plain", None, None);
    let before = recorded(&w, thread).await.len();
    let error = app
        .submit(&alice(), thread, message("go on", None), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::PickNotOffered(_)), "{error:?}");
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
    let error = app_of(&w)
        .set_thread_pick(&bob(), thread, Some(ModelId::parse("fast").ok()), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::NotFound), "{error:?}");
}

fn app_of(w: &World) -> std::sync::Arc<TestApp> {
    w.app()
}

#[tokio::test]
async fn an_edit_and_a_forks_first_message_carry_the_pick_and_check_it() {
    let w = offering();
    let app = w.app();
    let thread = start(&w, "write it", Some(deep_high())).await.unwrap();
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
        },
        target: None,
        id: None,
    };
    let forked = app.fork_thread(&alice(), thread, edit).await.unwrap();
    assert_eq!(recorded(&w, forked.thread.id).await, [Some(deep_high())]);

    // a fork's first message names its own, checked: a model the card does not list is refused
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
                pick: Some(ModelPick::model("huge").unwrap()),
            }),
        },
        target: None,
        id: None,
    };
    let error = app
        .fork_thread(&alice(), thread, refused)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::PickNotOffered(_)), "{error:?}");
}
