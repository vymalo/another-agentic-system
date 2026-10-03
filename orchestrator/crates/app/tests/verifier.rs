//! The verifier path of the dispatcher (ADR 0018) against the scripted agent and the in-memory
//! store: what a `verify` outbox row does, one case per way it can end.
//!
//! The worker is played by the test (`App::apply`, no dispatcher delivers its delegation), so
//! that the only row the dispatcher has is the verification. `reviewer` is a scripted agent that
//! answers every request as its [`VerdictScript`] says.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, AppError, GateLayer};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, CheckStatus, EventBody, EventKind, Hold, Input, Job,
    ThreadRecord, ThreadState, Timer, Verdict, verifier_context,
};
use orch_ports::memory::{Call, VerdictScript};
use orch_ports::{
    AgentEndpoint, AgentError, Clock as _, Lease, OutboxItem, OutboxKind, OutboxPayload,
    OutboxStatus, PortSet, SystemClock, ThreadStore,
};
use serde_json::json;
use support::*;

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn directory_with_reviewer() -> AgentDirectory {
    let entry = |id: &str, name: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
    };
    AgentDirectory::new(vec![
        entry("coder", "Coder"),
        entry("plain", "Plain"),
        entry("reviewer", "Reviewer"),
    ])
}

/// An application in which `plain` requires the verifier `reviewer`, with `attempts` attempts.
fn app_with(w: &World, attempts: u32) -> Arc<TestApp> {
    let gate = GateLayer::from_json(&json!({
        "require": ["verifier"], "verifier": "reviewer", "maxAttempts": attempts,
    }))
    .unwrap()
    .unwrap();
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory_with_reviewer().fixed_registry(),
            },
            directory_with_reviewer(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                target_gates: [(AgentId::new("plain"), gate)].into(),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn from_worker(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

/// A thread of `plain` whose worker has pushed and finished: `verifying`, with one `verify` row
/// pending. Nothing delivers the worker's own delegation.
async fn verifying(w: &World, app: &TestApp) -> ThreadRecord {
    verifying_with(w, app, None).await
}

/// [`verifying`], for a thread whose screen has sent `catalog` (ADR 0023) with the second
/// message of the job.
async fn verifying_with(
    w: &World,
    app: &TestApp,
    catalog: Option<orch_core::UiCatalogData>,
) -> ThreadRecord {
    let t = create(app, &alice(), "plain", "fix the login").await;
    if let Some(catalog) = catalog {
        app.submit(
            &alice(),
            t.id,
            Input::UserMessage {
                user: alice(),
                text: "and mind the style".into(),
                message_id: None,
                run_id: None,
                origin: orch_core::Origin::Agui,
                catalog: Some(catalog),
                mentions: Vec::new(),
            },
            None,
        )
        .await
        .unwrap();
    }
    w.store
        .skip_unsent_delegates(t.id, SystemClock.now())
        .await
        .unwrap();
    for update in [
        AgentUpdate::Artifact {
            name: "branch".into(),
            mime_type: None,
            uri: None,
            text: Some(
                json!({"repository": "https://github.com/acme/demo.git", "branch": "agent/fix",
                       "commit": SHA})
                .to_string(),
            ),
        },
        AgentUpdate::Message {
            message_id: "m1".into(),
            text: "I fixed the login.".into(),
            is_final: true,
            purpose: None,
        },
        AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: None,
        },
    ] {
        app.apply(t.id, from_worker(update), None, None, None)
            .await
            .unwrap();
    }
    let t = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(t.state, ThreadState::Verifying);
    t
}

async fn verify_row(w: &World, t: &ThreadRecord) -> OutboxItem {
    w.store
        .list_open_outbox(t.id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.kind == OutboxKind::Verify)
        .expect("a verify row")
}

async fn row_of(w: &World, row: &OutboxItem) -> OutboxItem {
    w.store.get_outbox(row.id).await.unwrap().unwrap()
}

fn sends_to(w: &World, agent: &str) -> Vec<Call> {
    w.agent
        .sends()
        .into_iter()
        .filter(|c| matches!(c, Call::Send { agent: a, .. } if a.as_str() == agent))
        .collect()
}

fn checks(events: &[orch_core::Event]) -> Vec<&orch_core::CheckResult> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::CheckResult(c) => Some(c),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_application_turns_a_verification_request_into_a_verify_row() {
    let w = World::new();
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    assert_eq!(row.status, OutboxStatus::Pending);
    assert_eq!((row.sent_at, row.task_id.clone()), (None, None));
    let OutboxPayload::Verify {
        attempt,
        verification,
        verifier,
        pushed,
        text,
    } = &row.payload
    else {
        panic!("{:?}", row.payload);
    };
    assert_eq!((*attempt, *verification), (1, 1));
    assert_eq!(verifier.as_str(), "reviewer");
    assert_eq!(
        (pushed.commit.as_str(), pushed.branch.as_str()),
        (SHA, "agent/fix")
    );
    assert!(
        text.contains(SHA) && text.contains("attempt 1 of 3"),
        "{text}"
    );
    assert!(
        text.contains("I fixed the login."),
        "the agent's summary: {text}"
    );
    // The deadline was armed in the same commit.
    let timers = w
        .store
        .find_inbox(
            orch_ports::TIMER_SOURCE,
            &format!("{}:verifier_deadline:1:1", t.id),
        )
        .await
        .unwrap();
    assert!(timers.is_some(), "the verifier's deadline is an inbox row");
    // A user cannot forge the verdict, or the failure.
    for forged in [
        Input::VerifierReported {
            attempt: 1,
            verification: 1,
            verdict: Verdict {
                passed: true,
                findings: vec![],
            },
        },
        Input::VerifierFailed {
            attempt: 1,
            verification: 1,
            reason: "down".into(),
        },
    ] {
        let err = app.submit(&alice(), t.id, forged, None).await.unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    }
}

#[tokio::test]
async fn a_passing_verdict_finishes_the_job_and_is_never_the_workers_update() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Pass);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;

    // The verifier was asked once, in its own context, with the row as the message id.
    let sends = sends_to(&w, "reviewer");
    assert_eq!(sends.len(), 1);
    let Call::Send {
        message_id,
        context_id,
        task_id,
        reference_task_ids,
        text,
        release,
        ..
    } = &sends[0]
    else {
        unreachable!()
    };
    assert_eq!(message_id, &row.id.to_string());
    assert_eq!(context_id, &verifier_context(t.id, 1, 1));
    assert_ne!(context_id, &t.id.to_string(), "not the worker's context");
    assert_eq!((task_id, release), (&None, &None));
    // never a reference to the author's tasks: the verifier is told nothing of them (ADR 0002)
    assert!(reference_task_ids.is_empty());
    assert!(text.contains(&format!("commit {SHA}")), "{text}");

    // Its task is the row's, not the thread's.
    let done = row_of(&w, &row).await;
    assert_eq!(done.status, OutboxStatus::Delivered);
    assert_eq!(done.task_id.as_deref(), Some("task-1"));
    assert!(done.sent_at.is_some());
    assert_eq!(
        w.store.get_binding(t.id).await.unwrap().unwrap().task_id,
        None
    );

    // The log holds the worker's events and the verdict as a check result; nothing of the
    // verifier's own statuses, and no second `completed`.
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "artifact",
            "agent_message",
            "agent_status:completed",
            "check_result",
            "check_result",
            "thread_state:done"
        ]
    );
    let cards = checks(&ev);
    assert_eq!(
        (cards[0].status, cards[1].status),
        (CheckStatus::Pending, CheckStatus::Passed)
    );
    assert!(ev.iter().all(|e| e.actor.name != "reviewer"));
    let job: Job = app.get_thread(&alice(), t.id).await.unwrap().job;
    assert_eq!(job.attempt, 1);
    run.shutdown().await;
}

