//! Child sessions through the application (ADR 0072, S2d): what an agent says of its sessions makes
//! a child thread and the parent's events once each however often it is said, the orchestrator
//! follows the round the child thread is for without sending anything, usage is counted once, a
//! parent's delete takes its children and refuses while one works, and a child is neither forked nor
//! shared.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AppConfig, AppError, ChildSessionDrop, ForkAt, ForkRequest, PublicView, SharingMode,
    SharingSettings,
};
use orch_core::{
    AgentTaskState, EventBody, EventKind, NotDeletable, RoundReport, SessionBy, SessionNews,
    SessionOutcome, SessionReport, SessionState, ThreadId, ThreadRecord, ThreadState,
};
use orch_ports::memory::Call;
use orch_ports::{Lease, OutboxKind, OutboxPayload, ThreadStore};
use support::*;

fn report(session: &str, state: SessionState, round: Option<RoundReport>) -> SessionReport {
    SessionReport {
        session: session.into(),
        context_id: format!("ctx-1/s/{session}"),
        agent: "planner".into(),
        description: "Plan the migration".into(),
        parent_context_id: None,
        keep_open: false,
        by: Some(SessionBy::Agent),
        state,
        closing: false,
        round,
    }
}

fn round(number: u32, task: &str, state: AgentTaskState) -> RoundReport {
    RoundReport {
        number,
        task_id: Some(task.into()),
        state: Some(state),
    }
}

fn count(events: &[orch_core::Event], kind: EventKind) -> usize {
    events.iter().filter(|e| e.kind() == kind).count()
}

async fn children(w: &World, parent: ThreadId) -> Vec<ThreadRecord> {
    w.store.child_threads(&alice(), parent).await.unwrap()
}

/// The stats of the dropped news, by reason.
fn dropped(app: &TestApp, why: ChildSessionDrop) -> u64 {
    app.child_session_stats()
        .dropped
        .iter()
        .find(|(reason, _)| *reason == why)
        .map_or(0, |(_, n)| *n)
}

#[tokio::test]
async fn the_news_of_a_session_makes_a_child_thread_and_the_parents_event_once_however_often() {
    let w = World::new();
    let app = w.app();
    // no dispatcher: the parent stays queued, and a session is heard of in every state
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let said = report(
        "s1",
        SessionState::Working,
        Some(round(1, "r-1", AgentTaskState::Working)),
    );
    for _ in 0..3 {
        assert!(
            app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said.clone()]))
                .await
                .unwrap()
        );
    }
    let kids = children(&w, parent.id).await;
    assert_eq!(kids.len(), 1, "one child for one session");
    let kid = &kids[0];
    assert_eq!(kid.owner, alice());
    assert_eq!(kid.target, parent.target);
    assert_eq!(kid.title, "Plan the migration");
    assert_eq!(kid.description.as_deref(), Some("Plan the migration"));
    assert_eq!(kid.state, ThreadState::Queued);
    assert_eq!(kid.rail_parent, Some(parent.id));
    let of = kid.child_of.as_ref().unwrap();
    assert_eq!((of.thread, of.session.as_str()), (parent.id, "s1"));
    assert_eq!(kid.job.child.as_ref().unwrap().agent, "planner");
    // no event of its own yet: the round starts when the dispatcher follows it
    assert_eq!(kid.last_seq, 0);

    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 0);
    assert_eq!(count(&ev, EventKind::ChildSessionClosed), 0);
    match &ev
        .iter()
        .find(|e| e.kind() == EventKind::ChildSessionOpened)
        .unwrap()
        .body
    {
        EventBody::ChildSessionOpened(data) => {
            assert_eq!(data.thread, kid.id);
            assert_eq!(data.session, "s1");
            assert_eq!(data.agent, "planner");
            assert_eq!(data.round, Some(1));
        }
        other => panic!("{other:?}"),
    }

    // exactly one row follows the round, and nothing is sent to the agent
    let rows = w.store.list_open_outbox(kid.id).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, OutboxKind::Delegate);
    assert!(matches!(
        rows[0].payload,
        OutboxPayload::Observe { round: 1, ref task_id, .. } if task_id == "r-1"
    ));

    // the round ended and the session closed: the rest of the parent's events, once each
    let over = report(
        "s1",
        SessionState::Closed,
        Some(round(1, "r-1", AgentTaskState::Completed)),
    );
    for _ in 0..2 {
        app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![over.clone()]))
            .await
            .unwrap();
    }
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 1);
    assert_eq!(count(&ev, EventKind::ChildSessionClosed), 1);
    assert_eq!(children(&w, parent.id).await.len(), 1);
    // told twice more, the child's round is still followed once
    assert_eq!(w.store.list_open_outbox(kid.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn news_that_breaks_a_bound_creates_nothing_and_is_counted() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let mut bad = report("s1", SessionState::Working, None);
    bad.session = String::new();
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![bad]))
        .await
        .unwrap();
    assert!(children(&w, parent.id).await.is_empty());
    assert_eq!(dropped(&app, ChildSessionDrop::Malformed), 1);
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 0);
    // a well-formed report beside a malformed one is still taken
    let mut bad = report("s2", SessionState::Working, None);
    bad.context_id = "x\ny".into();
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![bad, report("s3", SessionState::Working, None)]),
    )
    .await
    .unwrap();
    assert_eq!(children(&w, parent.id).await.len(), 1);
    assert_eq!(dropped(&app, ChildSessionDrop::Malformed), 2);
}

