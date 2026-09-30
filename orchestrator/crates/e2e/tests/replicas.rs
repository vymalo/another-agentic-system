//! Several orchestrator replicas on one database: the one that serves a request need not be the
//! one that dispatches the work. Needs Postgres (separate pools and listeners, like separate
//! processes); a no-op without `ORCH_TEST_DATABASE_URL`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_testsupport::CallKind;

const WAIT: Duration = Duration::from_secs(20);

/// `orch-1` only serves the API (the viewer's connect stream lives there), `orch-2` only dispatches.
async fn a_stream_on_one_replica_sees_events_dispatched_by_another(backend: Backend) {
    let world = World::start(backend).await;
    let api_only = world.instance_with("orch-1", false).await;
    let dispatching = world.instance_with("orch-2", true).await;
    let chat = world.chat(&api_only);

    let id = chat.create_thread("plain", "gate across", None).await;
    let mut sse = chat.agui_connect(&id, None, false).await;
    assert_eq!(sse.status, 200);
    // The stream lives on orch-1, whose dispatcher does not run: the running state can only
    // have been written by orch-2 and reach orch-1 through the database.
    chat.wait_state(&id, "working").await;
    eventually("orch-2 delivered the message", || async {
        (world.plain.executions().len() == 1).then_some(())
    })
    .await;
    world.plain.release_gate();

    let frames = sse
        .frames_until(WAIT, |f| f.event["type"] == "RUN_FINISHED")
        .await;
    let seqs: Vec<i64> = frames.iter().filter_map(|f| f.id).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        serde_json::json!({"type": "success"})
    );
    let events = chat.events(&id).await;
    assert_eq!(shape(&events), FIVE);
    assert_contiguous(&events);
    assert_eq!(world.plain.executions().len(), 1);
    drop(dispatching);
}

/// The cancel request lands on a replica that runs no dispatcher; the replica that holds the
/// delegation sends `CancelTask`.
async fn cancel_through_another_replica_reaches_the_agent(backend: Backend) {
    let world = World::start(backend).await;
    let api_only = world.instance_with("orch-1", false).await;
    let dispatching = world.instance_with("orch-2", true).await;
    let chat = world.chat(&api_only);

    let id = chat.create_thread("plain", "slow across", None).await;
    chat.wait_state(&id, "working").await;
    eventually("orch-2 runs the task", || async {
        (world.plain.executions().len() == 1).then_some(())
    })
    .await;

    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;

    let cancels = world.plain.cancels();
    assert_eq!(cancels.len(), 1, "exactly one CancelTask reached the agent");
    assert_eq!(cancels[0].kind, CallKind::Cancel);
    assert_eq!(cancels[0].task_id, world.plain.executions()[0].task_id);
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
    drop(dispatching);
}

postgres_only!(
    a_stream_on_one_replica_sees_events_dispatched_by_another,
    cancel_through_another_replica_reaches_the_agent,
);
