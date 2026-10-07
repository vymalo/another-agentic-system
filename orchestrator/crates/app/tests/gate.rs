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
    let mut gate = GatePolicy::requiring(sources.iter().copied());
    // A gate that requires CI names its checks (`GateError::CiWithoutChecks`).
    gate.ci.required = ["build".to_owned()].into();
    AppConfig {
        stream_poll: Duration::from_millis(100),
        gate,
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
            OutboxPayload::Action { .. }
            | OutboxPayload::Cancel { .. }
            | OutboxPayload::Verify { .. }
            | OutboxPayload::Title { .. }
            | OutboxPayload::Description { .. }
            | OutboxPayload::Ask { .. }
            | OutboxPayload::Steer { .. } => None,
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
    // the job is the default but for the description's ledger, which the end of the job counted
    // (ADR 0035) whether or not the deployment has a description task
    assert!(!done.job.description.may_ask(1), "job 1 asked");
    assert_eq!(
        done.job,
        Job {
            description: done.job.description,
            ..Job::default()
        }
    );
}

/// The owner's "Hi" (ADR 0018, 2026-09-30, and again 2026-10-04: "plot an image in TypeScript and
/// show it here"): the coder answered in plain text and completed. This test used to assert that
/// checks with no pushed commit never end the thread done (the gate reworked the thread twice and
/// failed it). The owner decided on 2026-10-04 that only pushed work is verified, so an agent that
/// pushed nothing gave an answer: the thread is Done on attempt 1, whatever else was reported.
#[tokio::test]
async fn an_agent_that_pushed_nothing_gave_an_answer_and_the_thread_is_done() {
    // no checks, passing checks and failing checks: none of them is about a pushed commit
    for reported in [
        None,
        Some(checks(true, &[])),
        Some(checks(false, &["boom"])),
    ] {
        let w = World::new();
        let app = w.app_with(gated(&[CheckSource::AgentChecks]));
        let t = create(&app, &alice(), "plain", "Hi").await;
        if let Some(reported) = reported {
            apply(&app, t.id, reported).await;
        }
        apply(&app, t.id, completed()).await;
        let after = app.get_thread(&alice(), t.id).await.unwrap();
        assert_eq!((after.state, after.job.attempt), (ThreadState::Done, 1));
        assert_eq!(
            delegated_texts(&w, t.id).await,
            ["Hi"],
            "only the person's message went to the agent: no rework was sent"
        );
        let ev = events(&app, &alice(), t.id).await;
        assert!(
            ev.iter().all(|e| !matches!(
                e.body,
                EventBody::CheckResult(_) | EventBody::Rework(_) | EventBody::Error(_)
            )),
            "nothing was verified, so nothing says it was: {ev:?}"
        );
        let announced: Vec<ThreadState> = ev
            .iter()
            .filter_map(|e| match &e.body {
                EventBody::ThreadState(d) => Some(d.state),
                _ => None,
            })
            .collect();
        assert_eq!(announced, [ThreadState::Done]);
    }
}

/// A `branch` artifact the gate cannot use is an agent that tried to push and got it wrong: the
/// gate applies, with the reason, as it did before the answers' rule.
#[tokio::test]
async fn a_branch_artifact_that_is_unusable_fails_with_its_reason() {
    let w = World::new();
    let mut config = gated(&[CheckSource::AgentChecks]);
    config.gate.max_attempts = 1;
    let app = w.app_with(config);
    let t = create(&app, &alice(), "plain", "fix it").await;
    apply(
        &app,
        t.id,
        artifact(
            "branch",
            json!({"repository": "https://github.com/o/r.git", "branch": "agent/x", "commit": "abc"}),
        ),
    )
    .await;
    apply(&app, t.id, checks(true, &[])).await;
    apply(&app, t.id, completed()).await;
    let failed = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(failed.state, ThreadState::Failed);
    let ev = events(&app, &alice(), t.id).await;
    let message = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.message.clone()),
            _ => None,
        })
        .expect("an error event");
    assert!(
        message.contains("the `branch` artifact was not usable")
            && message.contains("`commit` is not a full commit hash"),
        "{message}"
    );
}

/// Pushed work is verified as before: checks on another commit fail, checks on the pushed one
/// pass, and a rework that pushes nothing does not leave the gate.
#[tokio::test]
async fn pushed_work_is_verified_and_a_rework_that_pushes_nothing_is_not_an_answer() {
    let w = World::new();
    let mut config = gated(&[CheckSource::AgentChecks]);
    config.gate.max_attempts = 2;
    let app = w.app_with(config);
    let t = create(&app, &alice(), "plain", "fix login").await;
    // checks on another commit than the pushed one: failed, sent back
    apply(&app, t.id, branch()).await;
    apply(
        &app,
        t.id,
        artifact(
            "checks",
            json!({"passed": true, "commit": "dddddddddddddddddddddddddddddddddddddddd"}),
        ),
    )
    .await;
    apply(&app, t.id, completed()).await;
    let after = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((after.state, after.job.attempt), (ThreadState::Queued, 2));
    // attempt 2 pushes nothing: that is no answer after a failed push, so it fails, with the reason
    apply(&app, t.id, checks(true, &[])).await;
    apply(&app, t.id, completed()).await;
    let failed = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(failed.state, ThreadState::Failed);
    let ev = events(&app, &alice(), t.id).await;
    let message = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.message.clone()),
            _ => None,
        })
        .expect("an error event");
    assert!(message.contains("no pushed commit"), "{message}");

    // a push whose checks passed on it: done
    let t = create(&app, &alice(), "plain", "fix login").await;
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, checks(true, &[])).await;
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((done.state, done.job.attempt), (ThreadState::Done, 1));
    let ev = events(&app, &alice(), t.id).await;
    assert!(
        ev.iter()
            .any(|e| matches!(&e.body, EventBody::CheckResult(c)
        if c.status == orch_core::CheckStatus::Passed))
    );
}

