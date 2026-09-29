//! `ThreadStore` conformance cases. Each is `async fn(store)` on a fresh, isolated store.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    Actor, AgentId, AgentTarget, AgentTaskState, EventBody, ThreadId, ThreadState, UserId,
    UserMessageData,
};
use uuid::Uuid;

use crate::{
    BindingUpdate, Commit, CommitOutcome, NewEvent, NewOutbox, NewThreadRecord, OutboxFinal,
    OutboxId, OutboxKind, OutboxPayload, OutboxStatus, StoreError, ThreadStore,
};

const LEASE: Duration = Duration::from_secs(30);

/// The fixed time origin of the cases.
pub fn t0() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0().checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn thread_id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn outbox_id(n: u128) -> OutboxId {
    OutboxId(Uuid::from_u128(
        0x0190_0000_0000_7000_9000_0000_0000_0000 + n,
    ))
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn bob() -> UserId {
    UserId::new("bob@example.com")
}

fn new_thread(owner: &UserId, n: u128) -> NewThreadRecord {
    NewThreadRecord {
        id: thread_id(n),
        owner: owner.clone(),
        title: format!("thread {n}"),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        context_id: format!("ctx-{n}"),
        now: t0(),
    }
}

fn user_event(text: &str, key: Option<&str>) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UserMessage(UserMessageData { text: text.into() }),
        idempotency_key: key.map(str::to_owned),
    }
}

fn delegate(n: u128) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Delegate {
            text: format!("do {n}"),
            release: None,
        },
    }
}

fn cancel_row(n: u128) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Cancel,
    }
}

fn commit(state: ThreadState, events: Vec<NewEvent>, outbox: Vec<NewOutbox>) -> Commit {
    Commit {
        new_state: state,
        events,
        outbox,
        binding: None,
        now: t0(),
    }
}

/// Creates a thread with one user event and one delegate row (outbox id = thread number).
async fn seed<S: ThreadStore>(store: &S, owner: &UserId, n: u128) {
    store
        .create_thread(
            new_thread(owner, n),
            commit(
                ThreadState::Queued,
                vec![user_event("hi", None)],
                vec![delegate(n)],
            ),
        )
        .await
        .unwrap();
}

async fn claim<S: ThreadStore>(store: &S, owner: &str, now: Timestamp) -> Vec<crate::OutboxItem> {
    store.claim_outbox(owner, now, LEASE, 100).await.unwrap()
}

fn applied(outcome: CommitOutcome) -> (orch_core::ThreadRecord, Vec<orch_core::Event>) {
    match outcome {
        CommitOutcome::Applied { thread, events } => (thread, events),
        CommitOutcome::Duplicate => panic!("unexpected Duplicate"),
    }
}

pub async fn ping<S: ThreadStore>(store: S) {
    store.ping().await.unwrap();
}

pub async fn create_get_roundtrip<S: ThreadStore>(store: S) {
    let (record, events) = store
        .create_thread(
            new_thread(&alice(), 1),
            commit(
                ThreadState::Queued,
                vec![user_event("hi", None)],
                vec![delegate(1)],
            ),
        )
        .await
        .unwrap();
    assert_eq!(record.id, thread_id(1));
    assert_eq!(record.owner, alice());
    assert_eq!(record.state, ThreadState::Queued);
    assert_eq!(record.version, 1);
    assert_eq!(record.last_seq, 1);
    assert_eq!(record.created_at, t0());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[0].thread_id, thread_id(1));

    let got = store
        .get_thread(Some(&alice()), thread_id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, record);
    assert_eq!(
        store.list_events(thread_id(1), 0, 10).await.unwrap(),
        events
    );

    let binding = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert_eq!(binding.agent_id, AgentId::new("coder"));
    assert_eq!(binding.context_id, "ctx-1");
    assert_eq!(binding.task_id, None);
    assert_eq!(binding.task_state, None);

    assert!(
        store
            .get_thread(None, thread_id(999))
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_binding(thread_id(999)).await.unwrap().is_none());
    assert_eq!(store.list_open_outbox(thread_id(1)).await.unwrap().len(), 1);
}

pub async fn owner_isolation<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    assert!(
        store
            .get_thread(Some(&bob()), thread_id(1))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_thread(Some(&alice()), thread_id(1))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_thread(None, thread_id(1))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .list_threads(&bob(), None, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.list_threads(&alice(), None, 50).await.unwrap().len(),
        1
    );
}

