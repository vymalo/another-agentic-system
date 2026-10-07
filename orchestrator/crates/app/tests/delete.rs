//! Deleting a thread erases it (ADR 0043) through the application, the dispatcher, the inbox worker
//! and the purge sweep: the thread and its edits go with their files, forks stay, a thread that
//! works is refused, the permission is its own and the owner's, a purge that failed is finished by
//! the sweep, and a late input for a thread that is gone is dropped.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_app::{
    AgentScope, App, AppConfig, AppError, Dispatcher, ForkAt, ForkRequest, InboxConfig,
    InboxWorker, LateSource, Permission, Policy, PurgeConfig, PurgeWorker, RoleGrant,
};
use orch_core::{Classify, ErrorClass, ForkKind, NotDeletable, ThreadId, ThreadState, Timer};
use orch_ports::memory::{
    MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, FixedRegistry, InboxId,
    InboxPayload, InboxStatus, NewInbox, PortSet, Principal, Role, SystemClock, ThreadListing,
    ThreadStore, Topic, Wakeup,
};
use support::*;
use tokio_util::sync::CancellationToken;

/// Files that can be told to fail their erasure, for the sweep to finish.
#[derive(Clone, Default)]
struct Files {
    inner: MemoryArtifacts,
    /// How many of the next `delete_prefix` calls fail.
    failing: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl ArtifactStore for Files {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        self.inner.put(key, bytes, meta).await
    }
    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        self.inner.get(key).await
    }
    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.copy(from, to).await
    }
    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .failing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(ArtifactError::unavailable(
                "bucket orchestrator-secret-bucket is down",
            ));
        }
        self.inner.delete_prefix(thread).await
    }
}

type DeletePorts<X> = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    ScriptedModel,
    FixedRegistry,
    orch_ports::RefuseAll,
    X,
>;
type DApp<X> = App<DeletePorts<X>>;

