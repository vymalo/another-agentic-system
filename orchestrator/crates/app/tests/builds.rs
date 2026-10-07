//! The build of the agent that works on a thread is noted when the dispatcher gives it work
//! (ADR 0053): what its live card said, in the job ledger, never in the way of a send.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::ThreadState;
use support::*;

#[tokio::test]
async fn a_delegation_notes_what_the_agents_card_says_and_a_second_one_notes_nothing_new() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo one").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let first = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(first.job.builds.len(), 1, "{:?}", first.job.builds);
    let b = &first.job.builds[0];
    assert_eq!(b.agent.as_str(), "plain");
    assert_eq!(b.name.as_deref(), Some("Scripted agent"));
    assert_eq!(b.version.as_deref(), Some("1.0.0"));
    assert_eq!(b.job, 1);

    // the next job: the same card says the same, so nothing is added and the conversation keeps it
    app.post_message(&alice(), t.id, "echo two".to_owned())
        .await
        .unwrap();
    eventually("the second job to finish", || async {
        let thread = app.get_thread(&alice(), t.id).await.unwrap();
        (thread.job.number == 2 && thread.state == ThreadState::Done).then_some(())
    })
    .await;
    let second = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(second.job.builds, first.job.builds);
    // a note is not an event: the log has the five events of a turn, twice, and nothing else
    let ev = events(&app, &alice(), t.id).await;
    assert!(
        ev.iter().all(|e| e.kind().as_str() != "agent_build"),
        "{:?}",
        shape(&ev)
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_card_that_cannot_be_read_notes_nothing_and_the_send_goes_on() {
    let w = World::new();
    w.agent.set_card_down("plain", true);
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert!(thread.job.builds.is_empty(), "unknown stays unknown");
    run.shutdown().await;
}