pub async fn list_newest_first_before_limit<S: ThreadStore>(store: S) {
    for n in 1..=5 {
        seed(&store, &alice(), n).await;
    }
    seed(&store, &bob(), 6).await;
    let ids = |v: Vec<orch_core::ThreadRecord>| v.into_iter().map(|t| t.id).collect::<Vec<_>>();
    let all = ids(store.list_threads(&alice(), None, 50).await.unwrap());
    assert_eq!(
        all,
        vec![
            thread_id(5),
            thread_id(4),
            thread_id(3),
            thread_id(2),
            thread_id(1)
        ]
    );
    let page = ids(store.list_threads(&alice(), None, 2).await.unwrap());
    assert_eq!(page, vec![thread_id(5), thread_id(4)]);
    let next = ids(store
        .list_threads(&alice(), Some(thread_id(3)), 50)
        .await
        .unwrap());
    assert_eq!(next, vec![thread_id(2), thread_id(1)]);
    // A foreign or unknown cursor yields nothing.
    assert!(
        store
            .list_threads(&alice(), Some(thread_id(6)), 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_threads(&alice(), Some(thread_id(77)), 50)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn commit_contiguous_seq<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let (record, events) = applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![user_event("a", None), user_event("b", None)],
                    vec![],
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
    assert_eq!(record.version, 2);
    assert_eq!(record.last_seq, 3);
    assert_eq!(record.state, ThreadState::Working);
    let (record, events) = applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Working, vec![user_event("c", None)], vec![]),
            )
            .await
            .unwrap(),
    );
    assert_eq!(events[0].seq, 4);
    assert_eq!(record.version, 3);
    let read = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(read, record);
    // A commit without events still bumps the version and keeps last_seq.
    let (record, events) = applied(
        store
            .commit(
                thread_id(1),
                3,
                commit(ThreadState::Blocked, vec![], vec![]),
            )
            .await
            .unwrap(),
    );
    assert!(events.is_empty());
    assert_eq!(
        (record.version, record.last_seq, record.state),
        (4, 4, ThreadState::Blocked)
    );
    assert!(matches!(
        store
            .commit(
                thread_id(404),
                1,
                commit(ThreadState::Working, vec![], vec![])
            )
            .await,
        Err(StoreError::NotFound)
    ));
}

pub async fn version_conflict_writes_nothing<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    let mut c = commit(
        ThreadState::Done,
        vec![user_event("x", Some("k"))],
        vec![delegate(50)],
    );
    c.binding = Some(BindingUpdate {
        task_id: Some("t".into()),
        ..BindingUpdate::default()
    });
    let res = store.commit(thread_id(1), 99, c).await;
    assert!(matches!(res, Err(StoreError::VersionConflict)), "{res:?}");
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store.list_events(thread_id(1), 0, 10).await.unwrap().len(),
        1
    );
    assert_eq!(store.list_open_outbox(thread_id(1)).await.unwrap().len(), 1);
    assert_eq!(
        store
            .get_binding(thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .task_id,
        None
    );
}

pub async fn duplicate_key_writes_nothing<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    let first = store
        .commit(
            thread_id(1),
            1,
            commit(
                ThreadState::Working,
                vec![user_event("x", Some("k1"))],
                vec![],
            ),
        )
        .await
        .unwrap();
    let (record, _) = applied(first);
    assert_eq!(record.version, 2);
    let mut again = commit(
        ThreadState::Done,
        vec![user_event("y", Some("fresh")), user_event("x", Some("k1"))],
        vec![delegate(60)],
    );
    again.binding = Some(BindingUpdate {
        task_id: Some("t".into()),
        ..BindingUpdate::default()
    });
    assert_eq!(
        store.commit(thread_id(1), 2, again).await.unwrap(),
        CommitOutcome::Duplicate
    );
    let after = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(after, record, "nothing may change on Duplicate");
    assert_eq!(
        store.list_events(thread_id(1), 0, 10).await.unwrap().len(),
        2
    );
    assert_eq!(store.list_open_outbox(thread_id(1)).await.unwrap().len(), 1);
    assert_eq!(
        store
            .get_binding(thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .task_id,
        None
    );
    // The key is scoped to the thread.
    let other = store
        .commit(
            thread_id(2),
            1,
            commit(
                ThreadState::Working,
                vec![user_event("x", Some("k1"))],
                vec![],
            ),
        )
        .await
        .unwrap();
    assert!(matches!(other, CommitOutcome::Applied { .. }));
}