#[tokio::test]
async fn the_verifier_is_told_nothing_of_the_authors_screen_and_gets_no_thread() {
    // ADR 0002, ADR 0023: the verifier works in a context of its own; the author's catalog and
    // the thread's tools are the author's.
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Pass);
    let app = app_with(&w, 3);
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    let catalog_json = json!({"catalogId": id, "components": {"Note": {
        "type": "object",
        "properties": {"component": {"const": "Note"}},
    }}});
    let t = verifying_with(
        &w,
        &app,
        Some(orch_core::UiCatalogData {
            catalog_id: id.into(),
            version: 1,
            digest: orch_core::catalog_digest(&catalog_json).unwrap(),
            catalog: catalog_json,
        }),
    )
    .await;
    assert!(
        t.job.catalog.current().is_some(),
        "the thread has a catalog to withhold"
    );
    verify_row(&w, &t).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let sends = sends_to(&w, "reviewer");
    assert_eq!(sends.len(), 1);
    let Call::Send {
        ui_catalog,
        thread_tools,
        ..
    } = &sends[0]
    else {
        unreachable!()
    };
    // the verifier is told nothing of the author's screen and gets no tools on the author's thread
    assert_eq!((ui_catalog, thread_tools), (&None, &None));
    run.shutdown().await;
}

