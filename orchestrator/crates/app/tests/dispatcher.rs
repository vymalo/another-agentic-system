//! Dispatcher behaviour against the scripted agent and the in-memory store.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::NewThread;
use orch_core::{AgentId, AgentTarget, EventBody, ThreadState};
use orch_ports::memory::Call;
use orch_ports::{AgentError, Clock, OutboxStatus, SystemClock, ThreadStore};
use support::*;

const FIVE: [&str; 5] = [
    "user_message",
    "agent_status:working",
    "artifact",
    "agent_status:completed",
    "thread_state:done",
];

#[tokio::test]
async fn echo_completes_with_the_expected_event_sequence() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), FIVE);
    assert_contiguous(&ev);
    match &ev[2].body {
        EventBody::Artifact(a) => {
            assert_eq!(a.name, "echo");
            assert_eq!(a.text.as_deref(), Some("echo: echo hi"));
            assert_eq!(
                a.uri.as_deref(),
                Some("https://github.com/acme/demo/pull/1")
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(ev[1].actor.name, "plain");
    assert_eq!(w.agent.sends().len(), 1);
    let open = w.store.list_open_outbox(t.id).await.unwrap();
    assert!(open.is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn blocked_then_follow_up_continues_the_same_task() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "ask which branch").await;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    let v = serde_json::to_value(&ev[2]).unwrap();
    assert_eq!(v["data"]["detail"], "Which branch?");

    let msg = app
        .post_message(&alice(), t.id, "main".into())
        .await
        .unwrap();
    assert_eq!(msg.seq, 5);
    // The follow-up re-queues the thread.
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked",
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&ev);
    let sends = w.agent.sends();
    assert_eq!(sends.len(), 2);
    match (&sends[0], &sends[1]) {
        (
            Call::Send { task_id: None, .. },
            Call::Send {
                task_id: Some(t2),
                reference_task_ids,
                text,
                context_id,
                ..
            },
        ) => {
            assert_eq!(t2, "task-1");
            assert_eq!(text, "main");
            assert_eq!(context_id, &t.id.to_string());
            // the answer continues the task: it is not a new task, so it references none
            assert!(reference_task_ids.is_empty());
        }
        other => panic!("{other:?}"),
    }
    run.shutdown().await;
}

#[tokio::test]
async fn transient_send_failures_are_retried_with_backoff() {
    let w = World::new();
    w.agent
        .fail_next_sends(2, || AgentError::unreachable("boom"));
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo retry").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(w.agent.sends().len(), 3);
    assert_eq!(shape(&events(&app, &alice(), t.id).await), FIVE);
    run.shutdown().await;
}

#[tokio::test]
async fn a_rate_limited_agent_is_retried_no_sooner_than_it_asked() {
    let w = World::new();
    // The backoff curve of `fast()` starts at 20 ms; the agent asks for 400 ms.
    w.agent.fail_next_sends(1, || AgentError::RateLimited {
        retry_after: Some(Duration::from_millis(400)),
    });
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let started = std::time::Instant::now();
    let t = create(&app, &alice(), "plain", "echo later").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "retried after {:?}",
        started.elapsed()
    );
    assert_eq!(w.agent.sends().len(), 2);
    run.shutdown().await;
}