/// The owner's 2026-10-06 exports. A failed job, then "Can you investigate more?": the follow-up
/// job pushed nothing and reported no checks, and the gate used to fail it with "no checks
/// reported". A job that pushed nothing is done, in the job after a failed one as in any other.
#[tokio::test]
async fn an_investigation_after_a_failed_job_is_an_answer_not_a_failure() {
    let w = World::new();
    let mut config = gated(&[CheckSource::AgentChecks]);
    config.gate.max_attempts = 2;
    let app = w.app_with(config);
    let t = create(&app, &alice(), "plain", "fix the lint").await;
    for _ in 0..2 {
        apply(&app, t.id, branch()).await;
        apply(&app, t.id, checks(false, &["lint fails"])).await;
        apply(&app, t.id, completed()).await;
    }
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Failed
    );
    app.post_message(&alice(), t.id, "can you investigate more?".to_owned())
        .await
        .unwrap();
    let two = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((two.job.number, two.job.attempt), (2, 1));
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((done.state, done.job.attempt), (ThreadState::Done, 1));
    let ev = events(&app, &alice(), t.id).await;
    let after_job_2: Vec<_> = ev
        .iter()
        .skip_while(|e| !matches!(e.body, EventBody::JobStarted(_)))
        .collect();
    assert!(
        after_job_2.iter().all(|e| !matches!(
            e.body,
            EventBody::CheckResult(_) | EventBody::Rework(_) | EventBody::Error(_)
        )),
        "nothing was verified in job 2: {after_job_2:?}"
    );
}

/// A rework that exists because the `branch` artifact could not be used has no pushed commit to
/// answer for: told why, an agent with nothing to push (a scratch project) is done.
#[tokio::test]
async fn a_rework_after_an_unusable_branch_that_pushes_nothing_is_done() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::AgentChecks]));
    let t = create(&app, &alice(), "plain", "plot it").await;
    apply(
        &app,
        t.id,
        artifact(
            "branch",
            json!({"repository": "https://github.com/o/r.git", "branch": "agent/x", "commit": "abc"}),
        ),
    )
    .await;
    apply(&app, t.id, completed()).await;
    let second = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((second.state, second.job.attempt), (ThreadState::Queued, 2));
    apply(&app, t.id, checks(true, &[])).await;
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((done.state, done.job.attempt), (ThreadState::Done, 2));
}

/// The owner's 2026-10-07 "What is this repo about?": `yarn check` failed on errors the
/// repository had already. The coder marks a check that fails on the base commit too
/// (`preexisting`), and the gate passes the work with a note that says which.
#[tokio::test]
async fn checks_that_fail_on_the_base_too_pass_and_the_result_says_which() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::AgentChecks]));
    let t = create(
        &app,
        &alice(),
        "plain",
        "what does this repo do, and fix the typo",
    )
    .await;
    apply(&app, t.id, branch()).await;
    apply(
        &app,
        t.id,
        artifact(
            "checks",
            json!({"passed": false, "commit": SHA, "summary": "1 of 2 checks failed", "findings": [
                {"check": "yarn check", "message": "12 type errors", "preexisting": true,
                 "base_commit": "dddddddddddddddddddddddddddddddddddddddd"}
            ]}),
        ),
    )
    .await;
    apply(&app, t.id, completed()).await;
    let done = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!((done.state, done.job.attempt), (ThreadState::Done, 1));
    let ev = events(&app, &alice(), t.id).await;
    let result = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::CheckResult(c) => Some(c.clone()),
            _ => None,
        })
        .expect("a check_result");
    assert_eq!(result.status, orch_core::CheckStatus::Passed);
    let summary = result.summary.unwrap();
    assert!(
        summary.contains("failing on the base commit dddddddddddd too")
            && summary.contains("`yarn check`"),
        "{summary}"
    );
    assert!(result.findings.is_empty());
}

/// The coder asks what to do, the person answers, the attempt fails: the rework must carry the
/// answer too, or the agent is told to keep working on "Hi".
#[tokio::test]
async fn a_rework_carries_every_message_the_person_wrote() {
    let w = World::new();
    let app = w.app_with(gated(&[CheckSource::AgentChecks]));
    let t = create(&app, &alice(), "plain", "Hi").await;
    app.post_message(&alice(), t.id, "fix login in acme/widgets".into())
        .await
        .unwrap();
    apply(&app, t.id, branch()).await;
    apply(&app, t.id, checks(false, &["test_login fails"])).await;
    apply(&app, t.id, completed()).await;
    let texts = delegated_texts(&w, t.id).await;
    let rework = texts.last().unwrap();
    assert!(
        rework.contains("```request\nHi\n\n[next message]\nfix login in acme/widgets\n```"),
        "{rework}"
    );
    let job = app.get_thread(&alice(), t.id).await.unwrap().job;
    assert_eq!(
        job.task.as_deref(),
        Some("Hi\n\n[next message]\nfix login in acme/widgets")
    );
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
    // The next attempt is a new task: the delegation carries the person's request itself.
    assert!(
        texts[1].contains("```request\nmake it pass\n```"),
        "{}",
        texts[1]
    );
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
        apply(&app, t.id, branch()).await;
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
    apply(&lax, t.id, branch()).await;
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
        context_id: None,
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