#[tokio::test]
async fn a_session_whose_child_was_deleted_is_skipped_and_counted() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let said = report("s1", SessionState::Idle, None);
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said.clone()]))
        .await
        .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    assert_eq!(
        kid.state,
        ThreadState::Done,
        "a child with nothing to follow is at rest"
    );
    // the child is erased alone, through the application: the parent keeps the note, and is never
    // given another child for the session
    app.delete_thread(&alice(), kid.id).await.unwrap();
    let said = report("s1", SessionState::Closed, None);
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said]))
        .await
        .unwrap();
    assert!(children(&w, parent.id).await.is_empty());
    assert_eq!(dropped(&app, ChildSessionDrop::ChildDeleted), 1);
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
}

#[tokio::test]
async fn a_parent_that_notes_256_sessions_refuses_the_next_and_makes_no_thread_for_it() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let reports: Vec<SessionReport> = (0..256)
        .map(|n| report(&format!("s{n}"), SessionState::Idle, None))
        .collect();
    for chunk in reports.chunks(64) {
        app.child_sessions_news(parent.id, None, SessionNews::Listed(chunk.to_vec()))
            .await
            .unwrap();
    }
    assert_eq!(children(&w, parent.id).await.len(), 256);
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report("s256", SessionState::Idle, None)]),
    )
    .await
    .unwrap();
    assert_eq!(children(&w, parent.id).await.len(), 256);
    assert_eq!(dropped(&app, ChildSessionDrop::LedgerFull), 1);
}

#[tokio::test]
async fn a_child_has_no_children_of_its_own() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report("s1", SessionState::Idle, None)]),
    )
    .await
    .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    app.child_sessions_news(
        kid.id,
        None,
        SessionNews::Listed(vec![report("deeper", SessionState::Idle, None)]),
    )
    .await
    .unwrap();
    assert!(children(&w, kid.id).await.is_empty());
    assert_eq!(dropped(&app, ChildSessionDrop::Refused), 1);
}

