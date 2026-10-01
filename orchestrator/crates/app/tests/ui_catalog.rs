//! The UI catalog through the application (ADR 0023): an input that carries the screen's catalog
//! records it once per digest, and the delegation tells the agent which catalog is current, with
//! the thread it is about, whatever the agent does with it. The in-memory stack and the scripted
//! agent, which records what each request carried.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, Creation, Inbound, NewThread};
use orch_core::{
    EventKind, Input, Origin, ThreadId, ThreadState, UiActionData, UiCatalogData, UiDelivery,
    UiVersion, catalog_digest,
};
use orch_ports::ThreadStore;
use orch_ports::memory::Call;
use support::*;

/// A version of the screen's catalog with its real digest; `tag` tells two of a version apart.
fn catalog(version: u32, tag: &str) -> UiCatalogData {
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    let catalog = serde_json::json!({
        "catalogId": id,
        "components": {"Note": {
            "type": "object",
            "title": format!("{tag}-{version}"),
            "properties": {"component": {"const": "Note"}},
        }},
    });
    UiCatalogData {
        catalog_id: id.to_owned(),
        version,
        digest: catalog_digest(&catalog).unwrap(),
        catalog,
    }
}

fn message(text: &str, catalog: Option<&UiCatalogData>) -> Input {
    Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: catalog.cloned(),
    }
}

/// What each request to the agent carried: the delivery and the thread, in order.
fn deliveries(w: &World) -> Vec<(Option<UiDelivery>, Option<ThreadId>)> {
    w.agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send {
                ui_catalog,
                thread_tools,
                ..
            } => (
                ui_catalog.map(|boxed| *boxed),
                thread_tools.map(|grant| grant.thread),
            ),
            other => panic!("not a send: {other:?}"),
        })
        .collect()
}

fn catalog_events(events: &[orch_core::Event]) -> Vec<UiCatalogData> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            orch_core::EventBody::UiCatalog(d) => Some(d.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_agent_is_told_the_catalog_with_every_message_and_the_thread_it_is_about() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (v1, v2, older) = (catalog(1, "a"), catalog(2, "a"), catalog(1, "b"));

    // A thread that has never been shown a catalog tells the agent nothing of one, and still
    // names the thread.
    let t = create(&app, &alice(), "plain", "echo one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(deliveries(&w), [(None, Some(t.id))]);

    // The first catalog: recorded first, then the message, then the job; the agent gets it inline.
    app.submit(&alice(), t.id, message("echo two", Some(&v1)), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        &shape(&ev)[5..8],
        ["ui_catalog", "user_message", "job_started"]
    );
    assert_eq!(catalog_events(&ev), vec![v1.clone()]);
    assert_eq!(
        deliveries(&w)[1],
        (Some(UiDelivery::Inline(v1.clone())), Some(t.id))
    );

    // The same digest again: nothing more in the log, and a reference for the agent.
    app.submit(&alice(), t.id, message("echo three", Some(&v1)), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        catalog_events(&events(&app, &alice(), t.id).await),
        vec![v1.clone()]
    );
    assert_eq!(deliveries(&w)[2].0, Some(UiDelivery::Ref(v1.reference())));

    // A newer version: recorded, and inline again.
    app.submit(&alice(), t.id, message("echo four", Some(&v2)), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        catalog_events(&events(&app, &alice(), t.id).await),
        [v1.clone(), v2.clone()]
    );
    assert_eq!(deliveries(&w)[3].0, Some(UiDelivery::Inline(v2.clone())));

    // An older UI joins: its catalog is recorded, but the agent is told the newest.
    app.submit(&alice(), t.id, message("echo five", Some(&older)), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        catalog_events(&events(&app, &alice(), t.id).await),
        [v1, v2.clone(), older]
    );
    assert_eq!(deliveries(&w)[4].0, Some(UiDelivery::Ref(v2.reference())));

    // A message that carries none still names the current catalog.
    app.post_message(&alice(), t.id, "echo six".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(deliveries(&w)[5].0, Some(UiDelivery::Ref(v2.reference())));
    assert!(
        deliveries(&w)
            .iter()
            .all(|(_, thread)| *thread == Some(t.id))
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_action_carries_the_catalog_like_a_message_does() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let v1 = catalog(1, "a");
    let t = create(&app, &alice(), "plain", "ask which branch").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;

    let action = UiActionData {
        surface_id: "s1".into(),
        name: "answer".into(),
        source_component_id: "choices".into(),
        context: serde_json::Map::new(),
        version: UiVersion::V0_9_1,
        run_id: None,
    };
    app.submit(
        &alice(),
        t.id,
        Input::UiAction {
            user: alice(),
            action,
            catalog: Some(v1.clone()),
        },
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;

    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        &shape(&ev)[4..6],
        ["ui_catalog", "ui_action"],
        "the catalog first, then the action it came with"
    );
    let sends = w.agent.sends();
    let Call::Send {
        action: Some(_),
        ui_catalog,
        thread_tools,
        ..
    } = &sends[1]
    else {
        panic!("the action is the second send: {sends:?}");
    };
    assert_eq!(ui_catalog.as_deref(), Some(&UiDelivery::Inline(v1)));
    assert_eq!(thread_tools.as_ref().map(|grant| grant.thread), Some(t.id));
    run.shutdown().await;
}

#[tokio::test]
async fn a_catalog_the_envelope_refuses_is_refused_and_nothing_is_written() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "echo one").await;
    let before = events(&app, &alice(), t.id).await;

    let mut forged = catalog(1, "a");
    forged.digest = format!("sha256:{}", "0".repeat(64));
    let err = app
        .submit(&alice(), t.id, message("hi", Some(&forged)), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::Invalid(ref why) if why.contains("digest")),
        "{err:?}"
    );

    let mut huge = catalog(1, "a");
    huge.catalog["components"]["Note"]["description"] = "x".repeat(70_000).into();
    let err = app
        .submit(&alice(), t.id, message("hi", Some(&huge)), None)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Invalid(_)), "{err:?}");

    // nothing was written: no event, and no outbox row beyond the first message's
    assert_eq!(events(&app, &alice(), t.id).await.len(), before.len());
    assert_eq!(w.store.list_open_outbox(t.id).await.unwrap().len(), 1);
    assert_eq!(
        kinds(&events(&app, &alice(), t.id).await)
            .iter()
            .filter(|k| **k == EventKind::UiCatalog)
            .count(),
        0
    );
}