#[tokio::test]
async fn the_verifier_is_not_told_the_conversation_a_fork_of_the_authors_thread_continues() {
    // ADR 0002, ADR 0029: the verifier works in a context of its own and is told nothing of the
    // author's conversation, however the author's thread began.
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Pass);
    let app = app_with(&w, 3);
    // a thread of two messages, the first of which the worker answered; the second is edited into
    // a fork, whose worker (played by the test) pushes and finishes
    let parent = verifying(&w, &app).await;
    app.post_message(&alice(), parent.id, "and mind the style".into())
        .await
        .unwrap();
    let second = events(&app, &alice(), parent.id)
        .await
        .iter()
        .rev()
        .find(|e| e.kind() == EventKind::UserMessage)
        .unwrap()
        .seq;
    let fork = app
        .fork_thread(
            &alice(),
            parent.id,
            orch_app::ForkRequest {
                at: orch_app::ForkAt::Replace {
                    seq: second,
                    text: "mind the style, please".into(),
                    message_id: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap()
        .thread;
    for thread in [parent.id, fork.id] {
        w.store
            .skip_unsent_delegates(thread, SystemClock.now())
            .await
            .unwrap();
    }
    for update in [
        AgentUpdate::Artifact {
            name: "branch".into(),
            mime_type: None,
            uri: None,
            text: Some(
                json!({"repository": "https://github.com/acme/demo.git", "branch": "agent/fix",
                       "commit": SHA})
                .to_string(),
            ),
        },
        AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: None,
        },
    ] {
        app.apply(fork.id, from_worker(update), None, None, None)
            .await
            .unwrap();
    }
    let fork = app.get_thread(&alice(), fork.id).await.unwrap();
    assert_eq!(fork.state, ThreadState::Verifying);
    assert!(fork.forked_from.is_some());
    verify_row(&w, &fork).await;

    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), fork.id, ThreadState::Done).await;
    let sends = sends_to(&w, "reviewer");
    assert_eq!(sends.len(), 1, "one verification, of the fork's commit");
    let Call::Send {
        history,
        text,
        context_id,
        ..
    } = &sends[0]
    else {
        unreachable!()
    };
    assert_eq!(history, &None);
    assert!(!text.contains("<<<conversation"), "{text}");
    assert_ne!(context_id, &fork.id.to_string(), "a context of its own");
    run.shutdown().await;
}