#[tokio::test]
async fn the_dispatcher_follows_the_round_without_sending_and_the_parent_hears_of_it_once() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions planner").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    let kid = wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;

    // the child's log is the session's close, then the round: a job that nobody sent, its artifact,
    // its totals and its end
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(
        shape(&ev),
        [
            "child_session_state",
            "job_started",
            "artifact",
            "model_usage_total",
            "agent_status:completed",
            "thread_state:done"
        ],
        "{ev:?}"
    );
    assert_contiguous(&ev);
    match &ev[1].body {
        EventBody::JobStarted(started) => {
            assert_eq!(started.job, 1);
            let round = started.round.as_ref().unwrap();
            assert_eq!(round.number, 1);
            assert_eq!(round.by, Some(SessionBy::Agent));
        }
        other => panic!("{other:?}"),
    }
    // nothing was sent to the agent for the child: only the parent's message, and the child's
    // round was read by task, never by message
    let sends: Vec<_> = w
        .agent
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Send { .. }))
        .collect();
    assert_eq!(sends.len(), 1, "{sends:?}");
    assert!(
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::GetTask { task_id } if task_id == "task-1-r1"))
            || w.agent
                .calls()
                .iter()
                .any(|c| matches!(c, Call::Resubscribe { task_id } if task_id == "task-1-r1"))
    );
    // the child's binding is the round's, in the child's own context
    let binding = w.store.get_binding(kid.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id.as_deref(), Some("ctx-1/s/task-1-s1"));
    assert_eq!(binding.task_id.as_deref(), Some("task-1-r1"));

    // the parent's log: opened, reported and closed once each
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1, "{ev:?}");
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 1, "{ev:?}");
    assert_eq!(count(&ev, EventKind::ChildSessionClosed), 1, "{ev:?}");
    let reported = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionReported(data) => Some(data),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (reported.round, reported.report, reported.thread),
        (1, SessionOutcome::Done, kid.id)
    );

    // the ledger counts the parent's totals (20 in, 10 out: the helper's tokens are in them) and
    // not the helper's own (10 in, 5 out) a second time
    let month = app.usage_month(&alice(), None).await.unwrap();
    assert_eq!(month.models.len(), 1);
    assert_eq!(month.models[0].usage.tokens.total_tokens, 30);
    assert_eq!(month.tasks, 1);
    // the helper's usage is in its own log, marked as not counted
    let uncounted = ev_of(&app, kid.id).await;
    assert!(uncounted, "the child's totals say counted: false");
}

async fn ev_of(app: &TestApp, id: ThreadId) -> bool {
    events(app, &alice(), id)
        .await
        .iter()
        .any(|e| matches!(&e.body, EventBody::ModelUsageTotal(t) if !t.counted))
}

#[tokio::test]
async fn a_news_the_adapter_refused_is_dropped_and_counted_and_the_task_goes_on() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions-bad").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    run.shutdown().await;
    assert!(children(&w, parent.id).await.is_empty());
    assert_eq!(dropped(&app, ChildSessionDrop::Rejected), 1);
}

#[tokio::test]
async fn a_restart_while_a_round_runs_follows_it_again_and_writes_nothing_twice() {
    let w = World::new();
    let app = w.app();
    let first = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions planner gate").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    // the round works: the child follows it
    wait_state(&app, &alice(), kid.id, ThreadState::Working).await;
    eventually("the child to follow its round", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { task_id } if task_id == "task-1-r1"))
            .then_some(())
    })
    .await;
    first.kill();
    // its lease lapses; another dispatcher takes the row and follows the same task
    let second = spawn_dispatcher(&app, fast(), "d2");
    tokio::time::sleep(Duration::from_millis(600)).await;
    w.agent.release_gate();
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    second.shutdown().await;
    let ev = events(&app, &alice(), kid.id).await;
    assert_eq!(count(&ev, EventKind::JobStarted), 1, "{:?}", shape(&ev));
    assert_eq!(
        count(&ev, EventKind::ThreadState),
        1,
        "done, once: {:?}",
        shape(&ev)
    );
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1);
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 1);
    assert_eq!(children(&w, parent.id).await.len(), 1);
}

#[tokio::test]
async fn deleting_a_parent_erases_its_children_and_a_working_child_refuses() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions planner gate").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    wait_state(&app, &alice(), kid.id, ThreadState::Working).await;

    // the child works: the parent's delete is refused and nothing goes
    let err = app.delete_thread(&alice(), parent.id).await.unwrap_err();
    assert!(
        matches!(
            err,
            AppError::ThreadActive(NotDeletable::Active(ThreadState::Working))
        ),
        "{err:?}"
    );
    assert!(app.get_thread(&alice(), parent.id).await.is_ok());
    assert!(app.get_thread(&alice(), kid.id).await.is_ok());

    // it ends: both go together
    w.agent.release_gate();
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;
    app.delete_thread(&alice(), parent.id).await.unwrap();
    for id in [parent.id, kid.id] {
        assert!(
            matches!(app.get_thread(&alice(), id).await, Err(AppError::NotFound)),
            "{id} is gone"
        );
    }
}

fn sharing_app(w: &World) -> Arc<TestApp> {
    let keys = orch_app::ShareKeys::new(
        secrecy::SecretString::from("0123456789abcdef0123456789abcdef".to_owned()),
        None,
    )
    .unwrap();
    w.app_with(AppConfig {
        sharing: SharingSettings::new(SharingMode::Public, Some(keys), PublicView::default())
            .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    })
}

