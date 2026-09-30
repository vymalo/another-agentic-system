//! The inbox end to end, on both stores: a deadline scheduled by a gated completion fires and
//! blocks the thread; a row claimed by a process that died is applied once by the next; a
//! report that arrives before its watch waits for it. The real webhook surface is a later
//! slice, so the test plays it through `App::receive`, and the agent through `App::apply`
//! where the fake agent would only get in the way.
//!
//! Nothing here sleeps to synchronise: each wait is `eventually` on what the store says, with a
//! deadline, and the timings (a deadline of 300 ms, a lease of 800 ms, a poll of 50 ms) only
//! bound how long a passing run takes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use jiff::SignedDuration;
use orch_app::GateRules;
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, CheckSource, CiConclusion, CiPolicy, CiProvider,
    CiReport, EventBody, EventKind, GatePolicy, Hold, Input, ThreadState,
};
use orch_ports::{InboxStatus, Received, TIMER_SOURCE};
use orch_testsupport::fake::{VERIFY_REPOSITORY, verify_commit};
use serde_json::json;

fn ci_gate(timeout: Duration) -> GatePolicy {
    GatePolicy {
        ci: CiPolicy {
            timeout: SignedDuration::try_from(timeout).unwrap(),
            ..CiPolicy::default()
        },
        ..GatePolicy::requiring([CheckSource::Ci])
    }
}

fn setup(timeout: Duration) -> Setup {
    Setup {
        gate: ci_gate(timeout),
        // This build refuses a CI gate until the CI webhook exists (slice 6); the inbox is tested
        // underneath it.
        gate_rules: GateRules::default().honouring(CheckSource::ALL),
        ..Setup::default()
    }
}

fn report() -> CiReport {
    CiReport {
        provider: CiProvider::Github,
        repository: "github.com/acme/demo".to_owned(),
        sha: verify_commit(1),
        branch: Some("agent/demo".to_owned()),
        name: "build".to_owned(),
        conclusion: CiConclusion::Success,
        url: None,
        summary: None,
    }
}

fn watch_key() -> String {
    format!("ci:github.com/acme/demo@{}", verify_commit(1))
}

fn from_agent(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

fn pushed() -> Input {
    from_agent(AgentUpdate::Artifact {
        name: "branch".to_owned(),
        mime_type: None,
        uri: None,
        text: Some(
            json!({"repository": VERIFY_REPOSITORY, "branch": "agent/fix", "commit": verify_commit(1)})
                .to_string(),
        ),
    })
}

fn completed() -> Input {
    from_agent(AgentUpdate::Status {
        state: AgentTaskState::Completed,
        detail: None,
    })
}

fn states(events: &[orch_core::Event]) -> Vec<ThreadState> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ThreadState(d) => Some(d.state),
            _ => None,
        })
        .collect()
}

fn ci_cards(events: &[orch_core::Event]) -> usize {
    events
        .iter()
        .filter(|e| e.kind() == EventKind::CiResult)
        .count()
}

/// A gated completion arms a deadline; nobody reports CI; the deadline comes due, the inbox
/// worker applies it and the thread blocks (and no attempt is spent).
async fn a_deadline_scheduled_by_a_gated_completion_fires_and_blocks_the_thread(backend: Backend) {
    let world = World::with(backend, setup(Duration::from_millis(300))).await;
    let instance = world.instance("orch-1").await;
    let node = world.node("inbox-1").await;
    let inbox = node.spawn_inbox(fast_inbox(), "inbox-1");
    let chat = world.chat(&instance);

    // The fake agent pushes a branch and completes: the gate wants CI.
    let id = chat
        .create_thread("plain", "verify-ci a feature", None)
        .await;
    chat.wait_state(&id, "blocked").await;

    let thread = node.thread(id.parse().unwrap()).await;
    assert_eq!(thread.job.hold, Some(Hold::CiTimeout));
    assert_eq!(thread.job.attempt, 1, "a timeout spends no attempt");
    let events = node.events(thread.id).await;
    assert_eq!(states(&events).last(), Some(&ThreadState::Blocked));
    assert!(
        events.iter().any(|e| e.kind() == EventKind::CheckResult),
        "the verification began: CI was pending"
    );
    assert!(events.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.message.contains("CI did not report in time")
    )));
    assert_eq!(
        node.watch(&watch_key()).await,
        Some(thread.id),
        "the push started the watch"
    );
    let row = node
        .inbox_row(TIMER_SOURCE, &format!("{}:ci_deadline:1:1", thread.id))
        .await
        .expect("the deadline was an inbox row");
    assert_eq!(row.status, InboxStatus::Applied);
    assert_eq!(row.attempts, 1);
    inbox.shutdown().await;
}