fn app_with<X: ArtifactStore>(w: &World, files: X, policy: Policy) -> Arc<DApp<X>> {
    Arc::new(
        App::new(
            PortSet {
                artifacts: files,
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
                policy,
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn everyone<X: ArtifactStore>(w: &World, files: X) -> Arc<DApp<X>> {
    app_with(w, files, Policy::default())
}

fn dispatch<X: ArtifactStore>(app: &Arc<DApp<X>>) -> Running {
    let token = CancellationToken::new();
    let handle = tokio::spawn(Dispatcher::new(Arc::clone(app), fast(), "d1").run(token.clone()));
    Running { handle, token }
}

async fn start<X: ArtifactStore>(app: &DApp<X>, who: &orch_core::UserId, text: &str) -> ThreadId {
    app.create_thread(
        who,
        orch_app::NewThread {
            title: None,
            target: target("plain"),
            text: text.to_owned(),
        },
    )
    .await
    .unwrap()
    .id
}

async fn settle<X: ArtifactStore>(app: &DApp<X>, id: ThreadId, state: ThreadState) {
    eventually(&format!("{id} to be {state:?}"), || async {
        let t = app.get_thread(&alice(), id).await.ok()?;
        (t.state == state).then_some(())
    })
    .await;
}

/// A file of `thread` in the store.
async fn keep(files: &Files, thread: ThreadId, content: &str) -> ArtifactKey {
    let meta = ArtifactMeta::of("text/plain", None, content.as_bytes());
    let key = meta.key(thread);
    files
        .put(&key, Bytes::from(content.to_owned()), &meta)
        .await
        .unwrap();
    key
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

fn role(permissions: &[Permission]) -> RoleGrant {
    RoleGrant {
        permissions: permissions.iter().copied().collect::<BTreeSet<_>>(),
        agents: AgentScope::from_patterns(["*"]),
    }
}

fn after(seq: i64) -> ForkRequest {
    ForkRequest {
        at: ForkAt::AfterTurn { seq, first: None },
        target: None,
        id: None,
    }
}

fn replace(seq: i64, text: &str) -> ForkRequest {
    ForkRequest {
        at: ForkAt::Replace {
            seq,
            text: text.to_owned(),
            message_id: Some("m-edit".to_owned()),
        },
        target: None,
        id: None,
    }
}

async fn log<X: ArtifactStore>(
    app: &DApp<X>,
    who: &orch_core::UserId,
    id: ThreadId,
) -> Vec<orch_core::Event> {
    app.list_events(who, id, 0, 500).await.unwrap()
}

fn rail() -> ThreadListing {
    ThreadListing::recent(None, 50, false).in_rail_order()
}

#[tokio::test]
async fn deleting_erases_the_thread_its_edits_and_their_files_and_keeps_the_forks() {
    let w = World::new();
    let files = Files::default();
    let app = everyone(&w, files.clone());
    let run = dispatch(&app);
    let parent = start(&app, &alice(), "echo one").await;
    settle(&app, parent, ThreadState::Done).await;
    let message = log(&app, &alice(), parent).await[0].seq;
    // an edit of the first message (a branch the list does not show), and an edit of the edit
    let edit = app
        .fork_thread(&alice(), parent, replace(message, "echo edited"))
        .await
        .unwrap()
        .thread
        .id;
    settle(&app, edit, ThreadState::Done).await;
    let edit_message = log(&app, &alice(), edit)
        .await
        .iter()
        .find(|e| e.kind() == orch_core::EventKind::UserMessage && e.seq > 1)
        .map(|e| e.seq)
        .unwrap();
    let edit_of_edit = app
        .fork_thread(&alice(), edit, replace(edit_message, "echo again"))
        .await
        .unwrap()
        .thread
        .id;
    settle(&app, edit_of_edit, ThreadState::Done).await;
    // a fork from the end of the first turn: a conversation of its own, nested under its parent
    let seq = log(&app, &alice(), parent).await.last().unwrap().seq;
    let fork = app
        .fork_thread(&alice(), parent, after(seq))
        .await
        .unwrap()
        .thread
        .id;
    // another thread of the person's, and the files of each
    let other = start(&app, &alice(), "echo other").await;
    settle(&app, other, ThreadState::Done).await;
    run.shutdown().await;
    let (k_parent, k_edit, k_edit2, k_fork, k_other) = (
        keep(&files, parent, "of the parent").await,
        keep(&files, edit, "of the edit").await,
        keep(&files, edit_of_edit, "of the edit of the edit").await,
        keep(&files, fork, "of the fork").await,
        keep(&files, other, "of another thread").await,
    );
    let mut stream = w.wakeup.subscribe();
    assert_eq!(app.purges_pending().await.unwrap(), 0);

    app.delete_thread(&alice(), parent).await.unwrap();

    // the thread and its edits are gone, the files with them, the purge rows finished
    for gone in [parent, edit, edit_of_edit] {
        assert!(matches!(
            app.get_thread(&alice(), gone).await,
            Err(AppError::NotFound)
        ));
        assert!(w.store.get_thread(None, gone).await.unwrap().is_none());
    }
    for key in [&k_parent, &k_edit, &k_edit2] {
        assert!(files.get(key).await.unwrap().is_none(), "{key}");
    }
    assert_eq!(app.purges_pending().await.unwrap(), 0);
    assert_eq!(app.delete_stats().threads_deleted, 3);
    // the fork stays whole, standing alone, with its own file, and takes its parent's place
    let kept = app.get_thread(&alice(), fork).await.unwrap();
    assert_eq!(
        kept.forked_from.map(|f| (f.thread_id, f.kind)),
        Some((None, ForkKind::Fork))
    );
    assert_eq!(kept.rail_parent, None);
    assert!(files.get(&k_fork).await.unwrap().is_some());
    assert!(files.get(&k_other).await.unwrap().is_some());
    let listed: Vec<ThreadId> = app
        .list_threads(&alice(), rail())
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert_eq!(
        listed,
        vec![other, fork],
        "the fork is a thread of the list now"
    );
    // the fork still reads: it was copied, nothing refers to its parent
    assert!(!log(&app, &alice(), fork).await.is_empty());
    // every thread deleted was announced, so open streams end
    let mut told = BTreeSet::new();
    while let Ok(Some(topic)) =
        tokio::time::timeout(Duration::from_millis(100), stream.next()).await
    {
        if let Topic::Thread(id) = topic {
            told.insert(id);
        }
    }
    assert!(
        [parent, edit, edit_of_edit]
            .iter()
            .all(|t| told.contains(t)),
        "{told:?}"
    );
    // a second delete is a 404
    assert!(matches!(
        app.delete_thread(&alice(), parent).await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn a_thread_that_works_is_refused_until_it_has_ended_and_a_blocked_one_can_go() {
    let w = World::new();
    let files = Files::default();
    let app = everyone(&w, files.clone());
    // queued: nobody has picked it up
    let queued = start(&app, &alice(), "echo hi").await;
    keep(&files, queued, "kept").await;
    let err = app.delete_thread(&alice(), queued).await.unwrap_err();
    assert!(
        matches!(
            err,
            AppError::ThreadActive(NotDeletable::Active(ThreadState::Queued))
        ),
        "{err:?}"
    );
    assert_eq!(err.class(), ErrorClass::Rejected);
    assert!(
        app.get_thread(&alice(), queued).await.is_ok(),
        "nothing was deleted"
    );
    assert_eq!(
        files.calls.load(Ordering::SeqCst),
        0,
        "nor touched in the store"
    );

    // working, then cancelled: the cancel is a row, the state stays until the agent reports
    let run = dispatch(&app);
    let slow = start(&app, &alice(), "slow job").await;
    settle(&app, slow, ThreadState::Working).await;
    let err = app.delete_thread(&alice(), slow).await.unwrap_err();
    assert!(
        matches!(
            err,
            AppError::ThreadActive(NotDeletable::Active(ThreadState::Working))
        ),
        "{err:?}"
    );
    app.cancel(&alice(), slow).await.unwrap();
    // right after the cancel the thread still works, and deleting it would cascade the cancel away
    if app.get_thread(&alice(), slow).await.unwrap().state == ThreadState::Working {
        assert!(matches!(
            app.delete_thread(&alice(), slow).await,
            Err(AppError::ThreadActive(_))
        ));
    }
    settle(&app, slow, ThreadState::Cancelled).await;
    app.delete_thread(&alice(), slow).await.unwrap();
    assert!(matches!(
        app.get_thread(&alice(), slow).await,
        Err(AppError::NotFound)
    ));

    // blocked: its turn is over, it waits for the person
    let blocked = start(&app, &alice(), "ui pick").await;
    settle(&app, blocked, ThreadState::Blocked).await;
    run.shutdown().await;
    app.delete_thread(&alice(), blocked).await.unwrap();
    assert!(matches!(
        app.get_thread(&alice(), blocked).await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn a_running_ask_refuses_the_delete_of_its_thread() {
    let w = World::new();
    let app = everyone(&w, Files::default());
    let t = start(&app, &alice(), "echo hi").await;
    // the agent's job asked another agent, which has not answered: the thread is done by the
    // state, and the ask runs on
    let mut record = w.store.get_thread(None, t).await.unwrap().unwrap();
    record.job.asks.push(orch_core::Ask {
        n: 1,
        by: orch_core::Caller::Main,
        agent: orch_core::AgentId::new("coder"),
        depth: 1,
        call_key: None,
        fingerprint: None,
        task_id: None,
        context_id: None,
        outcome: None,
    });
    let mut commit = orch_ports::Commit {
        new_state: ThreadState::Done,
        job: Some(record.job.clone()),
        events: vec![],
        outbox: vec![],
        binding: None,
        now: jiff::Timestamp::now(),
        lease: None,
        watches: vec![],
        timers: vec![],
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    };
    commit.now = jiff::Timestamp::now();
    w.store.commit(t, record.version, commit).await.unwrap();
    let err = app.delete_thread(&alice(), t).await.unwrap_err();
    assert!(
        matches!(err, AppError::ThreadActive(NotDeletable::AskRunning)),
        "{err:?}"
    );
}

#[tokio::test]
async fn deleting_needs_thread_delete_and_not_thread_write_and_is_the_owners_alone() {
    let w = World::new();
    let roles: BTreeMap<Role, RoleGrant> = [
        ("user", role(&Permission::ALL)),
        (
            "reader",
            role(&[Permission::ThreadRead, Permission::ThreadDelete]),
        ),
        (
            "keeper",
            role(&[
                Permission::ThreadRead,
                Permission::ThreadWrite,
                Permission::AgentInvoke,
            ]),
        ),
    ]
    .into_iter()
    .map(|(name, grant)| (Role::new(name), grant))
    .collect();
    let app = app_with(&w, Files::default(), Policy::new(roles, None).unwrap());
    let alice_user = principal("alice@example.com", &["user"]);
    let reader = principal("reader@example.com", &["reader"]);
    let keeper = principal("keeper@example.com", &["keeper"]);
    // the threads are made by an app of everybody's rights (the reader cannot write one)
    let maker = everyone(&w, Files::default());
    let mine = start(&maker, &alice(), "echo one").await;
    let readers = start(
        &maker,
        &orch_core::UserId::new("reader@example.com"),
        "echo r",
    )
    .await;
    let keepers = start(
        &maker,
        &orch_core::UserId::new("keeper@example.com"),
        "echo k",
    )
    .await;
    for id in [mine, readers, keepers] {
        settle_as(&maker, id).await;
    }

    // a role without thread.delete is refused, for its own thread and for any id alike
    for id in [keepers, mine, ThreadId(uuid::Uuid::now_v7())] {
        let err = app.delete_thread(&keeper, id).await.unwrap_err();
        assert!(
            matches!(&err, AppError::Forbidden { permission, .. } if *permission == Permission::ThreadDelete),
            "{err:?}"
        );
    }
    // another person's thread is a 404, whatever the roles; so is a thread that is not there
    for who in [&alice_user, &reader] {
        let other = if who.user == alice() { readers } else { mine };
        assert!(matches!(
            app.delete_thread(who, other).await,
            Err(AppError::NotFound)
        ));
        assert!(matches!(
            app.delete_thread(who, ThreadId(uuid::Uuid::now_v7())).await,
            Err(AppError::NotFound)
        ));
    }
    assert!(w.store.get_thread(None, mine).await.unwrap().is_some());
    assert!(w.store.get_thread(None, readers).await.unwrap().is_some());
    // a role that may only read erases its own thread: thread.delete needs no thread.write
    app.delete_thread(&reader, readers).await.unwrap();
    assert!(w.store.get_thread(None, readers).await.unwrap().is_none());
    app.delete_thread(&alice_user, mine).await.unwrap();
    assert!(w.store.get_thread(None, mine).await.unwrap().is_none());
}

/// A thread the dispatcher has not touched is `queued`: the cases that only need a thread to
/// exist make it one a person can delete by finishing its turn.
async fn settle_as<X: ArtifactStore>(app: &Arc<DApp<X>>, id: ThreadId) {
    let run = dispatch(app);
    eventually("the turn to end", || async {
        let t = app.ports().store.get_thread(None, id).await.ok()??;
        t.state.is_terminal().then_some(())
    })
    .await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_purge_that_fails_is_left_to_the_sweep_which_finishes_it() {
    let w = World::new();
    let files = Files::default();
    let app = everyone(&w, files.clone());
    let t = start(&app, &alice(), "echo one").await;
    settle_as(&app, t).await;
    let key = keep(&files, t, "to be erased").await;
    // the inline purge fails, and so does the sweep's first try
    files.failing.store(2, Ordering::SeqCst);

    // the person is told it is deleted: the log is gone, and the files are on their way
    app.delete_thread(&alice(), t).await.unwrap();
    assert!(w.store.get_thread(None, t).await.unwrap().is_none());
    assert!(
        files.get(&key).await.unwrap().is_some(),
        "the inline purge failed"
    );
    assert_eq!(app.purges_pending().await.unwrap(), 1);
    assert_eq!(app.delete_stats().threads_deleted, 1);

    let sweep = PurgeWorker::new(
        Arc::clone(&app),
        PurgeConfig {
            lease: Duration::from_millis(150),
            poll_interval: Duration::from_millis(10),
            batch: 4,
        },
        "w1",
    );
    // the sweep claims the row under a lease, fails and leaves it; the claim is held meanwhile
    assert_eq!(sweep.tick().await, 1);
    assert!(files.get(&key).await.unwrap().is_some());
    assert_eq!(app.purges_pending().await.unwrap(), 1);
    assert_eq!(w.store.purge_of(t), Some((1, Some("w1".to_owned()))));
    assert_eq!(
        sweep.tick().await,
        0,
        "a claim that has not lapsed is not taken again"
    );
    // the lease lapses and the next pass finishes it: the row is removed with the files
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(sweep.tick().await, 1);
    assert!(files.get(&key).await.unwrap().is_none());
    assert_eq!(app.purges_pending().await.unwrap(), 0);
    assert_eq!(w.store.purge_of(t), None);
    assert_eq!(sweep.tick().await, 0);
}

#[tokio::test]
async fn the_sweep_erases_what_a_crash_between_the_log_and_the_files_left() {
    let w = World::new();
    let files = Files::default();
    let app = everyone(&w, files.clone());
    let t = start(&app, &alice(), "echo one").await;
    settle_as(&app, t).await;
    let key = keep(&files, t, "left behind").await;
    // the transaction of a delete committed and the process died before it erased the files
    let version = w.store.get_thread(None, t).await.unwrap().unwrap().version;
    w.store
        .delete_threads(&alice(), &[(t, version)], jiff::Timestamp::now())
        .await
        .unwrap();
    assert!(files.get(&key).await.unwrap().is_some());
    assert_eq!(app.purges_pending().await.unwrap(), 1);

    let sweep = PurgeWorker::new(Arc::clone(&app), PurgeConfig::default(), "w1");
    assert_eq!(sweep.tick().await, 1);
    assert!(files.get(&key).await.unwrap().is_none());
    assert_eq!(app.purges_pending().await.unwrap(), 0);
    assert_eq!(sweep.tick().await, 0, "nothing is left to claim");
}

#[tokio::test]
async fn a_deployment_with_no_artifact_store_has_nothing_to_erase_and_finishes_the_purge() {
    let w = World::new();
    let app = everyone(&w, orch_ports::NoArtifacts);
    let t = start(&app, &alice(), "echo one").await;
    settle_as(&app, t).await;
    app.delete_thread(&alice(), t).await.unwrap();
    assert!(w.store.get_thread(None, t).await.unwrap().is_none());
    assert_eq!(
        app.purges_pending().await.unwrap(),
        0,
        "no row waits for files that never were"
    );
}

#[tokio::test]
async fn an_open_stream_of_a_deleted_thread_ends() {
    let w = World::new();
    let app = everyone(&w, Files::default());
    let t = start(&app, &alice(), "echo one").await;
    settle_as(&app, t).await;
    let mut stream = app.event_stream(&alice(), t, 0).await.unwrap();
    let first = stream.next().await.expect("the log replays");
    assert_eq!(first.seq, 1);
    // a thread-level wake and a poll tick find nothing to say while the thread is there
    let mut live = Box::pin(async move {
        let mut n = 1;
        while stream.next().await.is_some() {
            n += 1;
        }
        n
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(400), &mut live)
            .await
            .is_err(),
        "the stream follows a thread that is there"
    );

    app.delete_thread(&alice(), t).await.unwrap();
    let seen = tokio::time::timeout(Duration::from_secs(5), live)
        .await
        .expect("the stream ends when the thread is deleted");
    assert!(
        seen >= 4,
        "the rest of the log was read before it ended: {seen}"
    );
}

#[tokio::test]
async fn a_late_reply_for_a_deleted_thread_is_dropped_counted_and_never_retried() {
    let w = World::new();
    let files = Files::default();
    let app = everyone(&w, files);
    let run = dispatch(&app);
    // the agent is working on a thread whose row is deleted under it (as a delete does, here
    // without waiting for the end: the app refuses that, a store-level erasure does not)
    let t = start(&app, &alice(), "slow job").await;
    settle(&app, t, ThreadState::Working).await;
    let version = w.store.get_thread(None, t).await.unwrap().unwrap().version;
    w.store
        .delete_threads(&alice(), &[(t, version)], jiff::Timestamp::now())
        .await
        .unwrap();
    eventually("the worker to notice", || async {
        let stats = app.delete_stats();
        (stats.late_input_dropped[0].1 >= 1).then_some(())
    })
    .await;
    let stats = app.delete_stats();
    assert_eq!(stats.late_input_dropped[0], (LateSource::Dispatcher, 1));
    assert_eq!(stats.late_input_dropped[1], (LateSource::Inbox, 0));
    assert!(w.store.outbox_of(t).is_empty(), "nothing is left to retry");
    // and the dispatcher goes on: another thread is served
    let next = start(&app, &alice(), "echo next").await;
    settle(&app, next, ThreadState::Done).await;
    assert_eq!(
        app.delete_stats().late_input_dropped[0].1,
        1,
        "dropped once"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_late_inbox_row_for_a_deleted_thread_is_finished_and_counted() {
    let w = World::new();
    let app = everyone(&w, Files::default());
    // a timer row that names a thread that is gone: the race of a claim and a delete
    let gone = ThreadId(uuid::Uuid::now_v7());
    let row = NewInbox {
        id: InboxId(uuid::Uuid::now_v7()),
        source: orch_ports::TIMER_SOURCE.to_owned(),
        idempotency_key: format!("late-{gone}"),
        payload: InboxPayload::Timer {
            thread: gone,
            timer: Timer::CiDeadline {
                attempt: 1,
                verification: 1,
            },
        },
        correlation: None,
    };
    let id = row.id;
    w.store.receive(row, jiff::Timestamp::now()).await.unwrap();
    let worker = InboxWorker::new(Arc::clone(&app), InboxConfig::default(), "i1");

    assert_eq!(worker.tick().await, 1);

    // not a failure: the row is applied (finished), not retried and not dead
    let row = w.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Applied, "{row:?}");
    assert_eq!(
        app.delete_stats().late_input_dropped[1],
        (LateSource::Inbox, 1)
    );
    assert_eq!(worker.tick().await, 0, "never claimed again");
}
