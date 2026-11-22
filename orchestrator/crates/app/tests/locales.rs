//! The interface locale of a message (ADR 0085, `locale/v1`) through the application and the
//! dispatcher: the tag a surface names reaches the delegation and the send, and it is **recorded
//! once, on the `user_message`** — the log is the state, so an export shows what the agent was told
//! and nothing else of the thread carries it.
//!
//! The tag's shape (a **400**) is `orch_app::locale`'s, tested beside it; the metadata an agent is
//! actually sent is `orch-agent-a2a`'s. What this file pins is the wiring between them, which is
//! where a hint is easiest to lose.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{Creation, Inbound, NewThread};
use orch_core::{LocaleHint, Origin, ThreadId, ThreadState};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, ThreadStore};
use support::*;

fn tag(text: &str) -> LocaleHint {
    LocaleHint::parse(text).expect("a well-formed tag")
}

fn new_thread(text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: text.to_owned(),
    }
}

/// A thread created with a first message naming `locale`.
async fn start(w: &World, text: &str, locale: Option<&str>) -> ThreadId {
    let app = w.app();
    let thread = ThreadId(uuid::Uuid::now_v7());
    match app
        .create_thread_as(
            &alice(),
            thread,
            new_thread(text),
            Inbound {
                locale: locale.map(tag),
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

#[tokio::test]
async fn the_locale_reaches_the_delegation_and_the_send_and_is_recorded_once() {
    let w = World::new();
    let thread = start(&w, "corrige la page de connexion", Some("fr-CM")).await;

    // the row the commit wrote carries it for the dispatcher
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { locale, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    assert_eq!(locale.as_ref(), Some(&tag("fr-CM")));

    // the log records it, on the `user_message`, and nowhere else: the log is the state
    let export = w.app().export_thread(&alice(), thread).await.unwrap();
    let logged = serde_json::to_value(
        export
            .events
            .iter()
            .find(|e| e.kind() == orch_core::EventKind::UserMessage)
            .expect("a user_message"),
    )
    .unwrap();
    assert_eq!(logged["data"]["locale"], "fr-CM", "{logged}");
    let whole: Vec<serde_json::Value> = export
        .events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    let occurrences = serde_json::to_string(&whole)
        .unwrap()
        .matches("fr-CM")
        .count();
    assert_eq!(
        occurrences, 1,
        "the tag is on the user_message and nowhere else"
    );

    // and the dispatcher put it on the request: the agent is sent the message, with the tag
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<LocaleHint>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.locale.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [Some(tag("fr-CM"))], "the send carries it");
}

#[tokio::test]
async fn a_message_that_names_no_locale_carries_none_and_writes_the_event_it_always_did() {
    let w = World::new();
    let thread = start(&w, "hi", None).await;
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let row = serde_json::to_value(&rows[0].payload).unwrap();
    assert!(
        row["delegate"].get("locale").is_none(),
        "no member on the row: {row}"
    );
    let run = spawn_dispatcher(&w.app(), fast(), "d1");
    wait_state(&w.app(), &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let sent: Vec<Option<LocaleHint>> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send(call) => Some(call.locale.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [None]);
}

#[tokio::test]
async fn a_message_the_person_sends_into_the_thread_carries_its_own_locale() {
    let w = World::new();
    let thread = start(&w, "hi", Some("en-CM")).await;
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    // a follow-up, on a thread that is done: the next job's delegation carries its tag
    app.submit(
        &alice(),
        thread,
        orch_core::Input::UserMessage {
            user: alice(),
            text: "et maintenant la redirection".to_owned(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: Some(tag("ybb-CM")),
        },
        None,
    )
    .await
    .unwrap();
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let carried: Vec<Option<LocaleHint>> = rows
        .iter()
        .filter_map(|row| match &row.payload {
            OutboxPayload::Delegate { locale, .. } | OutboxPayload::Steer { locale, .. } => {
                Some(locale.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(carried, [Some(tag("ybb-CM"))]);
}
