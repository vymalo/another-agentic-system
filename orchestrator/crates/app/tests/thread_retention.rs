//! The thread retention sweep (`retention.threads.days`, default 90): an idle thread whose last
//! event is older than the retention is erased through the person's delete path; younger ones, and
//! threads that work, stay.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use orch_app::{
    App, AppConfig, DEFAULT_THREAD_RETENTION_DAYS, ThreadSweepConfig, ThreadSweepWorker,
};
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{Commit, NewEvent, NewThreadRecord, PortSet, RefuseAll, ThreadStore};
use support::{alice, bob, directory};
use uuid::Uuid;

type Ports = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    FixedClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    RefuseAll,
>;

const NOW: &str = "2026-10-28T12:00:00Z";

fn app(store: &MemoryStore, cfg: AppConfig) -> Arc<App<Ports>> {
    let directory = directory();
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: store.clone(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: FixedClock::new(NOW.parse().unwrap()),
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: RefuseAll,
                registry: directory.fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            cfg,
        )
        .expect("a valid configuration"),
    )
}

/// A thread of `owner` in `state` whose only event was `age_days` days before `NOW`.
async fn thread(
    store: &MemoryStore,
    owner: &UserId,
    state: ThreadState,
    age_days: i64,
) -> ThreadId {
    let id = ThreadId(Uuid::now_v7());
    let at: Timestamp =
        NOW.parse::<Timestamp>().unwrap() - SignedDuration::from_hours(24 * age_days);
    store
        .create_thread(
            NewThreadRecord {
                id,
                owner: owner.clone(),
                title: "t".into(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                child_of: None,
                now: at,
            },
            Commit {
                new_state: state,
                job: None,
                events: vec![NewEvent {
                    at,
                    actor: Actor::user(owner),
                    body: EventBody::UserMessage(UserMessageData::new("hi")),
                    idempotency_key: None,
                }],
                outbox: Vec::new(),
                binding: None,
                now: at,
                lease: None,
                watches: Vec::new(),
                timers: Vec::new(),
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: None,
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
    id
}

async fn exists(store: &MemoryStore, id: ThreadId) -> bool {
    store.get_thread(None, id).await.unwrap().is_some()
}

fn worker(app: &Arc<App<Ports>>) -> Arc<ThreadSweepWorker<Ports>> {
    ThreadSweepWorker::new(
        Arc::clone(app),
        ThreadSweepConfig {
            batch: 2,
            ..ThreadSweepConfig::default()
        },
    )
}

#[tokio::test]
async fn the_default_retention_is_ninety_days() {
    assert_eq!(DEFAULT_THREAD_RETENTION_DAYS, 90);
    assert_eq!(AppConfig::default().thread_retention_days, 90);

    let store = MemoryStore::new();
    let old_done = thread(&store, &alice(), ThreadState::Done, 91).await;
    let old_blocked = thread(&store, &alice(), ThreadState::Blocked, 120).await;
    let old_bob = thread(&store, &bob(), ThreadState::Failed, 400).await;
    let young = thread(&store, &alice(), ThreadState::Done, 89).await;
    let working = thread(&store, &alice(), ThreadState::Working, 300).await;
    let app = app(&store, AppConfig::default());
    assert_eq!(app.thread_retention_days(), 90);

    // a batch of 2 over 3 doomed threads: the sweep goes on to the next batch
    assert_eq!(worker(&app).sweep().await, 3);
    for gone in [old_done, old_blocked, old_bob] {
        assert!(!exists(&store, gone).await);
    }
    for kept in [young, working] {
        assert!(exists(&store, kept).await);
    }
    assert_eq!(worker(&app).sweep().await, 0, "nothing is left to erase");
    assert_eq!(app.delete_stats().threads_deleted, 3);
}

#[tokio::test]
async fn a_deployment_can_change_the_retention() {
    let store = MemoryStore::new();
    let gone = thread(&store, &alice(), ThreadState::Done, 31).await;
    let kept = thread(&store, &alice(), ThreadState::Done, 29).await;
    let app = app(
        &store,
        AppConfig {
            thread_retention_days: 30,
            ..AppConfig::default()
        },
    );
    assert_eq!(worker(&app).sweep().await, 1);
    assert!(!exists(&store, gone).await);
    assert!(exists(&store, kept).await);
}
