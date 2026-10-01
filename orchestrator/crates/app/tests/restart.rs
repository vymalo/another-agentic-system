//! Restart safety at the application level: two app instances (and dispatchers) over one
//! shared in-memory "database", the first killed mid-stream.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_core::ThreadState;
use orch_ports::memory::Call;
use support::*;

const FIVE: [&str; 5] = [
    "user_message",
    "agent_status:working",
    "artifact",
    "agent_status:completed",
    "thread_state:done",
];

async fn crash_scenario(release_before_restart: bool, resubscribe: bool) {
    let w = World::new();
    w.agent.set_resubscribe_supported(resubscribe);
    let app1 = w.app();
    let run1 = spawn_dispatcher(&app1, fast(), "instance-1");
    let t = create(&app1, &alice(), "plain", "gate crash").await;
    wait_state(&app1, &alice(), t.id, ThreadState::Working).await;

    // The process dies mid-stream: every worker of instance 1 is gone, the lease is not released.
    run1.kill();
    if release_before_restart {
        w.agent.release_gate();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A new instance on the same database takes over once the lease expires.
    let app2 = w.app();
    let run2 = spawn_dispatcher(&app2, fast(), "instance-2");
    if !release_before_restart {
        eventually("instance 2 resumes the task", || async {
            w.agent
                .calls()
                .iter()
                .any(|c| matches!(c, Call::Resubscribe { .. } | Call::GetTask { .. }))
                .then_some(())
        })
        .await;
        w.agent.release_gate();
    }
    wait_state(&app2, &alice(), t.id, ThreadState::Done).await;

    let ev = events(&app2, &alice(), t.id).await;
    assert_eq!(shape(&ev), FIVE, "each update exactly once");
    assert_contiguous(&ev);
    assert_eq!(
        w.agent.sends().len(),
        1,
        "the message must be sent to the agent exactly once"
    );
    run2.shutdown().await;
}

#[tokio::test]
async fn kill_mid_stream_then_finish_before_restart_polls_the_result() {
    crash_scenario(true, true).await;
}

#[tokio::test]
async fn kill_mid_stream_then_resubscribe_finishes_live() {
    crash_scenario(false, true).await;
}

#[tokio::test]
async fn kill_mid_stream_without_resubscribe_falls_back_to_polling() {
    crash_scenario(false, false).await;
}

#[tokio::test]
async fn a_crash_between_send_and_recording_is_recovered_by_message_id() {
    // Simulate: the agent got the message (task exists) but the orchestrator died before
    // `mark_sent`. The re-claiming worker must find the task by message id, not send again.
    let w = World::new();
    let app1 = w.app();
    let t = create(&app1, &alice(), "plain", "gate lost ack").await;
    // Claim as instance 1 and send by hand, then never record it.
    let rows = orch_ports::ThreadStore::claim_outbox(
        &w.store,
        "instance-1",
        orch_ports::Clock::now(&orch_ports::SystemClock),
        Duration::from_millis(50),
        10,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    let binding = orch_ports::ThreadStore::get_binding(&w.store, t.id)
        .await
        .unwrap()
        .unwrap();
    let _stream = orch_ports::AgentClient::send_stream(
        &w.agent,
        orch_ports::SendRequest {
            endpoint: app1
                .directory()
                .get(&binding.agent_id)
                .unwrap()
                .endpoint
                .clone(),
            message_id: rows[0].id.to_string(),
            context_id: binding.context_id.clone(),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: orch_ports::SendContent::Text("gate lost ack".into()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
        },
    )
    .await
    .unwrap();

    let app2 = w.app();
    let run2 = spawn_dispatcher(&app2, fast(), "instance-2");
    eventually("recovery lookup", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Find { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app2, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(w.agent.sends().len(), 1, "no second send");
    let ev = events(&app2, &alice(), t.id).await;
    assert_contiguous(&ev);
    assert_eq!(
        ev.last().map(|e| e.kind()),
        Some(orch_core::EventKind::ThreadState)
    );
    run2.shutdown().await;
}
