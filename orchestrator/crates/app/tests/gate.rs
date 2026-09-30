//! The verification gate through the thread service: the job is created with the thread, saved
//! with every change of state and survives a change of configuration. No dispatcher runs: the
//! test plays the agent by applying its updates.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_app::{AppConfig, ApplyOutcome, GateRules};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, CheckSource, CiConclusion, CiProvider, CiReport,
    EventBody, GatePolicy, Input, Job, ThreadId, ThreadState,
};
use orch_ports::{OutboxPayload, ThreadStore};
use serde_json::json;
use support::*;

const SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";

fn gated(sources: &[CheckSource]) -> AppConfig {
    AppConfig {
        stream_poll: Duration::from_millis(100),
        gate: GatePolicy::requiring(sources.iter().copied()),
        // The core decides CI and the verifier already; these tests play the application that
        // will honour them.
        gate_rules: GateRules::default().honouring(CheckSource::ALL),
        ..AppConfig::default()
    }
}

fn from_agent(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

fn artifact(name: &str, data: serde_json::Value) -> Input {
    from_agent(AgentUpdate::Artifact {
        name: name.to_owned(),
        mime_type: None,
        uri: None,
        text: Some(data.to_string()),
    })
}

fn branch() -> Input {
    artifact(
        "branch",
        json!({"repository": "https://github.com/o/r.git", "branch": "agent/x", "commit": SHA}),
    )
}

fn checks(passed: bool, findings: &[&str]) -> Input {
    artifact(
        "checks",
        json!({"passed": passed, "commit": SHA, "findings": findings}),
    )
}

fn completed() -> Input {
    from_agent(AgentUpdate::Status {
        state: AgentTaskState::Completed,
        detail: None,
    })
}

async fn apply(app: &TestApp, id: ThreadId, input: Input) {
    let outcome = app.apply(id, input, None, None, None).await.unwrap();
    assert!(
        matches!(outcome, ApplyOutcome::Applied { .. }),
        "{outcome:?}"
    );
}

async fn delegated_texts(w: &World, id: ThreadId) -> Vec<String> {
    w.store
        .list_open_outbox(id)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|row| match row.payload {
            OutboxPayload::Delegate { text, .. } => Some(text),
            OutboxPayload::Action { .. } | OutboxPayload::Cancel => None,
        })
        .collect()
}

#[tokio::test]
async fn the_default_gate_requires_nothing_and_the_job_stays_the_default() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "go").await;
    assert_eq!(t.job, Job::default());
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(done.state, ThreadState::Done);
    assert_eq!(done.job, Job::default());
}

#[tokio::test]
async fn a_failed_check_reworks_and_the_job_follows_the_thread() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::AgentChecks]));
    let t = create(&app, &alice(), "plain", "make it pass").await;
    assert!(t.job.gate.requires(CheckSource::AgentChecks));
    assert_eq!(t.job.task.as_deref(), Some("make it pass"));

    apply(&app, t.id, branch()).await;
    apply(&app, t.id, checks(false, &["test_login fails"])).await;
    let mid = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        mid.job.pushed.as_ref().map(|p| p.commit.as_str()),
        Some(SHA)
    );
    assert_eq!(mid.job.results.len(), 1, "saved before the agent finished");

    apply(&app, t.id, completed()).await;
    let reworked = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(reworked.state, ThreadState::Queued);
    assert_eq!(reworked.job.attempt, 2);
    assert!(reworked.job.results.is_empty() && reworked.job.pushed.is_none());
    let texts = delegated_texts(&w, t.id).await;
    assert_eq!(texts.len(), 2, "the task, then the rework");
    assert!(texts[1].contains("test_login fails"), "{}", texts[1]);
    let ev = events(&app, &alice(), t.id).await;
    assert!(kinds(&ev).contains(&orch_core::EventKind::Rework));
    assert!(kinds(&ev).contains(&orch_core::EventKind::CheckResult));
    assert_contiguous(&ev);

    // The second attempt passes.
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, checks(true, &[])).await;
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(done.state, ThreadState::Done);
    assert_eq!(done.job.attempt, 2);
}

#[tokio::test]
async fn running_out_of_attempts_fails_the_thread_with_the_findings() {
    let w = World::new();
    let mut cfg = gated(&[CheckSource::AgentChecks]);
    cfg.gate.max_attempts = 2;
    let app = w.app_with(cfg);
    let t = create(&app, &alice(), "plain", "go").await;
    for _ in 0..2 {
        apply(&app, t.id, checks(false, &["still red"])).await;
        apply(&app, t.id, completed()).await;
    }
    let failed = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(failed.state, ThreadState::Failed);
    assert_eq!(failed.job.attempt, 2);
    let ev = events(&app, &alice(), t.id).await;
    let error = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d),
            _ => None,
        })
        .unwrap();
    assert!(error.message.contains("still red"));
}

#[tokio::test]
async fn a_thread_keeps_the_gate_it_was_created_under() {
    let w = World::new();
    let strict = w.app_with(gated(&[CheckSource::AgentChecks]));
    let t = create(&strict, &alice(), "plain", "go").await;
    // A replica configured differently (or a later release) serves the same thread.
    let lax = w.app();
    apply(&lax, t.id, checks(false, &["red"])).await;
    apply(&lax, t.id, completed()).await;
    let after = lax.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        after.state,
        ThreadState::Queued,
        "still gated: reworked, not done"
    );
    assert!(after.job.gate.requires(CheckSource::AgentChecks));
}