pub async fn concurrent_writers_keep_seq_contiguous<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for i in 0..20 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            loop {
                let t = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
                let c = commit(
                    ThreadState::Working,
                    vec![user_event(&format!("w{i}"), Some(&format!("key-{i}")))],
                    vec![],
                );
                match store.commit(thread_id(1), t.version, c).await {
                    Ok(CommitOutcome::Applied { .. }) => return,
                    Ok(CommitOutcome::Duplicate) => panic!("distinct keys must not collide"),
                    Err(StoreError::VersionConflict) => tokio::task::yield_now().await,
                    Err(e) => panic!("{e}"),
                }
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let events = store.list_events(thread_id(1), 0, 100).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=21).collect::<Vec<i64>>()
    );
    let record = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(record.last_seq, 21);
    assert_eq!(record.version, 21);
    let mut texts: Vec<String> = events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::UserMessage(m) => Some(m.text.clone()),
            _ => None,
        })
        .collect();
    texts.sort();
    texts.dedup();
    assert_eq!(texts.len(), 21, "no duplicated events");
}

pub async fn list_events_after_limit<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let events: Vec<NewEvent> = (0..4).map(|i| user_event(&format!("e{i}"), None)).collect();
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Working, events, vec![]),
            )
            .await
            .unwrap(),
    );
    let seqs = |v: Vec<orch_core::Event>| v.into_iter().map(|e| e.seq).collect::<Vec<_>>();
    assert_eq!(
        seqs(store.list_events(thread_id(1), 0, 100).await.unwrap()),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        seqs(store.list_events(thread_id(1), 2, 2).await.unwrap()),
        vec![3, 4]
    );
    assert_eq!(
        seqs(store.list_events(thread_id(1), 4, 100).await.unwrap()),
        vec![5]
    );
    assert!(
        store
            .list_events(thread_id(1), 5, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_events(thread_id(404), 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

pub async fn claim_once_and_concurrent_claimers<S: ThreadStore>(store: S) {
    for n in 1..=6 {
        seed(&store, &alice(), n).await;
    }
    let first = claim(&store, "a", t0()).await;
    assert_eq!(first.len(), 6);
    assert!(
        first
            .iter()
            .all(|r| r.attempts == 1 && r.status == OutboxStatus::Inflight)
    );
    assert!(first.iter().all(|r| r.lease_owner.as_deref() == Some("a")));
    assert!(
        first
            .iter()
            .all(|r| r.kind == OutboxKind::Delegate && r.sent_at.is_none())
    );
    assert!(claim(&store, "b", t0()).await.is_empty(), "already leased");
    // `limit` is honoured and rows come oldest first.
    for n in 11..=13 {
        seed(&store, &alice(), n).await;
    }
    let some = store.claim_outbox("a", t0(), LEASE, 2).await.unwrap();
    assert_eq!(
        some.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(11), outbox_id(12)]
    );

    // Two concurrent claimers never share a row.
    for n in 21..=40 {
        seed(&store, &alice(), n).await;
    }
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for who in ["c1", "c2", "c3"] {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let mut mine = Vec::new();
            loop {
                let got = store.claim_outbox(who, t0(), LEASE, 3).await.unwrap();
                if got.is_empty() {
                    return mine;
                }
                mine.extend(got.into_iter().map(|r| r.id));
            }
        }));
    }
    let mut all = Vec::new();
    for t in tasks {
        all.extend(t.await.unwrap());
    }
    // 20 new rows plus the one left over from 11..=13.
    assert_eq!(all.len(), 21);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 21, "a row was claimed twice");
}