#[tokio::test]
async fn findings_fail_the_job_on_the_last_attempt_and_go_to_the_log() {
    let w = World::new();
    w.agent.set_verifier(
        "reviewer",
        VerdictScript::Fail(vec!["no test".into(), "no docs".into()]),
    );
    let app = app_with(&w, 1);
    let t = verifying(&w, &app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    let ev = events(&app, &alice(), t.id).await;
    let answer = checks(&ev).into_iter().last().unwrap();
    assert_eq!(answer.status, CheckStatus::Failed);
    assert_eq!(answer.findings, ["no test", "no docs"]);
    let error = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert!(
        !error.retryable && error.message.contains("no test"),
        "{error:?}"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn no_verdict_and_an_unusable_one_fail_the_check() {
    for (script, says) in [
        (
            VerdictScript::Silent,
            "no verdict: the verifier finished without reporting",
        ),
        (
            VerdictScript::Garbled,
            "cannot be used: `passed` is missing or not a boolean",
        ),
    ] {
        let w = World::new();
        w.agent.set_verifier("reviewer", script.clone());
        let app = app_with(&w, 1);
        let t = verifying(&w, &app).await;
        let run = spawn_dispatcher(&app, fast(), "d1");
        wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
        let ev = events(&app, &alice(), t.id).await;
        let finding = &checks(&ev).into_iter().last().unwrap().findings[0];
        assert!(finding.contains(says), "{script:?}: {finding}");
        run.shutdown().await;
    }
}

#[tokio::test]
async fn a_verifier_task_that_ends_without_an_answer_holds_the_thread() {
    for (script, says) in [
        (
            VerdictScript::Broken,
            "the verifier's task ended without an answer",
        ),
        (VerdictScript::Asks, "asked for input"),
    ] {
        let w = World::new();
        w.agent.set_verifier("reviewer", script.clone());
        let app = app_with(&w, 3);
        let t = verifying(&w, &app).await;
        let row = verify_row(&w, &t).await;
        let run = spawn_dispatcher(&app, fast(), "d1");
        let held = wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
        assert_eq!(held.job.hold, Some(Hold::VerifierFailed), "{script:?}");
        assert_eq!(held.job.attempt, 1, "{script:?}: no attempt is spent");
        let ev = events(&app, &alice(), t.id).await;
        let error = ev
            .iter()
            .find_map(|e| match &e.body {
                EventBody::Error(d) => Some(d),
                _ => None,
            })
            .unwrap();
        assert!(
            error.retryable && error.message.contains(says),
            "{script:?}: {error:?}"
        );
        assert!(
            checks(&ev).iter().all(|c| c.status == CheckStatus::Pending),
            "no verdict was recorded"
        );
        assert_eq!(row_of(&w, &row).await.status, OutboxStatus::Dead);
        if script == VerdictScript::Asks {
            assert!(
                w.agent
                    .calls()
                    .iter()
                    .any(|c| matches!(c, Call::Cancel { task_id } if task_id == "task-1")),
                "a verifier that waits for input is let go, by its task"
            );
        }
        run.shutdown().await;
    }
}

#[tokio::test]
async fn an_unreachable_verifier_is_retried_then_holds_the_thread() {
    let w = World::new();
    w.agent.set_unreachable("reviewer");
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let held = wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    assert_eq!(held.job.hold, Some(Hold::VerifierFailed));
    // The first claim sent, and could not tell whether the verifier got it. Every later one
    // looks first, and a lookup that cannot be answered is no reason to ask again.
    assert_eq!(sends_to(&w, "reviewer").len(), 1, "asked once");
    let dead = row_of(&w, &row).await;
    assert_eq!(dead.attempts, 5, "as many tries as a delegation gets");
    assert!(finds(&w) >= 4, "each retry looked first");
    assert_eq!(dead.status, OutboxStatus::Dead);
    assert!(
        dead.last_error.as_deref().unwrap().contains("unreachable"),
        "{dead:?}"
    );
    let ev = events(&app, &alice(), t.id).await;
    let message = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.message.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        message, "the verifier could not be used: the agent could not be reached",
        "no transport detail reaches the chat"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_refused_request_is_not_retried() {
    let w = World::new();
    w.agent.fail_next_sends(1, || {
        AgentError::Rejected("the repository is not one I can read".into())
    });
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let held = wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    assert_eq!(held.job.hold, Some(Hold::VerifierFailed));
    assert_eq!(sends_to(&w, "reviewer").len(), 1);
    let ev = events(&app, &alice(), t.id).await;
    assert!(ev.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.message.contains("the repository is not one I can read")
    )));
    run.shutdown().await;
}

#[tokio::test]
async fn a_verifier_that_is_not_configured_any_more_holds_the_thread() {
    let w = World::new();
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    // Another process, configured without the verifier, serves the row.
    let lean = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory().fixed_registry(),
            },
            directory(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .unwrap(),
    );
    let run = spawn_dispatcher(&lean, fast(), "d1");
    let held = wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    assert_eq!(held.job.hold, Some(Hold::VerifierFailed));
    assert!(sends_to(&w, "reviewer").is_empty());
    let ev = events(&app, &alice(), t.id).await;
    assert!(ev.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.message.contains("no longer configured")
    )));
    run.shutdown().await;
}