/// The process that claimed a report dies before it commits. Another process takes the row over
/// when the lease lapses and applies it once; the first, waking up late, is refused.
async fn a_row_claimed_by_a_dead_process_is_applied_once_by_the_next(backend: Backend) {
    let world = World::with(backend, setup(Duration::from_secs(3600))).await;
    let first = world.node("inbox-1").await;
    let second = world.node("inbox-2").await;
    let thread = first.create_thread("plain", "ship it").await;
    first.apply(thread.id, pushed()).await;
    first.apply(thread.id, completed()).await;
    assert_eq!(first.thread(thread.id).await.state, ThreadState::Verifying);

    let received = first.receive("github", "delivery-1", report()).await;
    let Received::Stored { id } = received else {
        panic!("{received:?}");
    };
    // Process 1 claims the row (nothing else is due: the deadline is an hour away) and dies.
    let stale = first
        .claim_and_die("inbox-1", Duration::from_millis(800))
        .await;
    assert_eq!(
        first.thread(thread.id).await.state,
        ThreadState::Verifying,
        "nothing was applied"
    );

    let inbox = second.spawn_inbox(fast_inbox(), "inbox-2");
    eventually(
        "process 2 takes the row over and finishes the thread",
        || async { (second.thread(thread.id).await.state == ThreadState::Done).then_some(()) },
    )
    .await;
    let events = second.events(thread.id).await;
    assert_eq!(ci_cards(&events), 1, "applied once");
    let row = second.inbox_row("github", "delivery-1").await.unwrap();
    assert_eq!(
        (row.id, row.status, row.attempts),
        (id, InboxStatus::Applied, 2)
    );

    // Process 1 was only paused: what it does now is fenced and writes nothing.
    let late = second
        .apply_from_inbox_late(
            thread.id,
            Input::CiReported(CiReport {
                conclusion: CiConclusion::Failure,
                ..report()
            }),
            &stale,
        )
        .await;
    assert_eq!(late, orch_app::ApplyOutcome::Fenced);
    let after = second.events(thread.id).await;
    assert_eq!(after.len(), events.len());
    assert_eq!(second.thread(thread.id).await.state, ThreadState::Done);
    inbox.shutdown().await;
}

/// CI is faster than the agent: its report arrives before any thread watches the commit. It
/// waits, parked; the commit that starts the watch wakes it and it is applied.
async fn a_report_received_before_its_watch_is_applied_after_a_later_commit(backend: Backend) {
    let world = World::with(backend, setup(Duration::from_secs(3600))).await;
    let node = world.node("inbox-1").await;
    let inbox = node.spawn_inbox(fast_inbox(), "inbox-1");
    let thread = node.create_thread("plain", "ship it").await;

    let received = node.receive("github", "delivery-1", report()).await;
    assert!(matches!(received, Received::Stored { .. }), "{received:?}");
    eventually("the report is parked", || async {
        let row = node.inbox_row("github", "delivery-1").await?;
        (row.status == InboxStatus::Parked).then_some(())
    })
    .await;
    assert_eq!(
        node.receive("github", "delivery-1", report()).await,
        Received::Duplicate,
        "the redelivery of a parked report is a duplicate"
    );

    // The agent pushes (the watch, and with it the re-arm) and completes (verification starts).
    node.apply(thread.id, pushed()).await;
    node.apply(thread.id, completed()).await;
    eventually(
        "the parked report is applied and the thread is done",
        || async { (node.thread(thread.id).await.state == ThreadState::Done).then_some(()) },
    )
    .await;
    let events = node.events(thread.id).await;
    assert_eq!(ci_cards(&events), 1);
    let row = node.inbox_row("github", "delivery-1").await.unwrap();
    assert_eq!(row.status, InboxStatus::Applied);
    assert_eq!(
        node.receive("github", "delivery-1", report()).await,
        Received::Duplicate
    );
    inbox.shutdown().await;
}

backends!(
    a_deadline_scheduled_by_a_gated_completion_fires_and_blocks_the_thread,
    a_row_claimed_by_a_dead_process_is_applied_once_by_the_next,
    a_report_received_before_its_watch_is_applied_after_a_later_commit,
);