#[tokio::test]
async fn a_child_is_neither_forked_nor_shared_and_a_fork_of_its_parent_takes_no_children() {
    let w = World::new();
    let app = sharing_app(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions planner").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    run.shutdown().await;

    let fork = |seq| ForkRequest {
        at: ForkAt::AfterTurn { seq, first: None },
        target: None,
        id: None,
    };
    let kid_seq = app.get_thread(&alice(), kid.id).await.unwrap().last_seq;
    match app.fork_thread(&alice(), kid.id, fork(kid_seq)).await {
        Err(AppError::Arrangement { code, .. }) => assert_eq!(code, "child_thread"),
        other => panic!("{other:?}"),
    }
    match app
        .share_thread(&alice(), kid.id, orch_core::ShareLevel::Internal)
        .await
    {
        Err(AppError::Arrangement { code, .. }) => assert_eq!(code, "child_thread"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        app.get_thread(&alice(), kid.id).await.unwrap().share,
        None,
        "nothing was shared"
    );

    // the parent forks as any thread does: its log is copied, its sessions and children are not
    let last = app.get_thread(&alice(), parent.id).await.unwrap().last_seq;
    let forked = app
        .fork_thread(&alice(), parent.id, fork(last))
        .await
        .unwrap()
        .thread;
    assert!(forked.child_of.is_none());
    assert!(forked.job.sessions.is_empty(), "{:?}", forked.job.sessions);
    assert!(children(&w, forked.id).await.is_empty());
    // the copy keeps the log's words, and never says again to the agent what a session did
    let copied = events(&app, &alice(), forked.id).await;
    assert_eq!(count(&copied, EventKind::ChildSessionOpened), 1);
    // and the parent still has its one child
    assert_eq!(children(&w, parent.id).await.len(), 1);
}

#[tokio::test]
async fn a_child_with_nothing_to_follow_is_at_rest_and_its_parent_can_be_deleted() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    run.shutdown().await;
    // idle with no round; closed as the first thing seen; a round with no task to follow
    let reports = vec![
        report("idle", SessionState::Idle, None),
        report("closed", SessionState::Closed, None),
        report(
            "notask",
            SessionState::Working,
            Some(RoundReport {
                number: 1,
                task_id: None,
                state: None,
            }),
        ),
    ];
    app.child_sessions_news(parent.id, None, SessionNews::Listed(reports))
        .await
        .unwrap();
    let kids = children(&w, parent.id).await;
    assert_eq!(kids.len(), 3);
    for kid in &kids {
        assert_eq!(kid.state, ThreadState::Done, "{}", kid.title);
        assert!(w.store.list_open_outbox(kid.id).await.unwrap().is_empty());
    }
    // deleted through the application: the parent and the three children go
    app.delete_thread(&alice(), parent.id).await.unwrap();
    for id in std::iter::once(parent.id).chain(kids.iter().map(|k| k.id)) {
        assert!(matches!(
            app.get_thread(&alice(), id).await,
            Err(AppError::NotFound)
        ));
    }
}

#[tokio::test]
async fn a_blocked_round_answered_in_the_same_task_is_followed_and_reported_again() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions-blocked planner").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    wait_state(&app, &alice(), kid.id, ThreadState::Blocked).await;
    let reports = |ev: &[orch_core::Event]| -> Vec<(u32, SessionOutcome)> {
        ev.iter()
            .filter_map(|e| match &e.body {
                EventBody::ChildSessionReported(d) => Some((d.round, d.report)),
                _ => None,
            })
            .collect()
    };
    eventually("the blocked round to be reported", || async {
        (reports(&events(&app, &alice(), parent.id).await) == [(1, SessionOutcome::Blocked)])
            .then_some(())
    })
    .await;
    // the person answers within the same round: the agent goes on, and the parent is told
    w.agent.release_gate();
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report(
            "task-1-s1",
            SessionState::Working,
            Some(round(1, "task-1-r1", AgentTaskState::Working)),
        )]),
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), kid.id, ThreadState::Done).await;
    eventually("the round to be reported again", || async {
        (reports(&events(&app, &alice(), parent.id).await)
            == [(1, SessionOutcome::Blocked), (1, SessionOutcome::Done)])
        .then_some(())
    })
    .await;
    run.shutdown().await;
    let log = events(&app, &alice(), kid.id).await;
    assert_eq!(
        count(&log, EventKind::JobStarted),
        1,
        "one job: {:?}",
        shape(&log)
    );
}