pub async fn lease_expiry_reclaim<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let got = claim(&store, "a", t0()).await;
    assert_eq!(got.len(), 1);
    assert!(
        claim(&store, "b", at(29)).await.is_empty(),
        "lease still valid"
    );
    let again = claim(&store, "b", at(31)).await;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].attempts, 2);
    assert_eq!(again[0].lease_owner.as_deref(), Some("b"));
    // The old owner lost the row.
    assert!(!store.renew_lease(outbox_id(1), "a", at(120)).await.unwrap());
    assert!(
        !store
            .mark_sent(outbox_id(1), "a", BindingUpdate::default(), at(32))
            .await
            .unwrap()
    );
    assert!(
        !store
            .retry_outbox(outbox_id(1), "a", at(40), "x".into())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_outbox(outbox_id(1), "a", OutboxFinal::Delivered, at(32))
            .await
            .unwrap()
    );
    // The new owner can renew, which keeps others out.
    assert!(store.renew_lease(outbox_id(1), "b", at(100)).await.unwrap());
    assert!(claim(&store, "c", at(90)).await.is_empty());
    assert_eq!(claim(&store, "c", at(101)).await.len(), 1);
}

pub async fn delegate_ordering_per_thread<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Queued, vec![], vec![delegate(101)]),
            )
            .await
            .unwrap(),
    );
    // One delegate per thread per call, even though 3 rows are due.
    let got = claim(&store, "a", t0()).await;
    let ids: Vec<_> = got.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![outbox_id(1), outbox_id(2)]);
    assert!(
        claim(&store, "a", t0()).await.is_empty(),
        "second delegate waits for the first"
    );
    // A cancel row is not held back by the inflight delegate.
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Queued, vec![], vec![cancel_row(102)]),
            )
            .await
            .unwrap(),
    );
    let cancel = claim(&store, "a", t0()).await;
    assert_eq!(cancel.len(), 1);
    assert_eq!(cancel[0].kind, OutboxKind::Cancel);
    assert_eq!(cancel[0].id, outbox_id(102));
    // Completing the first delegate releases the second.
    assert!(
        store
            .complete_outbox(outbox_id(1), "a", OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    let next = claim(&store, "a", t0()).await;
    assert_eq!(
        next.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(101)]
    );
}

pub async fn retry_not_claimable_before_due<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    claim(&store, "a", t0()).await;
    assert!(
        !store
            .retry_outbox(outbox_id(1), "wrong", at(10), "e".into())
            .await
            .unwrap()
    );
    assert!(
        store
            .retry_outbox(outbox_id(1), "a", at(10), "boom".into())
            .await
            .unwrap()
    );
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, OutboxStatus::Pending);
    assert_eq!(row.last_error.as_deref(), Some("boom"));
    assert_eq!(row.lease_owner, None);
    assert!(claim(&store, "b", at(5)).await.is_empty());
    let got = claim(&store, "b", at(10)).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].attempts, 2);
    assert_eq!(got[0].last_error.as_deref(), Some("boom"));
}