#[tokio::test]
async fn a_verification_that_is_over_is_dropped_without_asking() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Pass);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    // The user wrote before the row was served: the verification is abandoned.
    app.post_message(&alice(), t.id, "wait, also the logout".into())
        .await
        .unwrap();
    w.store
        .skip_unsent_delegates(t.id, SystemClock.now())
        .await
        .unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the row is dropped", || async {
        (row_of(&w, &row).await.status == OutboxStatus::Skipped).then_some(())
    })
    .await;
    assert!(sends_to(&w, "reviewer").is_empty(), "nobody was asked");
    assert_eq!(state_of(&w, t.id).await, ThreadState::Queued);
    let ev = events(&app, &alice(), t.id).await;
    assert!(
        checks(&ev).iter().all(|c| c.status == CheckStatus::Pending),
        "and nothing was decided"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_verifier_that_hangs_is_told_to_stop_when_the_deadline_holds_the_thread() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Hang);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the verifier has the request", || async {
        (row_of(&w, &row).await.task_id.is_some()).then_some(())
    })
    .await;
    // The deadline the core armed comes due (the inbox worker would feed it in).
    app.apply(
        t.id,
        Input::TimerFired(Timer::VerifierDeadline {
            attempt: 1,
            verification: 1,
        }),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let held = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(held.state, ThreadState::Blocked);
    assert_eq!(held.job.hold, Some(Hold::VerifierTimeout));
    eventually("the row is dropped", || async {
        (row_of(&w, &row).await.status == OutboxStatus::Skipped).then_some(())
    })
    .await;
    assert!(
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { task_id } if task_id == "task-1")),
        "the verifier was told to stop"
    );
    assert_eq!(
        state_of(&w, t.id).await,
        ThreadState::Blocked,
        "and the thread stays held"
    );
    run.shutdown().await;
}

/// The worker that asked the verifier loses its claim (its lease lapsed and another claimer took
/// the row), and the verifier answers after that: nothing it learned may be written.
#[tokio::test]
async fn a_worker_that_lost_its_claim_writes_nothing() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Gated);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let run = spawn_dispatcher(&app, no_heartbeat(), "d1");
    eventually("the request is sent", || async {
        (row_of(&w, &row).await.task_id.is_some()).then_some(())
    })
    .await;
    let before = events(&app, &alice(), t.id).await;
    let claimed = w
        .store
        .claim_outbox(
            "thief",
            SystemClock.now() + Duration::from_secs(1),
            Duration::from_secs(3600),
            10,
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1, "the lapsed verify row");
    assert_eq!(claimed[0].attempts, 2);
    w.agent.release_gate();
    // The old worker gets the verdict and tries to write it; the store refuses.
    eventually("the old worker's write is refused", || async {
        (w.store.fenced_commits() > 0).then_some(())
    })
    .await;
    assert_eq!(
        shape(&events(&app, &alice(), t.id).await),
        shape(&before),
        "the old worker must not write the verdict"
    );
    assert_eq!(state_of(&w, t.id).await, ThreadState::Verifying);
    let held = row_of(&w, &row).await;
    assert_eq!(
        (held.status, held.lease_owner.as_deref()),
        (OutboxStatus::Inflight, Some("thief"))
    );
    // The thief's own write is what counts: it reads the task the first claim recorded.
    let lease = Lease {
        id: row.id,
        owner: "thief".into(),
        attempt: 2,
    };
    assert_eq!(held.task_id.as_deref(), Some("task-1"));
    let applied = app
        .apply(
            t.id,
            Input::VerifierReported {
                attempt: 1,
                verification: 1,
                verdict: Verdict {
                    passed: true,
                    findings: vec![],
                },
            },
            Some(format!("verdict:{}", row.id)),
            None,
            Some(&lease),
        )
        .await
        .unwrap();
    assert!(matches!(applied, orch_app::ApplyOutcome::Applied { .. }));
    run.kill();
}