#[tokio::test]
async fn one_parent_has_at_most_eight_rounds_followed_at_once_and_the_rest_wait() {
    let w = World::new();
    let app = w.app();
    let cfg = orch_app::DispatcherConfig {
        concurrency: 14,
        ..fast()
    };
    let run = spawn_dispatcher(&app, cfg, "d1");
    let parent = create(&app, &alice(), "plain", "sessions-many 11").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let followed = || {
        w.agent
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Resubscribe { task_id } if task_id.contains("-r")))
            .count()
    };
    eventually("eight rounds to be followed", || async {
        (followed() >= orch_app::MAX_OBSERVED_PER_PARENT).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let mut tasks: Vec<String> = w
        .agent
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::Resubscribe { task_id } if task_id.contains("-r") => Some(task_id.clone()),
            _ => None,
        })
        .collect();
    tasks.sort();
    tasks.dedup();
    assert_eq!(tasks.len(), orch_app::MAX_OBSERVED_PER_PARENT, "{tasks:?}");
    // the other three wait, and are never dropped: each round ends, each child is done
    for _ in 0..11 {
        w.agent.release_gate();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    eventually("every child to end", || async {
        let kids = children(&w, parent.id).await;
        (kids.len() == 11 && kids.iter().all(|k| k.state == ThreadState::Done)).then_some(())
    })
    .await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_round_that_names_another_contexts_task_is_refused_and_fails_the_follow() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let parent = create(&app, &alice(), "plain", "sessions-foreign").await;
    wait_state(&app, &alice(), parent.id, ThreadState::Done).await;
    let kid = eventually("the child thread", || async {
        children(&w, parent.id).await.into_iter().next()
    })
    .await;
    wait_state(&app, &alice(), kid.id, ThreadState::Failed).await;
    run.shutdown().await;
    let log = events(&app, &alice(), kid.id).await;
    // nothing of the parent's task was copied into the child
    assert_eq!(count(&log, EventKind::Artifact), 0, "{:?}", shape(&log));
    assert!(dropped(&app, ChildSessionDrop::ForeignContext) >= 1);
    let binding = w.store.get_binding(kid.id).await.unwrap().unwrap();
    assert_eq!(binding.context_id.as_deref(), Some("ctx-1/s/task-1-s1"));
}

#[tokio::test]
async fn a_closed_helper_takes_no_message_steer_stop_and_send_or_action() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report("s1", SessionState::Closed, None)]),
    )
    .await
    .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    let closed = |e: AppError| match e {
        AppError::Blocked { code, .. } => assert_eq!(code, "session_closed"),
        other => panic!("{other:?}"),
    };
    closed(
        app.post_message(&alice(), kid.id, "hello".into())
            .await
            .unwrap_err(),
    );
    closed(
        app.stop_and_send(&alice(), kid.id, "hello".into())
            .await
            .unwrap_err(),
    );
    let action = orch_core::UiActionData {
        surface_id: "s1".into(),
        name: "go".into(),
        source_component_id: "go".into(),
        context: serde_json::Map::new(),
        version: orch_core::UiVersion::V1_0,
        run_id: None,
        tier: None,
        tier_cap: None,
    };
    closed(
        app.submit(
            &alice(),
            kid.id,
            orch_core::Input::UiAction {
                user: alice(),
                action,
                catalog: None,
                tier: None,
                tier_cap: None,
            },
            None,
        )
        .await
        .unwrap_err(),
    );
    let log = events(&app, &alice(), kid.id).await;
    assert!(
        log.iter().all(|e| e.kind() != EventKind::UserMessage),
        "nothing was written: {:?}",
        shape(&log)
    );
    assert!(w.store.list_open_outbox(kid.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_child_stopped_before_its_round_began_skips_the_row() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report(
            "s1",
            SessionState::Working,
            Some(round(1, "never", AgentTaskState::Working)),
        )]),
    )
    .await
    .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    assert_eq!(kid.state, ThreadState::Queued);
    // stopped before the round began (the row waits to be claimed)
    app.apply(
        kid.id,
        orch_core::Input::CancelledBeforeStart,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), kid.id, ThreadState::Cancelled).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    eventually("the observe row to end", || async {
        w.store
            .list_open_outbox(kid.id)
            .await
            .unwrap()
            .is_empty()
            .then_some(())
    })
    .await;
    run.shutdown().await;
    let log = events(&app, &alice(), kid.id).await;
    assert_eq!(count(&log, EventKind::JobStarted), 0, "{:?}", shape(&log));
    assert!(
        !w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { task_id } | Call::GetTask { task_id } if task_id == "never"))
    );
}

