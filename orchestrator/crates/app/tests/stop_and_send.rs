//! Stop & send (ADR 0036) through the application, the dispatcher and the scripted agent: the
//! running task is cancelled, and the next job's task names the cancelled one in
//! `referenceTaskIds`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{EventBody, ThreadState};
use orch_ports::memory::Call;
use orch_ports::{OutboxStatus, ThreadStore};
use support::*;

/// The messages that reached the agent as messages (a steer the agent refused did not).
fn sends(w: &World) -> Vec<(Option<String>, Vec<String>, String)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                task_id,
                reference_task_ids,
                text,
                steer: false,
                ..
            } => Some((task_id, reference_task_ids, text)),
            Call::Send { steer: true, .. } => None,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn stop_and_send_cancels_the_running_task_and_the_next_task_names_it() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "slow job").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    let message = app
        .stop_and_send(&alice(), t.id, "echo do X instead".into())
        .await
        .unwrap();
    let EventBody::UserMessage(data) = &message.body else {
        panic!("{message:?}");
    };
    assert_eq!(data.delivery, Some(orch_core::Delivery::Interrupt));

    // the next job runs to its end: the abandoned one never shows `done` or `cancelled`
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "user_message",
            "agent_status:canceled",
            "job_started",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done",
        ]
    );
    assert_contiguous(&ev);

    // the agent was asked to cancel the running task, and only it
    assert_eq!(
        w.agent
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Cancel { .. }))
            .collect::<Vec<_>>(),
        [Call::Cancel {
            task_id: "task-1".into()
        }]
    );
    // the next task is a new one (no `taskId`), says what it continues, and carries the text
    assert_eq!(
        sends(&w),
        [
            (None, vec![], "slow job".to_owned()),
            (
                None,
                vec!["task-1".to_owned()],
                "echo do X instead".to_owned()
            ),
        ]
    );
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.number, 2);
    assert_eq!(thread.job.after_stop, None);
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_sent_before_the_stop_is_superseded_and_never_runs_ahead_of_the_next_job() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "slow job").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;

    // a plain Send while the job runs: logged as a steer; the plain agent does not list `steer/v1`,
    // so the row falls back to a delegation behind the one in flight
    let steered = app
        .post_message(&alice(), t.id, "echo steered".into())
        .await
        .unwrap();
    let EventBody::UserMessage(data) = &steered.body else {
        panic!("{steered:?}");
    };
    assert_eq!(data.delivery, Some(orch_core::Delivery::Steer));
    eventually("the steered message waits as a delegation", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (open
            .iter()
            .filter(|r| r.status == OutboxStatus::Pending)
            .count()
            == 1)
            .then_some(())
    })
    .await;

    app.stop_and_send(&alice(), t.id, "echo do X instead".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;

    // the messages stay in the log, and the one the stop superseded was never sent
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev).iter().filter(|k| *k == "user_message").count(),
        3
    );
    assert_eq!(
        sends(&w),
        [
            (None, vec![], "slow job".to_owned()),
            (
                None,
                vec!["task-1".to_owned()],
                "echo do X instead".to_owned()
            ),
        ]
    );
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn two_stops_are_one_job_with_both_messages() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "slow job").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    // both land in one commit order before the dispatcher can look: the second joins the first
    // if the stop has not landed yet, or is a message of the next job if it has
    app.stop_and_send(&alice(), t.id, "echo one".into())
        .await
        .unwrap();
    app.stop_and_send(&alice(), t.id, "echo two".into())
        .await
        .unwrap();
    eventually("the thread settles", || async {
        let thread = app.get_thread(&alice(), t.id).await.unwrap();
        (thread.state == ThreadState::Done || thread.state == ThreadState::Cancelled)
            .then_some(thread)
    })
    .await;
    assert_ne!(
        state_of(&w, t.id).await,
        ThreadState::Cancelled,
        "a stop & send never ends cancelled"
    );
    let sent = sends(&w);
    assert!(sent.len() >= 2 && sent.len() <= 3, "{sent:?}");
    assert_eq!(
        sent[1].1,
        ["task-1"],
        "the next task names the cancelled one"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn stop_and_send_on_a_finished_thread_is_a_plain_message() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let message = app
        .stop_and_send(&alice(), t.id, "echo two".into())
        .await
        .unwrap();
    let EventBody::UserMessage(data) = &message.body else {
        panic!("{message:?}");
    };
    assert_eq!(data.delivery, None, "nothing was running");
    eventually("the second job ends", || async {
        let thread = app.get_thread(&alice(), t.id).await.unwrap();
        (thread.job.number == 2 && thread.state == ThreadState::Done).then_some(())
    })
    .await;
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. })),
        "nothing to cancel"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn the_text_a_stopping_job_may_hold_is_bounded_and_the_surface_says_422() {
    let w = World::new();
    let app = w.app();
    // no dispatcher: the job stays `queued` and the stop stays on its way
    let t = create(&app, &alice(), "plain", "slow job").await;
    let big = "a".repeat(40 * 1024);
    app.stop_and_send(&alice(), t.id, big.clone())
        .await
        .unwrap();
    let refused = app.stop_and_send(&alice(), t.id, big).await;
    assert!(
        matches!(refused, Err(orch_app::AppError::Unprocessable(_))),
        "{refused:?}"
    );
    // nothing was written for the refused one
    assert_eq!(
        shape(&events(&app, &alice(), t.id).await),
        ["user_message", "user_message"]
    );
}
