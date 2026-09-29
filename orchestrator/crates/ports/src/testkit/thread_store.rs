//! `ThreadStore` conformance cases. Each is `async fn(store)` on a fresh, isolated store.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentTarget, AgentTaskState, Classify,
    ErrorClass, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use uuid::Uuid;

use crate::{
    BindingUpdate, Commit, CommitOutcome, Lease, NewEvent, NewOutbox, NewThreadRecord, OutboxFinal,
    OutboxId, OutboxKind, OutboxPayload, OutboxStats, OutboxStatus, StoreError, ThreadStore,
};

/// The class of the error, if any: cases assert classes, never concrete variants or sources.
fn class_of<T>(res: &Result<T, StoreError>) -> Option<ErrorClass> {
    res.as_ref().err().map(Classify::class)
}

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
        body: EventBody::UserMessage(UserMessageData::new(text)),
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
        lease: None,
    }
}

/// The claim of outbox row `n` that `owner` got at its `attempt`-th claim.
fn lease(n: u128, owner: &str, attempt: u32) -> Lease {
    Lease {
        id: outbox_id(n),
        owner: owner.to_owned(),
        attempt,
    }
}

/// `c` made under `lease`.
fn under(mut c: Commit, lease: Lease) -> Commit {
    c.lease = Some(lease);
    c
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
        CommitOutcome::Fenced => panic!("unexpected Fenced"),
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

/// The event codec: optional `data` members and every status spelling come back as written.
/// A message id or run id that was never set stays absent (`None`), not an empty string.
pub async fn event_data_roundtrip<S: ThreadStore>(store: S) {
    let named = NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UserMessage(UserMessageData {
            text: "with ids".into(),
            message_id: Some("msg-1".into()),
            run_id: Some("run-1".into()),
        }),
        idempotency_key: None,
    };
    let only_run = NewEvent {
        body: EventBody::UserMessage(UserMessageData {
            text: "run only".into(),
            message_id: None,
            run_id: Some("run-2".into()),
        }),
        ..user_event("unused", None)
    };
    let auth = NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), Some("rev-1".into())),
        body: EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::AuthRequired,
            detail: Some("github".into()),
        }),
        idempotency_key: None,
    };
    let auth_bare = NewEvent {
        body: EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::AuthRequired,
            detail: None,
        }),
        ..auth.clone()
    };
    // A2UI (ADR 0013): the two additive kinds, and the delegation of an action.
    let surface = NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::UiSurface(orch_core::UiSurfaceData {
            operations: vec![
                serde_json::json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}}),
            ],
        }),
        idempotency_key: None,
    };
    let action = ui_action();
    let wanted: Vec<NewEvent> = vec![
        named,
        user_event("no ids", None),
        only_run,
        auth,
        auth_bare,
        surface,
        NewEvent {
            at: t0(),
            actor: Actor::user(&alice()),
            body: EventBody::UiAction(action.clone()),
            idempotency_key: None,
        },
    ];
    let bodies: Vec<EventBody> = wanted.iter().map(|e| e.body.clone()).collect();
    let action_row = NewOutbox {
        id: outbox_id(2),
        payload: OutboxPayload::Action {
            action,
            at: t0(),
            release: Some("staging".into()),
        },
    };
    let rows = vec![delegate(1), action_row.clone()];
    store
        .create_thread(
            new_thread(&alice(), 1),
            commit(ThreadState::Queued, wanted, rows),
        )
        .await
        .unwrap();

    let read = store.list_events(thread_id(1), 0, 10).await.unwrap();
    assert_eq!(
        read.iter().map(|e| e.body.clone()).collect::<Vec<_>>(),
        bodies
    );
    assert_eq!(
        read.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    // The wire form the API serves is what the store returned: no null, camelCase ids.
    let data: Vec<serde_json::Value> = read.iter().map(|e| e.body.data_value()).collect();
    assert_eq!(
        data[0],
        serde_json::json!({"text": "with ids", "messageId": "msg-1", "runId": "run-1"})
    );
    assert_eq!(data[1], serde_json::json!({"text": "no ids"}));
    assert_eq!(
        data[3],
        serde_json::json!({"status": "auth_required", "detail": "github"})
    );
    assert_eq!(data[4], serde_json::json!({"status": "auth_required"}));
    assert_eq!(
        data[5],
        serde_json::json!({"operations": [
            {"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}}]})
    );
    assert_eq!(
        data[6],
        serde_json::json!({"surfaceId": "s1", "name": "go", "sourceComponentId": "btn",
                           "context": {"choice": "a"}, "version": "v0.9.1", "runId": "run-3"})
    );
    // The delegation of an action keeps its payload, and is a `delegate` row.
    let open = store.list_open_outbox(thread_id(1)).await.unwrap();
    let stored = open.iter().find(|r| r.id == outbox_id(2)).unwrap();
    assert_eq!(stored.payload, action_row.payload);
    assert_eq!(stored.kind, OutboxKind::Delegate);
}

fn ui_action() -> orch_core::UiActionData {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), serde_json::json!("a"));
    orch_core::UiActionData {
        surface_id: "s1".into(),
        name: "go".into(),
        source_component_id: "btn".into(),
        context,
        version: orch_core::UiVersion::V0_9_1,
        run_id: Some("run-3".into()),
    }
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
    let res = store
        .commit(
            thread_id(404),
            1,
            commit(ThreadState::Working, vec![], vec![]),
        )
        .await;
    assert_eq!(class_of(&res), Some(ErrorClass::NotFound), "{res:?}");
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
    assert_eq!(class_of(&res), Some(ErrorClass::Conflict), "{res:?}");
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
                    Ok(CommitOutcome::Fenced) => panic!("no lease was given"),
                    Err(e) if e.class() == ErrorClass::Conflict => tokio::task::yield_now().await,
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
    assert!(!store.renew_lease(&lease(1, "a", 1), at(120)).await.unwrap());
    assert!(
        !store
            .mark_sent(&lease(1, "a", 1), BindingUpdate::default(), at(32))
            .await
            .unwrap()
    );
    assert!(
        !store
            .retry_outbox(&lease(1, "a", 1), at(40), "x".into())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, at(32))
            .await
            .unwrap()
    );
    assert_fenced(&store, 1, lease(1, "a", 1)).await;
    // The new owner can renew, which keeps others out.
    assert!(store.renew_lease(&lease(1, "b", 2), at(100)).await.unwrap());
    assert!(claim(&store, "c", at(90)).await.is_empty());
    assert_eq!(claim(&store, "c", at(101)).await.len(), 1);
}

/// Asserts that a commit under `stale` is refused and writes nothing to thread `n`.
async fn assert_fenced<S: ThreadStore>(store: &S, n: u128, stale: Lease) {
    let before = store.get_thread(None, thread_id(n)).await.unwrap().unwrap();
    let events = store.list_events(thread_id(n), 0, 100).await.unwrap();
    let binding = store.get_binding(thread_id(n)).await.unwrap().unwrap();
    let open = store.list_open_outbox(thread_id(n)).await.unwrap();
    let mut late = under(
        commit(
            ThreadState::Done,
            vec![user_event("late", Some("late-key"))],
            vec![delegate(900 + u128::from(stale.attempt))],
        ),
        stale,
    );
    late.binding = Some(BindingUpdate {
        task_id: Some("late-task".into()),
        ..BindingUpdate::default()
    });
    assert_eq!(
        store
            .commit(thread_id(n), before.version, late)
            .await
            .unwrap(),
        CommitOutcome::Fenced
    );
    assert_eq!(
        store.get_thread(None, thread_id(n)).await.unwrap().unwrap(),
        before,
        "a fenced commit changes nothing"
    );
    assert_eq!(
        store.list_events(thread_id(n), 0, 100).await.unwrap(),
        events
    );
    assert_eq!(
        store.get_binding(thread_id(n)).await.unwrap().unwrap(),
        binding
    );
    assert_eq!(store.list_open_outbox(thread_id(n)).await.unwrap(), open);
}

pub async fn stale_attempt_is_fenced<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    // "a" claims, the lease lapses, and the same name claims again: only the attempt differs.
    let first = claim(&store, "a", t0()).await;
    assert_eq!(first[0].lease(), Some(lease(1, "a", 1)));
    let second = claim(&store, "a", at(31)).await;
    assert_eq!(second[0].lease(), Some(lease(1, "a", 2)));
    let stale = lease(1, "a", 1);
    assert!(!store.renew_lease(&stale, at(200)).await.unwrap());
    assert!(
        !store
            .mark_sent(&stale, BindingUpdate::default(), at(32))
            .await
            .unwrap()
    );
    assert!(
        !store
            .retry_outbox(&stale, at(40), "x".into())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_outbox(&stale, OutboxFinal::Delivered, at(32))
            .await
            .unwrap()
    );
    assert_fenced(&store, 1, stale.clone()).await;
    // The row is still the second claim's, untouched by the stale calls.
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, OutboxStatus::Inflight);
    assert_eq!(row.attempts, 2);
    assert_eq!(row.lease_until, Some(at(61)));
    assert!(row.sent_at.is_none() && row.last_error.is_none());
    // A lease is only good for the thread its row belongs to.
    assert_fenced(&store, 2, lease(1, "a", 2)).await;
    // The current claim works.
    let current = lease(1, "a", 2);
    assert!(store.renew_lease(&current, at(100)).await.unwrap());
    let c = under(
        commit(
            ThreadState::Working,
            vec![user_event("now", Some("now-key"))],
            vec![],
        ),
        current,
    );
    let (record, events) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!((record.version, events.len()), (2, 1));
}

