//! Cancelling sends `CancelTask` to the agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use common::*;
use orch_testsupport::CallKind;

#[tokio::test]
async fn cancel_sends_cancel_task_and_cancels_the_thread() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "slow work", None).await;
    chat.wait_state(&id, "working").await;

    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;

    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:canceled",
            "thread_state:cancelled"
        ]
    );
    assert_contiguous(&events);
    let cancels = world.plain.cancels();
    assert_eq!(cancels.len(), 1, "exactly one CancelTask reached the agent");
    assert_eq!(cancels[0].kind, CallKind::Cancel);
    assert_eq!(cancels[0].task_id, world.plain.executions()[0].task_id);
}

#[tokio::test]
async fn cancelling_a_finished_thread_is_accepted_and_changes_nothing() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo done", None).await;
    chat.wait_state(&id, "done").await;
    let before = chat.events(&id).await;

    assert_eq!(chat.cancel(&id).await, 202);
    assert_eq!(chat.events(&id).await, before, "no new events");
    assert!(
        world.plain.cancels().is_empty(),
        "nothing to cancel on the agent"
    );
    assert_eq!(chat.state(&id).await, "done");
}

#[tokio::test]
async fn a_blocked_thread_can_be_cancelled() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "ask me", None).await;
    chat.wait_state(&id, "blocked").await;
    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;
    assert_eq!(world.plain.cancels().len(), 1);
}