pub async fn complete_outcomes<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    claim(&store, "a", t0()).await;
    assert!(
        !store
            .complete_outbox(outbox_id(1), "other", OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(outbox_id(1), "a", OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(
                outbox_id(2),
                "a",
                OutboxFinal::Dead {
                    error: "gave up".into()
                },
                t0()
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(outbox_id(3), "a", OutboxFinal::Skipped, t0())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_outbox(outbox_id(1), "a", OutboxFinal::Delivered, t0())
            .await
            .unwrap(),
        "already final"
    );
    let status = |n| {
        let store = &store;
        async move { store.get_outbox(outbox_id(n)).await.unwrap().unwrap() }
    };
    assert_eq!(status(1).await.status, OutboxStatus::Delivered);
    let dead = status(2).await;
    assert_eq!(dead.status, OutboxStatus::Dead);
    assert_eq!(dead.last_error.as_deref(), Some("gave up"));
    assert_eq!(status(3).await.status, OutboxStatus::Skipped);
    assert!(claim(&store, "b", at(100_000)).await.is_empty());
    for n in 1..=3 {
        assert!(
            store
                .list_open_outbox(thread_id(n))
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(store.get_outbox(outbox_id(999)).await.unwrap().is_none());
}

pub async fn mark_sent_is_atomic<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    claim(&store, "a", t0()).await;
    let update = BindingUpdate {
        task_id: Some("task-1".into()),
        task_state: Some(AgentTaskState::Working),
        revision: Some("rev-1".into()),
    };
    assert!(
        !store
            .mark_sent(outbox_id(1), "other", update.clone(), at(1))
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .get_binding(thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .task_id,
        None
    );
    assert!(
        store
            .get_outbox(outbox_id(1))
            .await
            .unwrap()
            .unwrap()
            .sent_at
            .is_none()
    );
    assert!(
        store
            .mark_sent(outbox_id(1), "a", update, at(1))
            .await
            .unwrap()
    );
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.sent_at, Some(at(1)));
    assert_eq!(row.status, OutboxStatus::Inflight);
    let b = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert_eq!(b.task_id.as_deref(), Some("task-1"));
    assert_eq!(b.task_state, Some(AgentTaskState::Working));
    assert_eq!(b.revision.as_deref(), Some("rev-1"));
    assert_eq!(b.context_id, "ctx-1");
}

pub async fn skip_unsent_delegates<S: ThreadStore>(store: S) {
    // Thread 1: a pending delegate and a cancel row.
    seed(&store, &alice(), 1).await;
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Queued, vec![], vec![cancel_row(102)]),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(1), t0())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .get_outbox(outbox_id(1))
            .await
            .unwrap()
            .unwrap()
            .status,
        OutboxStatus::Skipped
    );
    assert_eq!(
        store
            .get_outbox(outbox_id(102))
            .await
            .unwrap()
            .unwrap()
            .status,
        OutboxStatus::Pending
    );
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(1), t0())
            .await
            .unwrap(),
        0
    );

    // Thread 2: inflight, unsent, lease valid -> a live worker owns it; not skipped.
    seed(&store, &alice(), 2).await;
    let got = store.claim_outbox("a", t0(), LEASE, 1).await.unwrap();
    // The oldest claimable row is thread 1's cancel row, so claim again for thread 2's delegate.
    let mut rows = got;
    rows.extend(store.claim_outbox("a", t0(), LEASE, 1).await.unwrap());
    assert!(rows.iter().any(|r| r.id == outbox_id(2)));
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(2), at(10))
            .await
            .unwrap(),
        0
    );
    // Once the lease expired nobody live owns it -> skipped.
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(2), at(60))
            .await
            .unwrap(),
        1
    );

    // Thread 3: sent delegates are never skipped.
    seed(&store, &alice(), 3).await;
    let rows = claim(&store, "b", at(100)).await;
    assert!(rows.iter().any(|r| r.id == outbox_id(3)));
    assert!(
        store
            .mark_sent(outbox_id(3), "b", BindingUpdate::default(), at(100))
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(3), at(1000))
            .await
            .unwrap(),
        0
    );
}

pub async fn release_leases<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    assert_eq!(claim(&store, "a", t0()).await.len(), 2);
    assert_eq!(store.release_leases("nobody", at(1)).await.unwrap(), 0);
    assert!(claim(&store, "b", at(2)).await.is_empty(), "still leased");
    assert_eq!(store.release_leases("a", at(1)).await.unwrap(), 2);
    let got = claim(&store, "b", at(2)).await;
    assert_eq!(got.len(), 2);
    assert!(
        got.iter()
            .all(|r| r.attempts == 2 && r.lease_owner.as_deref() == Some("b"))
    );
}

pub async fn binding_applied_with_commit<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let mut c = commit(ThreadState::Blocked, vec![user_event("q", None)], vec![]);
    c.binding = Some(BindingUpdate {
        task_id: Some("t9".into()),
        task_state: Some(AgentTaskState::InputRequired),
        revision: Some("r".into()),
    });
    applied(store.commit(thread_id(1), 1, c).await.unwrap());
    let b = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert_eq!(b.task_id.as_deref(), Some("t9"));
    assert_eq!(b.task_state, Some(AgentTaskState::InputRequired));
    assert_eq!(b.revision.as_deref(), Some("r"));
    // A partial update leaves the other fields alone.
    let mut c = commit(ThreadState::Working, vec![], vec![]);
    c.binding = Some(BindingUpdate {
        task_state: Some(AgentTaskState::Working),
        ..BindingUpdate::default()
    });
    applied(store.commit(thread_id(1), 2, c).await.unwrap());
    let b = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert_eq!(b.task_id.as_deref(), Some("t9"));
    assert_eq!(b.task_state, Some(AgentTaskState::Working));
    assert_eq!(b.revision.as_deref(), Some("r"));
}