fn no_heartbeat() -> orch_app::DispatcherConfig {
    orch_app::DispatcherConfig {
        heartbeat: Duration::from_secs(3600),
        ..fast()
    }
}

/// The process that sent the request dies. The next claimant finds the task on the row and
/// follows it: the verifier is asked once, and there is one verdict.
#[tokio::test]
async fn a_reclaimed_row_follows_the_verifiers_task_instead_of_asking_again() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Gated);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let first = spawn_dispatcher(&app, fast(), "d1");
    eventually("the request is sent", || async {
        (row_of(&w, &row).await.task_id.is_some()).then_some(())
    })
    .await;
    first.kill();
    let second = spawn_dispatcher(&app, fast(), "d2");
    eventually("the second worker re-attaches", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(sends_to(&w, "reviewer").len(), 1, "asked once");
    let ev = events(&app, &alice(), t.id).await;
    let cards = checks(&ev);
    assert_eq!(cards.len(), 2, "pending, then one verdict");
    assert!(cards.iter().all(|c| !c.stale));
    assert_eq!(
        ev.iter()
            .filter(|e| e.kind() == EventKind::CheckResult)
            .count(),
        2
    );
    second.shutdown().await;
}

/// A crash between sending and recording: the row has no task, and the next claimant looks the
/// message up by its id instead of sending it again.
#[tokio::test]
async fn a_request_that_reached_the_verifier_before_the_crash_is_found_not_resent() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Gated);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    // Play the first claimant: it sends the request and dies before recording anything.
    let claimed = w
        .store
        .claim_outbox("d1", SystemClock.now(), Duration::from_millis(50), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let stream = orch_ports::AgentClient::send_stream(
        &w.agent,
        orch_ports::SendRequest {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("reviewer"),
                "https://x.example.com/card",
                None,
            ),
            message_id: row.id.to_string(),
            context_id: verifier_context(t.id, 1, 1),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: orch_ports::SendContent::Text("review".into()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
        },
    )
    .await
    .unwrap();
    drop(stream);
    assert_eq!(row_of(&w, &row).await.task_id, None);

    let run = spawn_dispatcher(&app, fast(), "d2");
    eventually("the request is found", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Find { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        sends_to(&w, "reviewer").len(),
        1,
        "never sent a second time"
    );
    assert_eq!(row_of(&w, &row).await.task_id.as_deref(), Some("task-1"));
    run.shutdown().await;
}

/// A verdict streamed before the claimant died is not in the stream the next claimant
/// resubscribes to (it gets the rest of the task). The task holds it, and it is read from there
/// before "no verdict" is concluded: a false one would cost the agent an attempt.
#[tokio::test]
async fn a_verdict_streamed_before_a_crash_is_read_from_the_task_not_lost() {
    let w = World::new();
    w.agent
        .set_verifier("reviewer", VerdictScript::VerdictThenGate);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    let first = spawn_dispatcher(&app, fast(), "d1");
    eventually("the request is sent", || async {
        (row_of(&w, &row).await.task_id.is_some()).then_some(())
    })
    .await;
    // The verdict is on the task (the script says it before the gate); the claimant dies with
    // the task not yet complete.
    first.kill();
    let second = spawn_dispatcher(&app, fast(), "d2");
    eventually("the second worker re-attaches", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(sends_to(&w, "reviewer").len(), 1, "asked once");
    assert!(
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::GetTask { .. })),
        "the verdict was read from the task"
    );
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        checks(&ev).iter().map(|c| c.status).collect::<Vec<_>>(),
        [CheckStatus::Pending, CheckStatus::Passed],
        "one verdict, and it is the one the verifier gave"
    );
    second.shutdown().await;
}

