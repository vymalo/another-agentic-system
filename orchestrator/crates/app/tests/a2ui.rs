//! A2UI through the durable dispatcher (ADR 0013): a surface from the agent is a `ui_surface`
//! event before the wait it accompanies, a user's action is a `ui_action` event and a delegation
//! to the same task, and the action's sizes are checked before anything is stored.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{EventBody, Input, MAX_ACTION_CONTEXT_BYTES, ThreadState, UiActionData, UiVersion};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, ThreadStore};
use serde_json::json;
use support::*;

fn action(name: &str) -> UiActionData {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    UiActionData {
        surface_id: "s1".into(),
        name: name.into(),
        source_component_id: "go".into(),
        context,
        version: UiVersion::V0_9_1,
        run_id: Some("run-2".into()),
    }
}

fn input(a: UiActionData) -> Input {
    Input::UiAction {
        user: alice(),
        action: a,
        catalog: None,
    }
}

#[tokio::test]
async fn a_surface_is_recorded_before_the_question_and_an_action_answers_it() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ui pick one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "ui_surface",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    let EventBody::UiSurface(surface) = &ev[2].body else {
        panic!("{:?}", ev[2]);
    };
    assert_eq!(surface.operations.len(), 2);
    assert_eq!(ev[2].actor.name, "plain", "attributed to the agent");

    let outcome = app
        .submit(&alice(), t.id, input(action("go")), Some("k-1".into()))
        .await
        .unwrap();
    assert!(matches!(outcome, orch_app::ApplyOutcome::Applied { .. }));
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;

    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev)[5..],
        [
            "ui_action",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&ev);
    assert_eq!(
        ev[5].actor.name, "alice@example.com",
        "attributed to the user"
    );
    assert_eq!(ev[5].body, EventBody::UiAction(action("go")));

    // The agent got the action as an action, on the same task, not as text.
    let sends = w.agent.sends();
    assert_eq!(sends.len(), 2);
    match (&sends[0], &sends[1]) {
        (
            Call::Send { task_id: None, .. },
            Call::Send {
                task_id: Some(task),
                action: Some(a),
                ..
            },
        ) => {
            assert_eq!(task, "task-1");
            assert_eq!(**a, action("go"));
        }
        other => panic!("{other:?}"),
    }
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn a_replayed_action_is_written_once() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ui pick one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    let first = app
        .submit(
            &alice(),
            t.id,
            input(action("go")),
            Some("agui:t:run:r".into()),
        )
        .await
        .unwrap();
    assert!(matches!(first, orch_app::ApplyOutcome::Applied { .. }));
    let again = app
        .submit(
            &alice(),
            t.id,
            input(action("go")),
            Some("agui:t:run:r".into()),
        )
        .await
        .unwrap();
    assert!(matches!(again, orch_app::ApplyOutcome::Duplicate));
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e.body, EventBody::UiAction(_)))
            .count(),
        1
    );
    assert_eq!(w.agent.sends().len(), 2, "delegated once");
    run.shutdown().await;
}

#[tokio::test]
async fn an_oversized_or_malformed_action_writes_nothing() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "ui pick one").await;
    let before = events(&app, &alice(), t.id).await.len();

    let mut big = action("go");
    big.context
        .insert("k".into(), json!("v".repeat(MAX_ACTION_CONTEXT_BYTES)));
    let mut nameless = action("");
    nameless.name.clear();
    for a in [big, nameless] {
        let err = app
            .submit(&alice(), t.id, input(a), None)
            .await
            .unwrap_err();
        assert!(matches!(err, orch_app::AppError::Invalid(_)), "{err:?}");
    }
    assert_eq!(events(&app, &alice(), t.id).await.len(), before);
    assert_eq!(
        w.store.list_open_outbox(t.id).await.unwrap().len(),
        1,
        "only the first message's delegation"
    );
}

#[tokio::test]
async fn an_action_on_a_finished_thread_is_refused_and_someone_elses_thread_is_not_found() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let err = app
        .submit(&alice(), t.id, input(action("go")), None)
        .await
        .unwrap_err();
    assert!(matches!(err, orch_app::AppError::Finished), "{err:?}");
    let err = app
        .submit(
            &bob(),
            t.id,
            Input::UiAction {
                user: bob(),
                action: action("go"),
                catalog: None,
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, orch_app::AppError::NotFound), "{err:?}");
    run.shutdown().await;
}

#[tokio::test]
async fn the_action_row_is_a_delegation_with_its_own_payload() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "ui pick one").await;
    // Not dispatched: look at what the commit wrote.
    let open = w.store.list_open_outbox(t.id).await.unwrap();
    assert!(matches!(open[0].payload, OutboxPayload::Delegate { .. }));
    // Move the thread to blocked by hand, as the dispatcher would after the agent asked.
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    run.shutdown().await;
    app.submit(&alice(), t.id, input(action("go")), None)
        .await
        .unwrap();
    let open = w.store.list_open_outbox(t.id).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].kind, orch_ports::OutboxKind::Delegate);
    let OutboxPayload::Action { action: a, at, .. } = &open[0].payload else {
        panic!("{:?}", open[0].payload);
    };
    assert_eq!(a, &action("go"));
    assert!(at.as_second() > 0);
    // The payload survives the JSON the Postgres store keeps it in.
    let json = serde_json::to_value(&open[0].payload).unwrap();
    assert_eq!(
        serde_json::from_value::<OutboxPayload>(json).unwrap(),
        open[0].payload
    );
}