#[tokio::test]
async fn transport_text_reaches_the_outbox_and_never_the_chat() {
    let w = World::new();
    w.agent.fail_next_sends(10, || {
        AgentError::unreachable("delivery failed").with_source(std::io::Error::other(
            "connect to http://10.0.0.7:9000/a2a?token=abc refused",
        ))
    });
    let app = w.app();
    let mut cfg = fast();
    cfg.max_attempts = 2;
    let run = spawn_dispatcher(&app, cfg, "d1");
    let t = create(&app, &alice(), "plain", "echo nobody").await;
    let row = w.store.list_open_outbox(t.id).await.unwrap()[0].id;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    let ev = events(&app, &alice(), t.id).await;
    let chat = serde_json::to_string(&ev).unwrap();
    assert!(chat.contains("the agent could not be reached"), "{chat}");
    assert!(
        !chat.contains("10.0.0.7") && !chat.contains("token=abc"),
        "{chat}"
    );
    let dead = eventually("the row is dead", || async {
        let r = w.store.get_outbox(row).await.unwrap().unwrap();
        (r.status == OutboxStatus::Dead).then_some(r)
    })
    .await;
    assert_eq!(
        dead.last_error.as_deref(),
        Some(
            "agent unreachable: delivery failed: connect to http://10.0.0.7:9000/a2a?token=abc refused"
        )
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_refuses_the_credentials_is_not_retried_and_the_chat_says_so_plainly() {
    let w = World::new();
    w.agent.fail_next_sends(10, || {
        AgentError::unauthenticated("HTTP 401 from https://plain.example.com")
    });
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo denied").await;
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    assert_eq!(w.agent.sends().len(), 1, "a refusal is permanent");
    let ev = events(&app, &alice(), t.id).await;
    match &ev[1].body {
        EventBody::Error(e) => {
            assert!(!e.retryable);
            assert_eq!(
                e.message,
                "the agent did not accept the orchestrator's credentials"
            );
            assert!(!e.message.contains("https://"), "{}", e.message);
        }
        other => panic!("{other:?}"),
    }
    run.shutdown().await;
}

#[tokio::test]
async fn exhausted_retries_dead_letter_into_blocked_and_a_follow_up_recovers() {
    let w = World::new();
    let app = w.app();
    let mut cfg = fast();
    cfg.max_attempts = 3;
    let run = spawn_dispatcher(&app, cfg, "d1");
    let t = create(&app, &alice(), "plain", "down please").await;
    let row = w.store.list_open_outbox(t.id).await.unwrap()[0].id;
    wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        ["user_message", "error", "thread_state:blocked"]
    );
    match &ev[1].body {
        EventBody::Error(e) => {
            assert!(e.retryable);
            // The chat gets fixed text for the class, never the transport's own words.
            assert_eq!(e.message, "the agent could not be reached");
        }
        other => panic!("{other:?}"),
    }
    // The operator gets the whole chain.
    let dead = w.store.get_outbox(row).await.unwrap().unwrap();
    assert_eq!(dead.status, OutboxStatus::Dead);
    let last_error = dead.last_error.unwrap();
    assert!(last_error.contains("scripted outage"), "{last_error}");
    assert_eq!(w.agent.sends().len(), 3);
    eventually("dead row", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        open.is_empty().then_some(())
    })
    .await;

    // The user tries again with something that works.
    app.post_message(&alice(), t.id, "echo again".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn permanent_rejection_fails_the_thread_without_retrying() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "fail now").await;
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), ["user_message", "error", "thread_state:failed"]);
    match &ev[1].body {
        EventBody::Error(e) => assert!(!e.retryable),
        other => panic!("{other:?}"),
    }
    assert_eq!(w.agent.sends().len(), 1);
    // A finished thread is not closed: the next message starts the next job (ADR 0020).
    app.post_message(&alice(), t.id, "echo more".into())
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev)[3..],
        [
            "user_message",
            "job_started",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_follow_up_after_done_is_a_new_task_in_the_same_context() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    app.post_message(&alice(), t.id, "echo again".into())
        .await
        .unwrap();
    eventually("the second job is done", || async {
        let ev = events(&app, &alice(), t.id).await;
        (shape(&ev)
            .iter()
            .filter(|s| *s == "thread_state:done")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev).iter().filter(|s| *s == "job_started").count(), 1);
    match &ev[6].body {
        EventBody::JobStarted(d) => assert_eq!(d.job, 2),
        other => panic!("{other:?}"),
    }
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    assert_eq!(record.job.number, 2);
    let sends = w.agent.sends();
    assert_eq!(sends.len(), 2);
    match (&sends[0], &sends[1]) {
        (
            Call::Send {
                task_id: None,
                reference_task_ids: first_refs,
                context_id: c1,
                ..
            },
            Call::Send {
                task_id: None,
                reference_task_ids,
                text,
                context_id: c2,
                ..
            },
        ) => {
            assert!(
                first_refs.is_empty(),
                "a thread's first task references nothing"
            );
            assert_eq!(text, "echo again");
            assert_eq!(c1, c2, "the same context");
            // a new task of the thread names the one before it (ADR 0021)
            assert_eq!(reference_task_ids, &["task-1".to_owned()]);
        }
        other => panic!("{other:?}"),
    }
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_queued_behind_the_one_that_ended_the_job_starts_the_next_job() {
    let w = World::new();
    let app = w.app();
    // Two delegations are queued before anything is sent: the second waits for the first, and
    // by then the thread is done. It used to be dropped ("thread already finished").
    let t = create(&app, &alice(), "plain", "echo one").await;
    app.post_message(&alice(), t.id, "echo two".into())
        .await
        .unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("both jobs are done", || async {
        let ev = events(&app, &alice(), t.id).await;
        (shape(&ev)
            .iter()
            .filter(|s| *s == "thread_state:done")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    let ev = events(&app, &alice(), t.id).await;
    assert_contiguous(&ev);
    let shape = shape(&ev);
    assert_eq!(shape.iter().filter(|s| *s == "job_started").count(), 1);
    assert!(!shape.contains(&"error".to_owned()), "{shape:?}");
    assert_eq!(w.agent.sends().len(), 2);
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    assert_eq!((record.state, record.job.number), (ThreadState::Done, 2));
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn a_stop_typed_during_an_earlier_job_does_not_stop_the_next() {
    let w = World::new();
    let app = w.app();
    // Job 1: a stop is asked, but the job ends by itself before the dispatcher serves it.
    let t = create(&app, &alice(), "plain", "echo one").await;
    app.cancel(&alice(), t.id).await.unwrap();
    app.apply(
        t.id,
        orch_core::Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: orch_core::AgentUpdate::Status {
                state: orch_core::AgentTaskState::Completed,
                detail: None,
            },
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    // Job 2 starts, then the dispatcher serves the old stop.
    app.post_message(&alice(), t.id, "gate two".into())
        .await
        .unwrap();
    let open = w.store.list_open_outbox(t.id).await.unwrap();
    assert!(open.iter().any(|r| matches!(
        r.payload,
        orch_ports::OutboxPayload::Cancel { job: Some(1) }
    )));
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the stop row is finished", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        (!open
            .iter()
            .any(|r| r.kind == orch_ports::OutboxKind::Cancel))
        .then_some(())
    })
    .await;
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. })),
        "the agent was never asked to stop"
    );
    assert_ne!(state_of(&w, t.id).await, ThreadState::Cancelled);
    run.shutdown().await;
}