pub async fn commit_after_another_owner_reclaims_is_fenced<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    assert_eq!(claim(&store, "a", t0()).await.len(), 1);
    assert_eq!(claim(&store, "b", at(31)).await.len(), 1);
    assert_fenced(&store, 1, lease(1, "a", 1)).await;
    // The new owner commits, and stays the owner afterwards.
    let c = under(
        commit(ThreadState::Working, vec![user_event("b", None)], vec![]),
        lease(1, "b", 2),
    );
    applied(store.commit(thread_id(1), 1, c).await.unwrap());
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.lease(), Some(lease(1, "b", 2)));
    // An owner name is not enough: "b" at the wrong attempt is fenced too.
    assert_fenced(&store, 1, lease(1, "b", 1)).await;
}

pub async fn commit_after_complete_is_fenced<S: ThreadStore>(store: S) {
    // Finished: delivered, dead or skipped rows hold no claim.
    for (n, outcome) in [
        (1, OutboxFinal::Delivered),
        (
            2,
            OutboxFinal::Dead {
                error: "gave up".into(),
            },
        ),
        (3, OutboxFinal::Skipped),
    ] {
        seed(&store, &alice(), n).await;
        let held = claim(&store, "a", at(0)).await;
        assert_eq!(held.len(), 1, "row {n}");
        let l = held[0].lease().unwrap();
        assert!(store.complete_outbox(&l, outcome, t0()).await.unwrap());
        assert_fenced(&store, n, l).await;
    }
    // Sent back to pending for a retry: the claim is over as well.
    seed(&store, &alice(), 4).await;
    let held = claim(&store, "a", t0()).await;
    let l = held[0].lease().unwrap();
    assert!(
        store
            .retry_outbox(&l, at(1000), "later".into())
            .await
            .unwrap()
    );
    assert_fenced(&store, 4, l).await;
    // Skipped by a cancel while nobody renewed the claim.
    seed(&store, &alice(), 5).await;
    let held = claim(&store, "a", t0()).await;
    let l = held[0].lease().unwrap();
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(5), at(60))
            .await
            .unwrap(),
        1
    );
    assert_fenced(&store, 5, l).await;
}