/// Plays a first claimant that sent the request and died before recording anything: the message
/// reached the verifier (`task-1`), the row knows nothing.
async fn sent_and_forgotten(w: &World, t: &ThreadRecord, row: &OutboxItem) {
    let claimed = w
        .store
        .claim_outbox("d1", SystemClock.now(), Duration::from_millis(50), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let stream = orch_ports::AgentClient::send_stream(
        &w.agent,
        orch_ports::SendRequest {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("reviewer"),
                "https://x.example.com/card",
                None,
            ),
            message_id: row.id.to_string(),
            context_id: verifier_context(t.id, 1, 1),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: orch_ports::SendContent::Text("review".into()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
        },
    )
    .await
    .unwrap();
    drop(stream);
    assert_eq!(row_of(w, row).await.task_id, None);
}

fn finds(w: &World) -> usize {
    w.agent
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Find { .. }))
        .count()
}

/// A lookup that fails does not say the verifier was not asked: it is tried again, and the
/// request is not sent a second time because of it.
#[tokio::test]
async fn a_lookup_that_fails_is_tried_again_and_does_not_cost_a_second_request() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Gated);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    sent_and_forgotten(&w, &t, &row).await;
    w.agent.fail_next_finds(2);

    let run = spawn_dispatcher(&app, fast(), "d2");
    eventually("the lookup got through", || async {
        (row_of(&w, &row).await.task_id.is_some()).then_some(())
    })
    .await;
    assert_eq!(finds(&w), 3, "two failures, then the answer");
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(
        sends_to(&w, "reviewer").len(),
        1,
        "never sent a second time"
    );
    assert_eq!(row_of(&w, &row).await.task_id.as_deref(), Some("task-1"));
    run.shutdown().await;
}

/// A lookup that keeps failing is not "no such task": the request is never sent again, the row
/// is retried, and in the end the thread waits for the user.
#[tokio::test]
async fn a_lookup_that_never_answers_holds_the_thread_instead_of_asking_twice() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Pass);
    let app = app_with(&w, 3);
    let t = verifying(&w, &app).await;
    let row = verify_row(&w, &t).await;
    sent_and_forgotten(&w, &t, &row).await;
    w.agent.fail_next_finds(usize::MAX);

    let run = spawn_dispatcher(&app, fast(), "d2");
    let held = wait_state(&app, &alice(), t.id, ThreadState::Blocked).await;
    assert_eq!(held.job.hold, Some(Hold::VerifierFailed));
    assert_eq!(held.job.attempt, 1, "no attempt is spent");
    assert_eq!(row_of(&w, &row).await.status, OutboxStatus::Dead);
    assert!(finds(&w) > 3, "the lookup was retried, then the row was");
    assert_eq!(
        sends_to(&w, "reviewer").len(),
        1,
        "the only request is the one the crashed claimant sent"
    );
    run.shutdown().await;
}

/// The checks the verifier's stream feeds are read here and nowhere else: its artifacts other
/// than the verdict, and its messages, never reach the log.
#[tokio::test]
async fn what_the_verifier_says_besides_its_verdict_is_not_logged() {
    let w = World::new();
    w.agent.set_verifier("reviewer", VerdictScript::Silent);
    let app = app_with(&w, 1);
    let t = verifying(&w, &app).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    let ev = events(&app, &alice(), t.id).await;
    assert!(
        ev.iter().all(|e| e.kind() != EventKind::Artifact
            || matches!(&e.body, EventBody::Artifact(a) if a.name == "branch")),
        "the verifier's `notes` artifact is not the worker's"
    );
    assert_eq!(
        ev.iter()
            .filter(|e| e.kind() == EventKind::AgentStatus)
            .count(),
        1,
        "only the worker's completion"
    );
    run.shutdown().await;
}