#[tokio::test]
async fn cancel_before_the_message_reached_the_agent() {
    let w = World::new();
    w.agent
        .fail_next_sends(1, || AgentError::unreachable("first try fails"));
    let app = w.app();
    let mut cfg = fast();
    cfg.backoff_base = Duration::from_secs(3600);
    cfg.backoff_max = Duration::from_secs(3600);
    let run = spawn_dispatcher(&app, cfg, "d1");
    let t = create(&app, &alice(), "plain", "echo never delivered").await;
    // The first attempt fails and is scheduled an hour ahead.
    eventually("delegate backing off", || async {
        let open = w.store.list_open_outbox(t.id).await.unwrap();
        open.iter()
            .any(|r| r.status == OutboxStatus::Pending && r.attempts == 1)
            .then_some(())
    })
    .await;
    app.cancel(&alice(), t.id).await.unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Cancelled).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), ["user_message", "thread_state:cancelled"]);
    assert_eq!(w.agent.sends().len(), 1, "the message was never delivered");
    assert!(w.store.list_open_outbox(t.id).await.unwrap().is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn cancel_a_running_task_reaches_the_agent() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "slow job").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    app.cancel(&alice(), t.id).await.unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Cancelled).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_status:canceled",
            "thread_state:cancelled"
        ]
    );
    assert_contiguous(&ev);
    assert!(w.agent.calls().contains(&Call::Cancel {
        task_id: "task-1".into()
    }));
    // Cancelling a finished thread is a quiet no-op.
    app.cancel(&alice(), t.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(events(&app, &alice(), t.id).await.len(), 4);
    run.shutdown().await;
}

#[tokio::test]
async fn cancel_after_completion_is_a_quiet_noop() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate me").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    // Cancel racing a finished task: let the agent finish first, then cancel through the store
    // before the dispatcher observed the end.
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    app.cancel(&alice(), t.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(state_of(&w, t.id).await, ThreadState::Done);
    run.shutdown().await;
}

#[tokio::test]
async fn a_worker_that_lost_its_lease_stops_writing() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate lease").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let before = events(&app, &alice(), t.id).await.len();
    // Another replica takes the row over.
    let now = SystemClock.now();
    assert_eq!(w.store.release_leases("d1", now).await.unwrap(), 1);
    let stolen = w
        .store
        .claim_outbox("thief", SystemClock.now(), Duration::from_secs(60), 10)
        .await
        .unwrap();
    assert_eq!(stolen.len(), 1);
    // d1's heartbeat notices within a heartbeat interval or two.
    tokio::time::sleep(Duration::from_millis(500)).await;
    w.agent.release_gate();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        events(&app, &alice(), t.id).await.len(),
        before,
        "the old worker must not write"
    );
    assert_eq!(state_of(&w, t.id).await, ThreadState::Working);
    run.kill();
}

/// A dispatcher whose heartbeat never fires within the test, so that only the store's fence,
/// not a lost-lease notice from the heartbeat, can stop a worker whose row was claimed again.
fn no_heartbeat() -> orch_app::DispatcherConfig {
    orch_app::DispatcherConfig {
        heartbeat: Duration::from_secs(3600),
        ..fast()
    }
}