#[tokio::test]
async fn verifying_is_a_state_the_thread_can_rest_in_and_be_cancelled_from() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::Ci]));
    let t = create(&app, &alice(), "plain", "go").await;
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, completed()).await;
    let verifying = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(verifying.state, ThreadState::Verifying);
    assert_eq!(verifying.job.verification, 1);

    // A CI report for the pushed commit, delivered as the inbox will deliver it later.
    let report = CiReport {
        provider: CiProvider::Generic,
        repository: "github.com/o/r".into(),
        sha: SHA.into(),
        branch: None,
        name: "build".into(),
        conclusion: CiConclusion::Success,
        url: None,
        summary: None,
    };
    let stale = CiReport {
        sha: "d".repeat(40),
        ..report.clone()
    };
    apply(&app, t.id, Input::CiReported(stale)).await;
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Verifying
    );
    // A user cannot forge one.
    let forged = app
        .submit(&alice(), t.id, Input::CiReported(report.clone()), None)
        .await;
    assert!(
        matches!(forged, Err(orch_app::AppError::Invalid(_))),
        "{forged:?}"
    );

    app.cancel(&alice(), t.id).await.unwrap();
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Cancelled
    );
}

#[tokio::test]
async fn a_passing_ci_report_finishes_a_verifying_thread() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::Ci]));
    let t = create(&app, &alice(), "plain", "go").await;
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, completed()).await;
    apply(
        &app,
        t.id,
        Input::CiReported(CiReport {
            provider: CiProvider::Github,
            repository: "https://github.com/O/R".into(),
            sha: SHA.into(),
            branch: Some("agent/x".into()),
            name: "build".into(),
            conclusion: CiConclusion::Success,
            url: None,
            summary: None,
        }),
    )
    .await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(done.state, ThreadState::Done);
}

/// What the dispatcher does with each envelope of a task: the update, its idempotency key and the
/// binding (the task and its state) written with it.
async fn apply_as_dispatcher(
    app: &TestApp,
    id: ThreadId,
    input: Input,
    key: &str,
    task: &str,
    state: Option<AgentTaskState>,
) -> ApplyOutcome {
    let binding = orch_ports::BindingUpdate {
        task_id: Some(task.to_owned()),
        task_state: state,
        revision: None,
    };
    app.apply(id, input, Some(key.to_owned()), Some(binding), None)
        .await
        .unwrap()
}

/// The worker that delegated attempt 1 dies after the completion that started the rework was
/// committed and before it finished its outbox row. The row is claimed again and the task is
/// followed from its start: every envelope comes again, under the keys it had. Nothing is
/// written: not a second `check_result`, not a second `rework`, not a third delegation, and the
/// completion of the old task is not taken for the completion of attempt 2.
#[tokio::test]
async fn a_replayed_completion_of_the_task_a_rework_left_changes_nothing() {
    use AgentTaskState::{Completed, Working};
    let w = World::new();
    let cfg = || gated(&[CheckSource::AgentChecks]);
    let app = w.app_with(cfg());
    let t = create(&app, &alice(), "plain", "make it pass").await;

    let task = "task-1";
    let envelopes = [
        (
            from_agent(AgentUpdate::Status {
                state: Working,
                detail: None,
            }),
            "turn:row-1:task-1:status:working",
            Some(Working),
        ),
        (branch(), "a2a:task-1:artifact:branch", None),
        (
            checks(false, &["test_login fails"]),
            "a2a:task-1:artifact:checks",
            None,
        ),
        (
            completed(),
            "turn:row-1:task-1:status:completed",
            Some(Completed),
        ),
    ];
    for (input, key, state) in envelopes.clone() {
        let outcome = apply_as_dispatcher(&app, t.id, input, key, task, state).await;
        assert!(
            matches!(outcome, ApplyOutcome::Applied { .. }),
            "{key}: {outcome:?}"
        );
    }
    let reworked = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        (reworked.state, reworked.job.attempt),
        (ThreadState::Queued, 2)
    );
    let log = events(&app, &alice(), t.id).await;
    let delegations = delegated_texts(&w, t.id).await;
    assert_eq!(delegations.len(), 2, "the task, then the rework");

    // Another process, the same database, the same task followed from its start.
    let app2 = w.app_with(cfg());
    for (input, key, state) in envelopes {
        let outcome = apply_as_dispatcher(&app2, t.id, input, key, task, state).await;
        assert_eq!(outcome, ApplyOutcome::Duplicate, "{key}");
    }

    let after = app2.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((after.state, after.job.attempt), (ThreadState::Queued, 2));
    assert_eq!(after.job, reworked.job, "the ledger is untouched");
    assert_eq!(
        events(&app2, &alice(), t.id).await,
        log,
        "the log is untouched"
    );
    assert_eq!(
        delegated_texts(&w, t.id).await,
        delegations,
        "no third delegation"
    );
}