#[tokio::test]
async fn a_thread_created_with_a_catalog_records_it_first_and_delivers_it_inline() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let v1 = catalog(1, "a");

    let id = ThreadId(orch_ports::IdGen::new_id(&w.ids));
    let created = app
        .create_thread_as(
            &alice(),
            id,
            NewThread {
                title: None,
                target: target("plain"),
                text: "echo hi".to_owned(),
            },
            Inbound {
                ui_catalog: Some(v1.clone()),
                ..Inbound::default()
            },
        )
        .await
        .unwrap();
    let Creation::Created { events, .. } = created else {
        panic!("created");
    };
    assert_eq!(
        events.iter().map(|e| e.kind()).collect::<Vec<_>>(),
        [EventKind::UiCatalog, EventKind::UserMessage]
    );
    wait_state(&app, &alice(), id, ThreadState::Done).await;
    assert_eq!(
        deliveries(&w),
        [(Some(UiDelivery::Inline(v1.clone())), Some(id))]
    );
    run.shutdown().await;

    // a schema that does not name its component is refused, and no thread is created
    let mut bad = catalog(1, "b");
    bad.catalog["components"]["Note"]["properties"] = serde_json::json!({});
    bad.digest = catalog_digest(&bad.catalog).unwrap();
    let other = ThreadId(orch_ports::IdGen::new_id(&w.ids));
    let err = app
        .create_thread_as(
            &alice(),
            other,
            NewThread {
                title: None,
                target: target("plain"),
                text: "echo hi".to_owned(),
            },
            Inbound {
                ui_catalog: Some(bad),
                ..Inbound::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::Invalid(ref why) if why.contains("properties.component.const")),
        "{err:?}"
    );
    assert!(app.find_thread(&alice(), other).await.unwrap().is_none());
}

#[tokio::test]
async fn the_thread_tools_endpoint_reads_the_thread_and_the_newest_catalog_without_a_user() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (v1, v2, older) = (catalog(1, "a"), catalog(2, "a"), catalog(1, "b"));

    // nothing has this id: no thread, no catalog
    let unknown = ThreadId(orch_ports::IdGen::new_id(&w.ids));
    assert!(app.thread_for_tools(unknown).await.unwrap().is_none());
    assert!(app.thread_ui_catalog(unknown).await.unwrap().is_none());

    // a thread the web sent no catalog for: it exists (whoever its owner is), and has none
    let t = create(&app, &alice(), "plain", "echo one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let found = app.thread_for_tools(t.id).await.unwrap().unwrap();
    assert_eq!(found.owner, alice());
    assert_eq!(found.target.agent_id.as_str(), "plain");
    assert!(app.thread_ui_catalog(t.id).await.unwrap().is_none());

    // version 1, then version 2: the newest, with its catalog
    for (text, sent) in [("echo two", &v1), ("echo three", &v2)] {
        app.submit(&alice(), t.id, message(text, Some(sent)), None)
            .await
            .unwrap();
        wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    }
    assert_eq!(app.thread_ui_catalog(t.id).await.unwrap(), Some(v2.clone()));

    // an older screen joins: recorded, but the newest stays
    app.submit(&alice(), t.id, message("echo four", Some(&older)), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(app.thread_ui_catalog(t.id).await.unwrap(), Some(v2));
    run.shutdown().await;
}

/// The ledger records every new digest and only a version at least the current one becomes
/// current, so any number of lower-versioned catalogs can follow the current one in the log. Its
/// event is found by its digest, not by a window of the newest events (which 33 or more such
/// catalogs would push it out of).
#[tokio::test]
async fn the_current_catalog_is_found_however_many_older_ones_were_recorded_after_it() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let current = catalog(50, "current");

    let t = create(&app, &alice(), "plain", "echo first").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    app.submit(
        &alice(),
        t.id,
        message("echo current", Some(&current)),
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        app.thread_ui_catalog(t.id).await.unwrap(),
        Some(current.clone())
    );

    // 40 older screens, each its own digest, after the newest one
    for n in 0..40 {
        let older = catalog(1 + n % 9, &format!("old-{n}"));
        app.submit(&alice(), t.id, message("echo older", Some(&older)), None)
            .await
            .unwrap();
        wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    }
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(catalog_events(&ev).len(), 41, "every digest was recorded");
    let record = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        record.job.catalog.current().map(|c| c.version),
        Some(50),
        "the thread keeps the newest"
    );
    // more than 32 `ui_catalog` events came after the current one's: a window of the newest 32 of
    // the kind does not hold it
    let newest_32: Vec<_> = ev
        .iter()
        .rev()
        .filter(|e| e.kind() == EventKind::UiCatalog)
        .take(32)
        .collect();
    assert!(
        newest_32
            .iter()
            .all(|e| !matches!(&e.body, orch_core::EventBody::UiCatalog(d) if *d == current)),
        "the current catalog's event is outside the newest 32"
    );
    assert_eq!(
        app.thread_ui_catalog(t.id).await.unwrap(),
        Some(current),
        "still the current one"
    );
    run.shutdown().await;
}