/// Another claimer takes the row over once its lease lapsed (by the clock it is handed), and
/// the old worker's agent then finishes: nothing the old worker learned may be written.
async fn a_late_result_is_dropped_after_a_reclaim_by(owner: &str, first_owner: &str) {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, no_heartbeat(), first_owner);
    let t = create(&app, &alice(), "plain", "gate fence").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let before = events(&app, &alice(), t.id).await;
    let row = w.store.list_open_outbox(t.id).await.unwrap().remove(0);
    assert_eq!(
        (row.attempts, row.lease_owner.as_deref()),
        (1, Some(first_owner))
    );
    // The lease is 400 ms; a claimer whose clock is a second ahead sees it lapsed.
    let claimed = w
        .store
        .claim_outbox(
            owner,
            SystemClock.now() + Duration::from_secs(1),
            Duration::from_secs(3600),
            10,
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 2);
    w.agent.release_gate();
    // Give the old worker time to receive the rest of the turn and try to write it.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&after),
        shape(&before),
        "the old worker must not write"
    );
    assert_eq!(state_of(&w, t.id).await, ThreadState::Working);
    let row = w.store.get_outbox(row.id).await.unwrap().unwrap();
    assert_eq!(
        row.status,
        OutboxStatus::Inflight,
        "and must not finish the row"
    );
    assert_eq!((row.attempts, row.lease_owner.as_deref()), (2, Some(owner)));
    assert!(row.sent_at.is_some());
    run.kill();
}

#[tokio::test]
async fn a_stale_worker_s_late_result_is_fenced() {
    a_late_result_is_dropped_after_a_reclaim_by("thief", "d1").await;
}

#[tokio::test]
async fn a_row_reclaimed_by_its_own_owner_fences_the_older_task() {
    // The owner name is per process, so a restarted or re-claiming instance is "d1" again:
    // only the attempt tells the two claims apart.
    a_late_result_is_dropped_after_a_reclaim_by("d1", "d1").await;
}

#[tokio::test]
async fn a_dropped_stream_is_resumed_with_resubscribe() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "drop connection").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    eventually("resubscribe", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(shape(&ev), FIVE);
    assert_eq!(w.agent.sends().len(), 1);
    run.shutdown().await;
}

#[tokio::test]
async fn polling_takes_over_when_resubscribe_is_unsupported() {
    let w = World::new();
    w.agent.set_resubscribe_supported(false);
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "drop connection").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    eventually("polling", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::GetTask { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(shape(&events(&app, &alice(), t.id).await), FIVE);
    assert_eq!(w.agent.sends().len(), 1);
    run.shutdown().await;
}

#[tokio::test]
async fn the_selected_release_is_sent_and_echoed_on_agent_events() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = app
        .create_thread(
            &alice(),
            NewThread {
                title: Some("release".into()),
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: Some("staging".into()),
                },
                text: "echo release".into(),
            },
        )
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    match &w.agent.sends()[0] {
        Call::Send { release, agent, .. } => {
            assert_eq!(release.as_deref(), Some("staging"));
            assert_eq!(agent.as_str(), "coder");
        }
        other => panic!("{other:?}"),
    }
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(ev[1].actor.revision.as_deref(), Some("rev-2"));
    assert_eq!(ev[0].actor.revision, None);
    run.shutdown().await;
}

#[tokio::test]
async fn graceful_shutdown_releases_leases_for_other_replicas() {
    let w = World::new();
    let app = w.app();
    let mut cfg = fast();
    cfg.lease = Duration::from_secs(60);
    let run = spawn_dispatcher(&app, cfg, "d1");
    let t = create(&app, &alice(), "plain", "gate shutdown").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    assert!(
        w.store
            .claim_outbox("d2", SystemClock.now(), Duration::from_secs(60), 10)
            .await
            .unwrap()
            .is_empty(),
        "the lease is still valid"
    );
    run.shutdown().await;
    let rows = w
        .store
        .claim_outbox("d2", SystemClock.now(), Duration::from_secs(60), 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "released on shutdown");
    assert!(rows[0].sent_at.is_some());
}

#[tokio::test]
async fn two_dispatchers_do_not_double_deliver() {
    let w = World::new();
    let app = w.app();
    let a = spawn_dispatcher(&app, fast(), "d1");
    let b = spawn_dispatcher(&Arc::clone(&app), fast(), "d2");
    let mut ids = Vec::new();
    for i in 0..6 {
        ids.push(
            create(&app, &alice(), "plain", &format!("echo {i}"))
                .await
                .id,
        );
    }
    for id in &ids {
        wait_state(&app, &alice(), *id, ThreadState::Done).await;
        assert_eq!(shape(&events(&app, &alice(), *id).await), FIVE);
    }
    assert_eq!(w.agent.sends().len(), 6);
    a.shutdown().await;
    b.shutdown().await;
}