#[tokio::test]
async fn a_round_of_a_thread_that_is_no_child_or_of_another_session_is_refused_and_counted() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let over = |session: &str| {
        report(
            session,
            SessionState::Idle,
            Some(round(1, "t-1", AgentTaskState::Completed)),
        )
    };
    // a thread that is no child hears of no round
    assert!(
        app.child_sessions_news(parent.id, None, SessionNews::OfRound(over("s1")))
            .await
            .unwrap()
    );
    assert_eq!(dropped(&app, ChildSessionDrop::Refused), 1);
    assert_eq!(dropped(&app, ChildSessionDrop::Malformed), 0);
    // nor does a child hear of the round of another session, and its parent is told nothing
    app.child_sessions_news(
        parent.id,
        None,
        SessionNews::Listed(vec![report("s1", SessionState::Idle, None)]),
    )
    .await
    .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    assert!(
        app.child_sessions_news(kid.id, None, SessionNews::OfRound(over("other")))
            .await
            .unwrap()
    );
    assert_eq!(dropped(&app, ChildSessionDrop::Refused), 2);
    assert_eq!(dropped(&app, ChildSessionDrop::Malformed), 0);
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionOpened), 1, "{ev:?}");
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 0, "{ev:?}");
    assert_eq!(children(&w, parent.id).await.len(), 1);
}

#[tokio::test]
async fn the_same_listing_is_applied_once_and_not_read_again() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let idle = report("s1", SessionState::Idle, None);
    let closed = report("s1", SessionState::Closed, None);
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![idle.clone()]))
        .await
        .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    app.delete_thread(&alice(), kid.id).await.unwrap();
    // said again as it was: not read from the store, so nothing is found missing
    for _ in 0..2 {
        app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![idle.clone()]))
            .await
            .unwrap();
    }
    assert_eq!(dropped(&app, ChildSessionDrop::ChildDeleted), 0);
    // a listing that differs is read, once however often it is repeated
    for _ in 0..2 {
        app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![closed.clone()]))
            .await
            .unwrap();
    }
    assert_eq!(dropped(&app, ChildSessionDrop::ChildDeleted), 1);
}

#[tokio::test]
async fn a_round_told_under_a_lost_claim_writes_nothing_and_stops_the_caller() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let said = report(
        "s1",
        SessionState::Working,
        Some(round(1, "never", AgentTaskState::Working)),
    );
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said.clone()]))
        .await
        .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    let row = w.store.list_open_outbox(kid.id).await.unwrap().remove(0);
    let stale = Lease {
        id: row.id,
        owner: "nobody".into(),
        attempt: 9,
    };
    let over = report(
        "s1",
        SessionState::Idle,
        Some(round(1, "never", AgentTaskState::Completed)),
    );
    let go_on = app
        .child_sessions_news(kid.id, Some(&stale), SessionNews::OfRound(over))
        .await
        .unwrap();
    assert!(!go_on, "the caller must stop");
    let ev = events(&app, &alice(), parent.id).await;
    assert_eq!(count(&ev, EventKind::ChildSessionReported), 0, "{ev:?}");
}

#[tokio::test]
async fn a_blank_description_leaves_the_child_without_one() {
    let w = World::new();
    let app = w.app();
    let parent = create(&app, &alice(), "plain", "echo hi").await;
    let mut said = report("s1", SessionState::Idle, None);
    said.description = "  \t ".into();
    app.child_sessions_news(parent.id, None, SessionNews::Listed(vec![said]))
        .await
        .unwrap();
    let kid = children(&w, parent.id).await.remove(0);
    assert_eq!(kid.description, None);
}