pub async fn expired_unclaimed_lease_still_commits<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let held = claim(&store, "a", t0()).await;
    let l = held[0].lease().unwrap();
    // The lease lapsed at 30 s, but nobody took the row over: the claim is still the current
    // one, and its work is not thrown away.
    let mut c = under(
        commit(ThreadState::Working, vec![user_event("late", None)], vec![]),
        l.clone(),
    );
    c.now = at(10_000);
    let (record, events) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!((record.version, events.len()), (2, 1));
    assert!(
        store
            .mark_sent(&l, BindingUpdate::default(), at(10_001))
            .await
            .unwrap()
    );
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
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
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
            .retry_outbox(&lease(1, "wrong", 1), at(10), "e".into())
            .await
            .unwrap()
    );
    assert!(
        store
            .retry_outbox(&lease(1, "a", 1), at(10), "boom".into())
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
            .complete_outbox(&lease(1, "other", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(
                &lease(2, "a", 1),
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
            .complete_outbox(&lease(3, "a", 1), OutboxFinal::Skipped, t0())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
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
            .mark_sent(&lease(1, "other", 1), update.clone(), at(1))
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
            .mark_sent(&lease(1, "a", 1), update, at(1))
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
            .mark_sent(&lease(3, "b", 1), BindingUpdate::default(), at(100))
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

pub async fn outbox_stats<S: ThreadStore>(store: S) {
    let stats = |due, waiting, leased, oldest_due_at| OutboxStats {
        due,
        waiting,
        leased,
        oldest_due_at,
    };

    assert_eq!(
        store.outbox_stats(at(5)).await.unwrap(),
        OutboxStats::default(),
        "an empty outbox counts nothing and has no oldest due row"
    );

    // Three delegate rows, all due since t0.
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    assert_eq!(
        store.outbox_stats(at(5)).await.unwrap(),
        stats(3, 0, 0, Some(t0()))
    );

    // Row 1 is claimed (lease until at(35)); row 2 is claimed and put back in backoff.
    let first = store.claim_outbox("a", at(5), LEASE, 1).await.unwrap();
    assert_eq!(first[0].id, outbox_id(1));
    let second = store.claim_outbox("a", at(5), LEASE, 1).await.unwrap();
    assert_eq!(second[0].id, outbox_id(2));
    assert!(
        store
            .retry_outbox(&lease(2, "a", 1), at(1000), "later".into())
            .await
            .unwrap()
    );
    assert_eq!(
        store.outbox_stats(at(6)).await.unwrap(),
        stats(1, 1, 1, Some(t0())),
        "row 3 is due, row 2 waits, row 1 is leased"
    );

    // Row 3 is claimed too (lease until at(36)). The lease of row 1 lapses at at(35): from
    // then on the row is due again, since when its lease lapsed.
    let third = store.claim_outbox("c", at(6), LEASE, 1).await.unwrap();
    assert_eq!(third[0].id, outbox_id(3));
    assert_eq!(
        store.outbox_stats(at(34)).await.unwrap(),
        stats(0, 1, 2, None)
    );
    assert_eq!(
        store.outbox_stats(at(35)).await.unwrap(),
        stats(1, 1, 1, Some(at(35))),
        "a lease is live only until `lease_until`, exclusive"
    );
    assert_eq!(
        store.outbox_stats(at(36)).await.unwrap(),
        stats(2, 1, 0, Some(at(35))),
        "the oldest due row is the one whose lease lapsed first"
    );

    // Finishing a row removes it from every count.
    let both = claim(&store, "b", at(36)).await;
    assert_eq!(both.len(), 2, "rows 1 and 3; row 2 is still in backoff");
    assert_eq!(
        store.outbox_stats(at(37)).await.unwrap(),
        stats(0, 1, 2, None)
    );
    for n in [1, 3] {
        assert!(
            store
                .complete_outbox(&lease(n, "b", 2), OutboxFinal::Delivered, at(37))
                .await
                .unwrap()
        );
    }
    assert_eq!(
        store.outbox_stats(at(37)).await.unwrap(),
        stats(0, 1, 0, None)
    );

    // Row 2 falls due at at(1000), is claimed and finished.
    assert_eq!(
        store.outbox_stats(at(1000)).await.unwrap(),
        stats(1, 0, 0, Some(at(1000)))
    );
    assert_eq!(claim(&store, "b", at(1000)).await.len(), 1);
    assert!(
        store
            .complete_outbox(&lease(2, "b", 2), OutboxFinal::Delivered, at(1001))
            .await
            .unwrap()
    );
    assert_eq!(
        store.outbox_stats(at(1001)).await.unwrap(),
        OutboxStats::default()
    );
}
