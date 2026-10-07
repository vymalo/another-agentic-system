//! `ThreadStore` conformance cases. Each is `async fn(store)` on a fresh, isolated store.

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentTarget, AgentTaskState, CheckResult,
    CheckSource, CheckStatus, CiConclusion, CiProvider, CiReport, Classify, ErrorClass, Event,
    EventBody, EventKind, GatePolicy, Hold, Job, Origin, PushedRef, ReworkData, SourceFindings,
    ThreadId, ThreadState, Timer, UiCatalogData, UiCatalogLedger, UiDelivery, UserId,
    UserMessageData, WatchKey,
};
use uuid::Uuid;

use crate::{
    BindingUpdate, Commit, CommitOutcome, InboxFinal, InboxId, InboxLease, InboxPayload,
    InboxStatus, Lease, NewEvent, NewInbox, NewOutbox, NewThreadRecord, NewTimer, OutboxFinal,
    OutboxId, OutboxKind, OutboxPayload, OutboxStats, OutboxStatus, Parking, Received,
    SharingChange, StoreError, TIMER_SOURCE, ThreadStore,
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
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        // no context: the agent assigns it with its first answer (ADR 0055)
        context_id: None,
        rail_parent: None,
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
            new_job: false,
            ui_catalog: None,
            mentions: Vec::new(),
        },
    }
}

fn steer_row(n: u128) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Steer {
            text: format!("steer {n}"),
            release: Some("staging".to_owned()),
            ui_catalog: None,
            mentions: Vec::new(),
        },
    }
}

fn cancel_row(n: u128) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Cancel { job: Some(1) },
    }
}

fn verify_row(n: u128) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Verify {
            attempt: 1,
            verification: 1,
            verifier: AgentId::new("reviewer"),
            pushed: PushedRef {
                repository: "github.com/acme/demo".to_owned(),
                branch: "agent/fix".to_owned(),
                commit: "a".repeat(40),
            },
            text: "review it".to_owned(),
        },
    }
}

/// An `ask` row (ADR 0026): ask `ask` of job 1, to a researcher.
fn ask_row(n: u128, ask: u32) -> NewOutbox {
    NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Ask {
            job: 1,
            ask,
            agent: AgentId::new("researcher"),
            depth: 1,
            text: format!("find {ask}"),
            continue_task: None,
            reference_task_ids: Vec::new(),
            context: None,
        },
    }
}

fn commit(state: ThreadState, events: Vec<NewEvent>, outbox: Vec<NewOutbox>) -> Commit {
    Commit {
        new_state: state,
        job: None,
        events,
        outbox,
        binding: None,
        now: t0(),
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
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
    assert_eq!(
        binding.context_id, None,
        "the agent has not assigned one yet"
    );
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
            origin: Origin::Agui,
            // ADR 0036: how a message sent while a job ran reached it comes back as written
            delivery: Some(orch_core::Delivery::Interrupt),
            mentions: Vec::new(),
        }),
        idempotency_key: None,
    };
    let only_run = NewEvent {
        body: EventBody::UserMessage(UserMessageData {
            text: "run only".into(),
            message_id: None,
            run_id: Some("run-2".into()),
            origin: Origin::Agui,
            delivery: None,
            mentions: Vec::new(),
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
        // ADR 0019: a message an MCP client sent says so; the chat's leave the member out.
        NewEvent {
            body: EventBody::UserMessage(UserMessageData {
                origin: Origin::Mcp,
                ..UserMessageData::new("from a tool")
            }),
            ..user_event("unused", None)
        },
        // ADR 0031: what an agent's words are for comes back as written, and is absent (never
        // null) when the event does not say.
        NewEvent {
            body: EventBody::AgentMessage(orch_core::AgentMessageData {
                purpose: Some(orch_core::MessagePurpose::Working),
                ..orch_core::AgentMessageData::plain("w-1", "I will look that up.")
            }),
            ..agent_event("unused")
        },
        NewEvent {
            body: EventBody::AgentMessage(orch_core::AgentMessageData {
                purpose: Some(orch_core::MessagePurpose::Answer),
                via: Some(orch_core::AnswerVia::TurnOutput),
                ..orch_core::AgentMessageData::plain("a-1", "Here it is.")
            }),
            ..agent_event("unused")
        },
        agent_event("unmarked"),
        // ADR 0026: the references of a message come back as sent (an emoji before the label:
        // the offsets are UTF-16 code units), and the member is absent when there are none.
        NewEvent {
            body: EventBody::UserMessage(UserMessageData {
                mentions: vec![
                    orch_core::Mention {
                        agent_id: AgentId::new("researcher"),
                        label: "@researcher".into(),
                        start: 3,
                        end: 14,
                        card_url: Some("http://researcher:8080/.well-known/agent-card.json".into()),
                    },
                    orch_core::Mention {
                        agent_id: AgentId::new("coder"),
                        label: "@coder".into(),
                        start: 20,
                        end: 26,
                        card_url: None,
                    },
                ],
                ..UserMessageData::new("\u{1F604} @researcher then @coder")
            }),
            ..user_event("unused", None)
        },
    ];
    let bodies: Vec<EventBody> = wanted.iter().map(|e| e.body.clone()).collect();
    let action_row = NewOutbox {
        id: outbox_id(2),
        payload: OutboxPayload::Action {
            action,
            at: t0(),
            release: Some("staging".into()),
            ui_catalog: Some(UiDelivery::Ref(catalog_data(2, "chat").reference())),
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

    let read = store.list_events(thread_id(1), 0, 20).await.unwrap();
    assert_eq!(
        read.iter().map(|e| e.body.clone()).collect::<Vec<_>>(),
        bodies
    );
    assert_eq!(
        read.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    // The wire form the API serves is what the store returned: no null, camelCase ids.
    let data: Vec<serde_json::Value> = read.iter().map(|e| e.body.data_value()).collect();
    assert_eq!(
        data[0],
        serde_json::json!({
            "text": "with ids", "messageId": "msg-1", "runId": "run-1", "delivery": "interrupt"
        })
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
    assert_eq!(
        data[7],
        serde_json::json!({"text": "from a tool", "origin": "mcp"})
    );
    assert_eq!(
        data[8],
        serde_json::json!({"text": "I will look that up.", "messageId": "w-1", "final": true,
                           "purpose": "working"})
    );
    assert_eq!(
        data[9],
        serde_json::json!({"text": "Here it is.", "messageId": "a-1", "final": true,
                           "purpose": "answer", "via": "turn_output"})
    );
    assert_eq!(
        data[10],
        serde_json::json!({"text": "unmarked", "messageId": "m-unmarked", "final": true})
    );
    assert_eq!(
        data[11],
        serde_json::json!({"text": "\u{1F604} @researcher then @coder", "mentions": [
            {"agentId": "researcher", "label": "@researcher", "start": 3, "end": 14,
             "cardUrl": "http://researcher:8080/.well-known/agent-card.json"},
            {"agentId": "coder", "label": "@coder", "start": 20, "end": 26}
        ]})
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
            .list_threads(&bob(), crate::ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .list_threads(&alice(), crate::ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .len(),
        1
    );
}

pub async fn list_newest_first_before_limit<S: ThreadStore>(store: S) {
    for n in 1..=5 {
        seed(&store, &alice(), n).await;
    }
    seed(&store, &bob(), 6).await;
    let ids = |v: Vec<orch_core::ThreadRecord>| v.into_iter().map(|t| t.id).collect::<Vec<_>>();
    let all = ids(store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 50, false))
        .await
        .unwrap());
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
    let page = ids(store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 2, false))
        .await
        .unwrap());
    assert_eq!(page, vec![thread_id(5), thread_id(4)]);
    let next = ids(store
        .list_threads(
            &alice(),
            crate::ThreadListing::recent(Some(thread_id(3)), 50, false),
        )
        .await
        .unwrap());
    assert_eq!(next, vec![thread_id(2), thread_id(1)]);
    // A foreign or unknown cursor yields nothing.
    assert!(
        store
            .list_threads(
                &alice(),
                crate::ThreadListing::recent(Some(thread_id(6)), 50, false)
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_threads(
                &alice(),
                crate::ThreadListing::recent(Some(thread_id(77)), 50, false)
            )
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

/// `latest_events`: the newest events of one kind, newest first, bounded by the limit, and only
/// of the thread asked for.
pub async fn latest_events_newest_first<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    let artifact = |name: &str| NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::Artifact(orch_core::ArtifactData {
            name: name.to_owned(),
            mime_type: None,
            uri: None,
            text: None,
            file: None,
        }),
        idempotency_key: None,
    };
    let events = vec![
        artifact("a"),
        user_event("between", None),
        artifact("b"),
        artifact("c"),
        user_event("last", None),
    ];
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
    // Another thread's artifact must not show.
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![artifact("other")], vec![]),
            )
            .await
            .unwrap(),
    );
    let names = |v: Vec<orch_core::Event>| {
        v.into_iter()
            .filter_map(|e| match e.body {
                EventBody::Artifact(a) => Some((e.seq, a.name)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let all = store
        .latest_events(thread_id(1), orch_core::EventKind::Artifact, 10)
        .await
        .unwrap();
    assert_eq!(
        names(all),
        [
            (5, "c".to_owned()),
            (4, "b".to_owned()),
            (2, "a".to_owned())
        ]
    );
    let newest = store
        .latest_events(thread_id(1), orch_core::EventKind::Artifact, 1)
        .await
        .unwrap();
    assert_eq!(names(newest), [(5, "c".to_owned())]);
    assert!(
        store
            .latest_events(thread_id(1), orch_core::EventKind::Error, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .latest_events(thread_id(9), orch_core::EventKind::Artifact, 10)
            .await
            .unwrap()
            .is_empty(),
        "no thread, no events"
    );
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

/// `Commit::finishes_outbox`: the claimed row ends in the same transaction as what the commit
/// writes (ADR 0020: a redelivered message starts the next job and is done with it), and a commit
/// that is fenced, or a repeat, finishes nothing.
pub async fn a_commit_can_finish_the_claimed_row_with_what_it_writes<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let held = claim(&store, "a", t0()).await;
    assert_eq!(held.len(), 1);
    let l = held[0].lease().unwrap();

    // a stale claim: nothing written, and the row is not touched
    let stale = Lease {
        attempt: l.attempt + 1,
        ..l.clone()
    };
    let mut c = under(
        commit(ThreadState::Working, vec![user_event("late", None)], vec![]),
        stale,
    );
    c.finishes_outbox = Some(OutboxFinal::Skipped);
    assert_eq!(
        store.commit(thread_id(1), 1, c).await.unwrap(),
        CommitOutcome::Fenced
    );
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, OutboxStatus::Inflight);

    // the claim holds: the events, the new row and the end of this one are one commit
    let mut c = under(
        commit(
            ThreadState::Queued,
            vec![user_event("next", Some("finish-1"))],
            vec![delegate(9)],
        ),
        l.clone(),
    );
    c.finishes_outbox = Some(OutboxFinal::Skipped);
    let (record, events) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!((record.version, events.len()), (2, 1));
    let row = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, OutboxStatus::Skipped);
    assert_eq!(row.lease(), None);
    let new = store.get_outbox(outbox_id(9)).await.unwrap().unwrap();
    assert_eq!(new.status, OutboxStatus::Pending);
    // the claim is gone with the row: the same commit again is fenced, not applied twice
    let mut again = under(
        commit(
            ThreadState::Queued,
            vec![user_event("next", Some("finish-1"))],
            vec![],
        ),
        l,
    );
    again.finishes_outbox = Some(OutboxFinal::Skipped);
    assert_eq!(
        store.commit(thread_id(1), 2, again).await.unwrap(),
        CommitOutcome::Fenced
    );
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

/// A `verify` row (ADR 0018) is not a delegation: it is not held back by the delegate of its
/// own thread (which is still being finished when the verification is requested), and it does
/// not hold back the next delegate. Its task is on the row, fenced by its claim, and the
/// thread's binding is left alone.
pub async fn verify_rows_are_unordered_and_keep_their_task_on_the_row<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    // The delegate is claimed and still in flight when the verification is requested.
    assert_eq!(claim(&store, "a", t0()).await.len(), 1);
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Verifying, vec![], vec![verify_row(101)]),
            )
            .await
            .unwrap(),
    );
    let stored = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(stored.kind, OutboxKind::Verify);
    assert_eq!(stored.task_id, None);
    assert!(matches!(
        &stored.payload,
        OutboxPayload::Verify { attempt: 1, verification: 1, verifier, text, .. }
            if verifier.as_str() == "reviewer" && text == "review it"
    ));
    let got = claim(&store, "v", t0()).await;
    assert_eq!(
        got.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(101)],
        "not held back by the inflight delegate"
    );
    assert_eq!(got[0].kind, OutboxKind::Verify);

    // A later delegate (a rework) still waits for the first one, not for the verification.
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Queued, vec![], vec![delegate(102)]),
            )
            .await
            .unwrap(),
    );
    assert!(claim(&store, "a", t0()).await.is_empty());
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", t0())
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(102)],
        "the verification is still inflight and does not hold the delegate back"
    );

    // Recording the send: fenced by the claim, on the row, and the binding stays the worker's.
    let binding_before = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert!(
        !store
            .mark_verify_sent(&lease(101, "other", 1), "t-v".into(), at(1))
            .await
            .unwrap()
    );
    assert!(
        !store
            .mark_verify_sent(&lease(101, "v", 2), "t-v".into(), at(1))
            .await
            .unwrap(),
        "a stale attempt of the same owner"
    );
    let untouched = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(
        (untouched.sent_at, untouched.task_id.as_deref()),
        (None, None)
    );
    assert!(
        store
            .mark_verify_sent(&lease(101, "v", 1), "t-v".into(), at(1))
            .await
            .unwrap()
    );
    let sent = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(sent.sent_at, Some(at(1)));
    assert_eq!(sent.task_id.as_deref(), Some("t-v"));
    assert_eq!(sent.status, OutboxStatus::Inflight);
    assert_eq!(
        store.get_binding(thread_id(1)).await.unwrap().unwrap(),
        binding_before,
        "the verifier's task is not the thread's"
    );

    // A crashed claimant's row is claimed again with its task on it; finishing it is fenced.
    let again = claim(&store, "w", at(60)).await;
    let row = again.iter().find(|r| r.id == outbox_id(101)).unwrap();
    assert_eq!(row.attempts, 2);
    assert_eq!(row.task_id.as_deref(), Some("t-v"));
    assert!(row.sent_at.is_some());
    assert!(
        !store
            .complete_outbox(&lease(101, "v", 1), OutboxFinal::Delivered, at(61))
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(&lease(101, "w", 2), OutboxFinal::Delivered, at(61))
            .await
            .unwrap()
    );
}

/// An `ask` row (ADR 0026) is not a delegation: the agent that asks is running inside the delegation
/// in flight, so the row is not held back by it, and two asks of a job run side by side, so neither
/// holds the other back. It does not hold back the next delegation either, and a job superseded by
/// a Stop & send does not have its asks skipped with the delegations (the dispatcher drops an ask
/// the ledger says has ended). Its task is on the row, fenced by its claim, and the thread's
/// binding is left alone.
pub async fn ask_rows_are_unordered_and_keep_their_task_on_the_row<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    // The delegate is claimed and still in flight when its agent asks twice.
    assert_eq!(claim(&store, "a", t0()).await.len(), 2);
    let mut second = ask_row(102, 2);
    second.payload = OutboxPayload::Ask {
        job: 1,
        ask: 2,
        agent: AgentId::new("browser"),
        depth: 2,
        text: "and then?".to_owned(),
        continue_task: Some("t-earlier".to_owned()),
        reference_task_ids: vec!["t-a".to_owned(), "t-b".to_owned()],
        context: Some("ctx-asked".to_owned()),
    };
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Working, vec![], vec![ask_row(101, 1), second]),
            )
            .await
            .unwrap(),
    );
    let stored = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(stored.kind, OutboxKind::Ask);
    assert_eq!(stored.task_id, None);
    assert_eq!(
        stored.payload,
        OutboxPayload::Ask {
            job: 1,
            ask: 1,
            agent: AgentId::new("researcher"),
            depth: 1,
            text: "find 1".to_owned(),
            continue_task: None,
            reference_task_ids: Vec::new(),
            context: None,
        }
    );
    let stored = store.get_outbox(outbox_id(102)).await.unwrap().unwrap();
    assert!(matches!(
        &stored.payload,
        OutboxPayload::Ask { ask: 2, agent, depth: 2, text, continue_task: Some(task), reference_task_ids, context: Some(context), .. }
            if agent.as_str() == "browser" && text == "and then?" && task == "t-earlier"
                && reference_task_ids == &["t-a".to_owned(), "t-b".to_owned()]
                && context == "ctx-asked"
    ));

    // both are claimed at once: not held back by the inflight delegate nor by each other
    let got = claim(&store, "w", t0()).await;
    assert_eq!(
        got.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(101), outbox_id(102)],
        "not held back by the inflight delegate, nor by each other"
    );
    assert!(got.iter().all(|r| r.kind == OutboxKind::Ask));

    // A later delegation still waits for the first one, and not for the asks.
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Queued, vec![], vec![delegate(103)]),
            )
            .await
            .unwrap(),
    );
    assert!(claim(&store, "a", t0()).await.is_empty());
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", t0())
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(103)],
        "the asks are still inflight and do not hold the delegation back"
    );

    // Recording the send: fenced by the claim, on the row, and the binding stays the worker's.
    let binding_before = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    assert!(
        !store
            .mark_verify_sent(&lease(101, "other", 1), "t-ask".into(), at(1))
            .await
            .unwrap()
    );
    assert!(
        !store
            .mark_verify_sent(&lease(101, "w", 2), "t-ask".into(), at(1))
            .await
            .unwrap(),
        "a stale attempt of the same owner"
    );
    let untouched = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(
        (untouched.sent_at, untouched.task_id.as_deref()),
        (None, None)
    );
    assert!(
        store
            .mark_verify_sent(&lease(101, "w", 1), "t-ask".into(), at(1))
            .await
            .unwrap()
    );
    let sent = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(sent.sent_at, Some(at(1)));
    assert_eq!(sent.task_id.as_deref(), Some("t-ask"));
    assert_eq!(sent.status, OutboxStatus::Inflight);
    assert_eq!(
        store.get_binding(thread_id(1)).await.unwrap().unwrap(),
        binding_before,
        "the asked agent's task is not the thread's"
    );

    // A job superseded by a Stop & send skips its unsent delegations and steers, not its asks.
    applied(
        store
            .commit(
                thread_id(1),
                3,
                commit(ThreadState::Working, vec![], vec![ask_row(104, 3)]),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(1), at(2))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .get_outbox(outbox_id(104))
            .await
            .unwrap()
            .unwrap()
            .status,
        OutboxStatus::Pending
    );

    // A crashed claimant's row is claimed again with its task on it; finishing it is fenced.
    let again = claim(&store, "x", at(60)).await;
    let row = again.iter().find(|r| r.id == outbox_id(101)).unwrap();
    assert_eq!(row.attempts, 2);
    assert_eq!(row.task_id.as_deref(), Some("t-ask"));
    assert!(row.sent_at.is_some());
    assert!(
        !store
            .complete_outbox(&lease(101, "w", 1), OutboxFinal::Delivered, at(61))
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_outbox(&lease(101, "x", 2), OutboxFinal::Delivered, at(61))
            .await
            .unwrap()
    );
    // another thread's ask is as unordered, and an open ask of this one never holds it back
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![ask_row(201, 1)]),
            )
            .await
            .unwrap(),
    );
    assert!(
        claim(&store, "y", at(61))
            .await
            .iter()
            .any(|r| r.id == outbox_id(201))
    );
}

/// The events of an ask (ADR 0026) and the job's ledger of asks read back as they were written, and
/// the deadline of an ask is a timer like the gate's: one row per ask, armed by the commit, due after
/// its delay.
pub async fn ask_events_roundtrip<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let started = NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), Some("r1".to_owned())),
        body: EventBody::AskStarted(orch_core::AskStartedData {
            ask: 1,
            agent: AgentId::new("researcher"),
            by: orch_core::Caller::Main,
            depth: 1,
            text: "find the data".to_owned(),
            step_id: orch_core::ask_step_id(1),
            parent_step_id: Some("task-1/tool:c1".to_owned()),
        }),
        idempotency_key: Some("ask:1".to_owned()),
    };
    let finished = NewEvent {
        at: at(5),
        actor: Actor::agent(&AgentId::new("researcher"), None),
        body: EventBody::AskFinished(orch_core::AskFinishedData {
            ask: 1,
            state: orch_core::AskOutcome::InputRequired,
            text: Some("half of it".to_owned()),
            question: Some("which years?".to_owned()),
            artifacts: vec![orch_core::AskArtifact {
                name: "table".to_owned(),
                uri: Some("https://example.com/t.csv".to_owned()),
                mime_type: Some("text/csv".to_owned()),
            }],
            error: None,
        }),
        idempotency_key: None,
    };
    let timed_out = NewEvent {
        at: at(9),
        actor: Actor::system(),
        body: EventBody::AskFinished(orch_core::AskFinishedData {
            ask: 2,
            state: orch_core::AskOutcome::TimedOut,
            text: None,
            question: None,
            artifacts: Vec::new(),
            error: Some("the asked agent did not answer in time".to_owned()),
        }),
        idempotency_key: None,
    };
    let mut job = busy_job();
    job.asks.truncate(1);
    let mut first = commit(
        ThreadState::Working,
        vec![started.clone()],
        vec![ask_row(101, 1)],
    );
    first.job = Some(job.clone());
    first.timers = vec![NewTimer {
        id: inbox_id(1),
        after: SignedDuration::from_secs(1800),
        timer: Timer::AskDeadline { job: 3, ask: 1 },
    }];
    let (record, events) = applied(store.commit(thread_id(1), 1, first).await.unwrap());
    assert_eq!(record.job.asks, job.asks, "the ledger reads back");
    assert_eq!(events[0].kind(), orch_core::EventKind::AskStarted);
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(
                    ThreadState::Working,
                    vec![finished.clone(), timed_out.clone()],
                    vec![],
                ),
            )
            .await
            .unwrap(),
    );
    let read = store.list_events(thread_id(1), 1, 10).await.unwrap();
    let bodies: Vec<&EventBody> = read.iter().map(|e| &e.body).collect();
    assert_eq!(bodies, vec![&started.body, &finished.body, &timed_out.body]);
    assert_eq!(read[0].actor, started.actor);
    assert_eq!(read[1].actor, finished.actor);
    assert_eq!(read[2].actor, Actor::system());
    assert_eq!(
        store
            .latest_events(thread_id(1), orch_core::EventKind::AskFinished, 5)
            .await
            .unwrap()
            .len(),
        2
    );
    // a replayed commit of the first event is a duplicate, and arms no second deadline
    let mut replay = commit(ThreadState::Working, vec![started], vec![]);
    replay.timers = vec![NewTimer {
        id: inbox_id(2),
        after: SignedDuration::from_secs(1800),
        timer: Timer::AskDeadline { job: 3, ask: 1 },
    }];
    assert_eq!(
        store.commit(thread_id(1), 3, replay).await.unwrap(),
        CommitOutcome::Duplicate
    );
    assert!(iclaim(&store, "t", at(1799)).await.is_empty());
    let due = iclaim(&store, "t", at(1800)).await;
    assert_eq!(ids(&due), vec![inbox_id(1)]);
    assert_eq!(
        due[0].decode().unwrap(),
        crate::InboxPayload::Timer {
            thread: thread_id(1),
            timer: Timer::AskDeadline { job: 3, ask: 1 },
        }
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
        context_id: Some("agent-ctx".into()),
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
    assert_eq!(b.context_id.as_deref(), Some("agent-ctx"));
}

/// The binding adopts the context the agent assigned, **once** (ADR 0055): an update that names a
/// context sets it while the binding has none, an empty one is no context, and a later one, whether
/// the same or another, changes nothing. A thread created with a context (one that began before
/// the agent assigned them, whose context is its own id) keeps it.
pub async fn the_binding_adopts_the_agents_context_once<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let context = |value: Option<&str>| BindingUpdate {
        context_id: value.map(str::to_owned),
        ..BindingUpdate::default()
    };
    let read = || async {
        store
            .get_binding(thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .context_id
    };
    assert_eq!(read().await, None);

    // none named, or an empty one: still none
    let mut c = commit(ThreadState::Working, vec![user_event("a", None)], vec![]);
    c.binding = Some(context(None));
    applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!(read().await, None);
    let mut c = commit(ThreadState::Working, vec![user_event("b", None)], vec![]);
    c.binding = Some(context(Some("")));
    applied(store.commit(thread_id(1), 2, c).await.unwrap());
    assert_eq!(read().await, None, "an empty context is none");

    // the first one named stands
    let mut c = commit(ThreadState::Working, vec![user_event("c", None)], vec![]);
    c.binding = Some(context(Some("first")));
    applied(store.commit(thread_id(1), 3, c).await.unwrap());
    assert_eq!(read().await.as_deref(), Some("first"));

    // ... against the same, another, and none
    for (version, named) in [(4, Some("first")), (5, Some("second")), (6, None)] {
        let mut c = commit(ThreadState::Working, vec![user_event("d", None)], vec![]);
        c.binding = Some(context(named));
        applied(store.commit(thread_id(1), version, c).await.unwrap());
        assert_eq!(read().await.as_deref(), Some("first"), "never replaced");
    }

    // a thread created with a context has it, and the agent's cannot replace it
    let mut seeded = new_thread(&alice(), 2);
    seeded.context_id = Some("legacy".to_owned());
    store
        .create_thread(seeded, commit(ThreadState::Working, vec![], vec![]))
        .await
        .unwrap();
    let mut c = commit(ThreadState::Working, vec![user_event("e", None)], vec![]);
    c.binding = Some(context(Some("other")));
    applied(store.commit(thread_id(2), 1, c).await.unwrap());
    assert_eq!(
        store
            .get_binding(thread_id(2))
            .await
            .unwrap()
            .unwrap()
            .context_id
            .as_deref(),
        Some("legacy")
    );
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

/// A commit that says `skip_unsent_delegates` finishes the thread's unsent delegations in its own
/// transaction, before its rows exist (ADR 0036): the abandoned job's rows are skipped, the one
/// the commit writes is not, a row already sent is not, and a commit that is refused skips none.
pub async fn a_commit_can_skip_the_unsent_delegates_it_supersedes<S: ThreadStore>(store: S) {
    let status = |n: u128| {
        let store = &store;
        async move {
            store
                .get_outbox(outbox_id(n))
                .await
                .unwrap()
                .map(|row| row.status)
        }
    };
    seed(&store, &alice(), 1).await; // row 1, pending
    // another thread's unsent row is nobody's to skip
    seed(&store, &alice(), 2).await; // row 2

    // a commit that is refused (a stale version) skips nothing and writes nothing
    let mut stale = commit(ThreadState::Queued, vec![], vec![delegate(10)]);
    stale.skip_unsent_delegates = true;
    let res = store.commit(thread_id(1), 99, stale).await;
    assert_eq!(class_of(&res), Some(ErrorClass::Conflict), "{res:?}");
    assert_eq!(status(1).await, Some(OutboxStatus::Pending));
    assert_eq!(status(10).await, None);

    // one that is applied finishes row 1 and leaves its own row alone
    let mut next = commit(ThreadState::Queued, vec![], vec![delegate(11)]);
    next.skip_unsent_delegates = true;
    applied(store.commit(thread_id(1), 1, next).await.unwrap());
    assert_eq!(status(1).await, Some(OutboxStatus::Skipped));
    assert_eq!(status(11).await, Some(OutboxStatus::Pending));
    assert_eq!(status(2).await, Some(OutboxStatus::Pending));

    // a row that reached the agent is never skipped: the next commit supersedes only row 11
    let rows = claim(&store, "a", t0()).await;
    let mine = rows.iter().find(|r| r.id == outbox_id(11)).unwrap();
    assert!(
        store
            .mark_sent(
                &lease(11, "a", mine.attempts),
                BindingUpdate::default(),
                t0()
            )
            .await
            .unwrap()
    );
    let mut again = commit(ThreadState::Queued, vec![], vec![delegate(12)]);
    again.skip_unsent_delegates = true;
    applied(store.commit(thread_id(1), 2, again).await.unwrap());
    assert_ne!(status(11).await, Some(OutboxStatus::Skipped));
    assert_eq!(status(12).await, Some(OutboxStatus::Pending));
}

/// A `steer` row (ADR 0036) is claimed beside the delegation that is in flight (that one stays open
/// until the agent's turn ends, and a steer is for that very turn), in order among the thread's own
/// steer rows, and it does not hold a delegation back. It reads back as written.
pub async fn steer_rows_claim_beside_an_inflight_delegate<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    // another thread's steer row is nobody's to wait for
    seed(&store, &alice(), 2).await;
    assert_eq!(
        claim(&store, "a", t0()).await.len(),
        2,
        "the delegations of both threads are in flight"
    );
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![],
                    vec![steer_row(101), steer_row(102), delegate(103)],
                ),
            )
            .await
            .unwrap(),
    );
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![steer_row(201)]),
            )
            .await
            .unwrap(),
    );
    let stored = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(stored.kind, OutboxKind::Steer);
    assert_eq!(
        stored.payload,
        OutboxPayload::Steer {
            text: "steer 101".to_owned(),
            release: Some("staging".to_owned()),
            ui_catalog: None,
            mentions: Vec::new(),
        }
    );
    assert_eq!(stored.sent_at, None);

    // The first steer of each thread is claimed with the delegation in flight; the second waits
    // for the first, and the delegation of the next job waits for the one in flight, not for a steer.
    let got = claim(&store, "a", t0()).await;
    assert_eq!(
        got.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(101), outbox_id(201)]
    );
    assert!(got.iter().all(|r| r.kind == OutboxKind::Steer));
    assert!(
        claim(&store, "a", t0()).await.is_empty(),
        "the second steer waits for the first, the second delegation for the first"
    );
    // A steer in a backoff holds the next one back: they are delivered in the order written.
    assert!(
        store
            .retry_outbox(&lease(101, "a", 1), at(10), "agent down".to_owned())
            .await
            .unwrap()
    );
    assert!(claim(&store, "a", at(5)).await.is_empty());
    assert_eq!(
        claim(&store, "a", at(10))
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(101)]
    );
    assert!(
        store
            .complete_outbox(&lease(101, "a", 2), OutboxFinal::Delivered, at(10))
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", at(10))
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(102)]
    );
    // The delegation of the thread's next job is claimed when the one in flight ends.
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, at(10))
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", at(10))
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(103)]
    );
}

/// A steer the agent did not take becomes a delegation (ADR 0036): the same row, kind `delegate`,
/// with the delegation's payload, pending and due now, and in its old place in the thread's order,
/// so it waits behind the delegation in flight and in front of the ones written after it. Only the
/// claim that holds the row can do it, and only for a `steer` row.
pub async fn a_requeued_steer_waits_behind_the_delegation_in_flight<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await; // delegate 1, claimed below and in flight
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![],
                    vec![steer_row(101), delegate(102)],
                ),
            )
            .await
            .unwrap(),
    );
    let got = claim(&store, "a", t0()).await;
    assert_eq!(
        got.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(1), outbox_id(101)]
    );

    // not the claim of the row: another owner, another attempt, a row that is not a steer
    for stale in [lease(101, "b", 1), lease(101, "a", 2)] {
        assert!(!store.requeue_as_delegate(&stale, at(1)).await.unwrap());
    }
    assert!(
        !store
            .requeue_as_delegate(&lease(1, "a", 1), at(1))
            .await
            .unwrap(),
        "a delegation is not a steer"
    );
    let still = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(still.kind, OutboxKind::Steer);
    assert_eq!(still.status, OutboxStatus::Inflight);

    assert!(
        store
            .requeue_as_delegate(&lease(101, "a", 1), at(1))
            .await
            .unwrap()
    );
    let row = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(row.kind, OutboxKind::Delegate);
    assert_eq!(
        row.payload,
        OutboxPayload::Delegate {
            text: "steer 101".to_owned(),
            release: Some("staging".to_owned()),
            new_job: false,
            ui_catalog: None,
            mentions: Vec::new(),
        }
    );
    assert_eq!(row.status, OutboxStatus::Pending);
    assert_eq!(row.sent_at, None);
    assert_eq!(row.lease_owner, None);
    assert_eq!(
        row.attempts, 1,
        "the counter that fences the leases is not reset"
    );
    // the claim that was given up has nothing left to act on
    assert!(
        !store
            .complete_outbox(&lease(101, "a", 1), OutboxFinal::Delivered, at(1))
            .await
            .unwrap()
    );
    assert!(
        !store
            .requeue_as_delegate(&lease(101, "a", 1), at(1))
            .await
            .unwrap()
    );

    // It waits behind the delegation in flight, and the one written after it waits behind it.
    assert!(claim(&store, "a", at(1)).await.is_empty());
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, at(2))
            .await
            .unwrap()
    );
    let next = claim(&store, "a", at(2)).await;
    assert_eq!(
        next.iter().map(|r| (r.id, r.kind)).collect::<Vec<_>>(),
        vec![(outbox_id(101), OutboxKind::Delegate)]
    );
    assert_eq!(next[0].attempts, 2);
    assert!(claim(&store, "a", at(2)).await.is_empty());
    assert!(
        store
            .complete_outbox(&lease(101, "a", 2), OutboxFinal::Delivered, at(3))
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", at(3))
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(102)]
    );

    // A claim that lapsed and was taken over is the new owner's: the old one cannot requeue.
    seed(&store, &alice(), 2).await;
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![steer_row(201)]),
            )
            .await
            .unwrap(),
    );
    let first = claim(&store, "a", at(100)).await;
    assert!(first.iter().any(|r| r.id == outbox_id(201)));
    let second = claim(&store, "b", at(200)).await;
    assert!(second.iter().any(|r| r.id == outbox_id(201)));
    assert!(
        !store
            .requeue_as_delegate(&lease(201, "a", 1), at(200))
            .await
            .unwrap()
    );
    assert!(
        store
            .requeue_as_delegate(&lease(201, "b", 2), at(200))
            .await
            .unwrap()
    );
}

/// The rows a stop supersedes include the `steer` rows (ADR 0036): both
/// [`skip_unsent_delegates`](ThreadStore::skip_unsent_delegates) and a commit that says so finish a
/// thread's unsent steers with its unsent delegations, and leave a steer a live worker holds.
pub async fn unsent_steers_are_skipped_with_the_unsent_delegates<S: ThreadStore>(store: S) {
    let status = |n: u128| {
        let store = &store;
        async move {
            store
                .get_outbox(outbox_id(n))
                .await
                .unwrap()
                .map(|row| row.status)
        }
    };
    seed(&store, &alice(), 1).await; // delegate 1
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![],
                    vec![steer_row(101), steer_row(102)],
                ),
            )
            .await
            .unwrap(),
    );
    // steer 101 is held by a live worker, 102 waits behind it
    let got = claim(&store, "a", at(0)).await;
    assert!(got.iter().any(|r| r.id == outbox_id(101)));

    // the call skips the unsent steer and delegation, and not the one a live worker holds
    assert_eq!(
        store
            .skip_unsent_delegates(thread_id(1), at(5))
            .await
            .unwrap(),
        1,
        "steer 102 (delegate 1 is in flight under a live lease, and so is steer 101)"
    );
    assert_eq!(status(102).await, Some(OutboxStatus::Skipped));
    assert_eq!(status(101).await, Some(OutboxStatus::Inflight));

    // a commit that supersedes finishes the rest of the unsent ones once the leases lapsed, never
    // a row it writes itself
    seed(&store, &alice(), 2).await;
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![steer_row(201)]),
            )
            .await
            .unwrap(),
    );
    let mut next = commit(
        ThreadState::Queued,
        vec![],
        vec![steer_row(202), delegate(203)],
    );
    next.skip_unsent_delegates = true;
    next.now = at(10);
    applied(store.commit(thread_id(2), 2, next).await.unwrap());
    assert_eq!(status(2).await, Some(OutboxStatus::Skipped));
    assert_eq!(status(201).await, Some(OutboxStatus::Skipped));
    assert_eq!(status(202).await, Some(OutboxStatus::Pending));
    assert_eq!(status(203).await, Some(OutboxStatus::Pending));
    assert_eq!(
        status(1).await,
        Some(OutboxStatus::Inflight),
        "another thread's"
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
        context_id: None,
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

/// A version of a UI catalog, with its real digest; the version is part of the content, so two
/// versions differ, and `tag` makes two of one version differ.
fn catalog_data(version: u32, tag: &str) -> UiCatalogData {
    let catalog = serde_json::json!({
        "catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
        "components": {"Note": {"type": "object", "title": format!("{tag}-{version}")}},
    });
    UiCatalogData {
        catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".to_owned(),
        version,
        digest: orch_core::catalog_digest(&catalog).unwrap(),
        catalog,
    }
}

/// A job that uses every field of the ledger.
fn busy_job() -> Job {
    let mut catalog = UiCatalogLedger::default();
    catalog.accept(Some(&catalog_data(1, "chat")));
    catalog.accept(Some(&catalog_data(2, "chat")));
    let mut gate = GatePolicy::requiring([CheckSource::Ci, CheckSource::Verifier]);
    gate.max_attempts = 4;
    gate.ci.required = ["build".to_owned()].into();
    gate.verifier = Some(AgentId::new("reviewer"));
    let mut job = Job {
        number: 3,
        gate,
        attempt: 2,
        verification: 3,
        task: Some("make the tests pass\n\n[next message]\nin acme/widgets".into()),
        branch_problem: Some("`commit` is not a full commit hash".into()),
        summary: Some("Done: the fix is on the branch".into()),
        pushed: Some(PushedRef {
            repository: "github.com/vymalo/repo".into(),
            branch: "agent/x".into(),
            commit: "a".repeat(40),
        }),
        results: vec![CheckResult {
            source: CheckSource::Ci,
            name: Some("build".into()),
            attempt: 2,
            commit: Some("a".repeat(40)),
            status: CheckStatus::Failed,
            summary: Some("red".into()),
            stale: false,
            findings: vec!["build: failure".into()],
        }],
        hold: Some(Hold::CiTimeout),
        catalog,
        steps: orch_core::StepLedger::default(),
        answer: orch_core::AnswerLedger::default(),
        title: orch_core::TitleLedger::of(orch_core::TitleSource::User),
        description: orch_core::DescriptionLedger::of(orch_core::DescriptionSource::Model),
        tools: vec!["docs".to_owned(), "websearch".to_owned()],
        // ADR 0036: the text a job being stopped holds for the next one
        after_stop: Some("do X instead".to_owned()),
        // ADR 0026: the agents the job's messages mentioned, and the mentions of the held text
        mentioned: [AgentId::new("researcher"), AgentId::new("coder")].into(),
        after_stop_mentions: vec![orch_core::Mention {
            agent_id: AgentId::new("coder"),
            label: "@coder".to_owned(),
            start: 3,
            end: 9,
            card_url: Some("http://coder:8080/.well-known/agent-card.json".to_owned()),
        }],
        // ADR 0026: one ask that ended with its task recorded, and one that runs
        asks: vec![
            orch_core::Ask {
                n: 1,
                by: orch_core::Caller::Main,
                agent: AgentId::new("researcher"),
                depth: 1,
                call_key: Some("ask:t:main:c1".to_owned()),
                fingerprint: Some("0123456789abcdef".to_owned()),
                task_id: Some("task-r".to_owned()),
                context_id: Some("ctx-r".to_owned()),
                outcome: Some(orch_core::AskOutcome::InputRequired),
            },
            orch_core::Ask {
                n: 2,
                by: orch_core::Caller::Ask(1),
                agent: AgentId::new("coder"),
                depth: 2,
                call_key: None,
                fingerprint: None,
                task_id: None,
                context_id: None,
                outcome: None,
            },
        ],
    };
    // Two steps open, one of them nested and updated: the ledger has an entry with a path and a
    // count of updates.
    for (id, parent, state) in [
        ("task-1/tool:c1", None, orch_core::StepState::Running),
        (
            "task-1/acp:c1:1",
            Some("task-1/tool:c1"),
            orch_core::StepState::Running,
        ),
        (
            "task-1/acp:c1:1",
            Some("task-1/tool:c1"),
            orch_core::StepState::Waiting,
        ),
    ] {
        let report = orch_core::StepReport {
            id: id.to_owned(),
            parent: parent.map(str::to_owned),
            kind: orch_core::StepKind::Command,
            label: "npm test".to_owned(),
            state,
            icon: None,
            detail: None,
            input: None,
            output: None,
        };
        orch_core::record_step(
            ThreadState::Working,
            &mut job,
            Actor::system(),
            &report,
            orch_core::StepSource::Agent,
        );
    }
    // The turn announced its answer (`turn_output`, ADR 0031): the ledger has a token, a count
    // and the digest of what was said last.
    let (announced, _) = orch_core::transition(
        &orch_core::Snapshot {
            state: ThreadState::Working,
            job,
        },
        &orch_core::Input::Answer {
            actor: Actor::system(),
            text: "The result.".to_owned(),
            job: 3,
            token: "m-3".to_owned(),
        },
    )
    .unwrap();
    assert!(announced.job.answer.is_announced());
    announced.job
}

/// The MCP servers attached to a thread (ADR 0024) live in its job ledger and in the log. A thread
/// created with servers, and a commit that attaches one and detaches another, read back with the
/// set in every read (the thread, the listing) and with the `tools_attached` and `tools_detached`
/// events as written (ids only); a commit that leaves the job alone leaves the set; and the next job
/// of the thread keeps it, as the core's `Job::next` does.
pub async fn job_tools_roundtrip<S: ThreadStore>(store: S) {
    use orch_core::ToolsData;
    let event = |body: EventBody| NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body,
        idempotency_key: None,
    };
    let servers = |ids: &[&str]| ToolsData {
        servers: ids.iter().map(|id| (*id).to_owned()).collect(),
    };

    // Created with two servers: the creation commit holds the message, then the event.
    let mut first = commit(
        ThreadState::Queued,
        vec![
            user_event("hi", None),
            event(EventBody::ToolsAttached(servers(&["docs", "websearch"]))),
        ],
        vec![],
    );
    first.job = Some(Job {
        tools: vec!["docs".to_owned(), "websearch".to_owned()],
        ..Job::default()
    });
    let (created, events) = store
        .create_thread(new_thread(&alice(), 1), first)
        .await
        .unwrap();
    assert_eq!(created.job.tools, ["docs", "websearch"]);
    assert_eq!(
        events.iter().map(Event::kind).collect::<Vec<_>>(),
        [EventKind::UserMessage, EventKind::ToolsAttached]
    );
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got, created);
    let listed = store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
        .await
        .unwrap();
    assert_eq!(listed[0].job.tools, ["docs", "websearch"]);

    // One attached and one detached, in one commit.
    let mut change = commit(
        ThreadState::Queued,
        vec![
            event(EventBody::ToolsAttached(servers(&["files"]))),
            event(EventBody::ToolsDetached(servers(&["docs"]))),
        ],
        vec![],
    );
    change.job = Some(Job {
        tools: vec!["files".to_owned(), "websearch".to_owned()],
        ..Job::default()
    });
    let (record, written) = applied(store.commit(thread_id(1), 1, change).await.unwrap());
    assert_eq!(record.job.tools, ["files", "websearch"]);
    assert_eq!(
        written.iter().map(|e| e.body.clone()).collect::<Vec<_>>(),
        [
            EventBody::ToolsAttached(servers(&["files"])),
            EventBody::ToolsDetached(servers(&["docs"]))
        ]
    );
    let read = store.list_events(thread_id(1), 2, 10).await.unwrap();
    assert_eq!(read, written, "the events read back as they were written");
    assert_eq!(
        serde_json::to_value(&read[0]).unwrap()["data"],
        serde_json::json!({"servers": ["files"]}),
        "the event holds ids and nothing else"
    );

    // A commit that leaves the job alone leaves the set.
    let (record, _) = applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Working, vec![user_event("more", None)], vec![]),
            )
            .await
            .unwrap(),
    );
    assert_eq!(record.job.tools, ["files", "websearch"]);

    // The thread's next job keeps the set; clearing it empties it.
    let mut next = commit(ThreadState::Queued, vec![], vec![]);
    next.job = Some(record.job.next());
    let (record, _) = applied(store.commit(thread_id(1), 3, next).await.unwrap());
    assert_eq!(
        (record.job.number, record.job.tools.clone()),
        (2, vec!["files".to_owned(), "websearch".to_owned()])
    );
    let mut clear = commit(
        ThreadState::Queued,
        vec![event(EventBody::ToolsDetached(servers(&[
            "files",
            "websearch",
        ])))],
        vec![],
    );
    clear.job = Some(Job {
        tools: vec![],
        ..record.job.clone()
    });
    let (record, _) = applied(store.commit(thread_id(1), 4, clear).await.unwrap());
    assert!(record.job.tools.is_empty());
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got, record);
    assert_eq!(got.job.number, 2, "the rest of the ledger is as it was");
}

/// The job is stored with the thread, comes back exactly, and a commit without one leaves it.
pub async fn job_roundtrip<S: ThreadStore>(store: S) {
    // A thread created without a job has no gate.
    let (plain, _) = store
        .create_thread(
            new_thread(&alice(), 1),
            commit(ThreadState::Queued, vec![user_event("hi", None)], vec![]),
        )
        .await
        .unwrap();
    assert_eq!(plain.job, Job::default());
    assert!(!plain.job.gate.is_active());

    // A thread created with one keeps it, in every read.
    let mut first = commit(ThreadState::Queued, vec![user_event("hi", None)], vec![]);
    first.job = Some(busy_job());
    let (created, _) = store
        .create_thread(new_thread(&alice(), 2), first)
        .await
        .unwrap();
    assert_eq!(created.job, busy_job());
    let got = store
        .get_thread(Some(&alice()), thread_id(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, created);
    let listed = store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
        .await
        .unwrap();
    assert_eq!(
        listed.iter().find(|t| t.id == thread_id(2)).unwrap().job,
        busy_job()
    );

    // A commit writes state and job together.
    let mut next = busy_job();
    next.attempt = 3;
    next.results.clear();
    next.hold = None;
    let mut c = commit(ThreadState::Verifying, vec![user_event("x", None)], vec![]);
    c.job = Some(next.clone());
    let (record, _) = applied(store.commit(thread_id(2), 1, c).await.unwrap());
    assert_eq!((record.state, &record.job), (ThreadState::Verifying, &next));
    let got = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(got, record);

    // A commit with no job changes the state and leaves the job.
    let (record, _) = applied(
        store
            .commit(
                thread_id(2),
                2,
                commit(ThreadState::Blocked, vec![], vec![]),
            )
            .await
            .unwrap(),
    );
    assert_eq!((record.state, &record.job), (ThreadState::Blocked, &next));

    // A job change alone is a commit: it bumps the version.
    let mut again = next.clone();
    again.verification += 1;
    let mut c = commit(ThreadState::Blocked, vec![], vec![]);
    c.job = Some(again.clone());
    let (record, events) = applied(store.commit(thread_id(2), 3, c).await.unwrap());
    assert_eq!((record.version, record.job), (4, again));
    assert!(events.is_empty());

    // The job never leaks into the thread of someone else.
    assert!(
        store
            .get_thread(Some(&bob()), thread_id(2))
            .await
            .unwrap()
            .is_none()
    );
}

/// The job is written under the same compare-and-swap, lock and idempotency check as the state:
/// a commit that is refused writes neither.
pub async fn job_is_written_with_the_state<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let mut winner = commit(
        ThreadState::Verifying,
        vec![user_event("w", Some("k"))],
        vec![],
    );
    let mut won = Job::with_gate(GatePolicy::requiring([CheckSource::AgentChecks]));
    won.attempt = 2;
    winner.job = Some(won.clone());
    let (record, _) = applied(store.commit(thread_id(1), 1, winner).await.unwrap());
    assert_eq!(record.job, won);

    // A second writer that read the same version loses; its job is not written.
    let mut loser = commit(ThreadState::Working, vec![user_event("l", None)], vec![]);
    loser.job = Some(busy_job());
    let res = store.commit(thread_id(1), 1, loser).await;
    assert_eq!(class_of(&res), Some(ErrorClass::Conflict), "{res:?}");
    let after = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(after, record, "a refused commit writes nothing");

    // A replayed idempotency key is a duplicate; its job is not written either.
    let mut replay = commit(
        ThreadState::Working,
        vec![user_event("w", Some("k"))],
        vec![],
    );
    replay.job = Some(busy_job());
    assert_eq!(
        store.commit(thread_id(1), 2, replay).await.unwrap(),
        CommitOutcome::Duplicate
    );
    let after = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(after, record);

    // And a fenced commit (a stale claim) writes no job.
    let held = claim(&store, "a", t0()).await;
    let stale = held[0].lease().unwrap();
    let mut fenced = under(
        commit(ThreadState::Working, vec![user_event("f", None)], vec![]),
        Lease {
            attempt: stale.attempt + 1,
            ..stale
        },
    );
    fenced.job = Some(busy_job());
    assert_eq!(
        store.commit(thread_id(1), 2, fenced).await.unwrap(),
        CommitOutcome::Fenced
    );
    let after = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(after, record);

    // Two writers race on one version: exactly one wins, and the job is the winner's.
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for n in 0..4_u32 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let mut job = Job::with_gate(GatePolicy::requiring([CheckSource::Ci]));
            job.verification = n + 10;
            let mut c = commit(ThreadState::Verifying, vec![], vec![]);
            c.job = Some(job.clone());
            (job, store.commit(thread_id(1), 2, c).await)
        }));
    }
    let mut winners = Vec::new();
    for task in tasks {
        let (job, res) = task.await.unwrap();
        match res {
            Ok(CommitOutcome::Applied { .. }) => winners.push(job),
            Err(e) => assert_eq!(e.class(), ErrorClass::Conflict),
            Ok(other) => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(winners.len(), 1);
    let after = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(after.job, winners[0]);
    assert_eq!(after.version, 3);
}

/// A `title` row (a request to the model for the thread's title) is the orchestrator's own and
/// depends on nothing: it is claimable at once, whatever delegation of its thread is still in
/// flight (it is requested by the very reply that delegation is delivering), it does not hold the
/// next delegation back, and its payload reads back as written.
pub async fn title_rows_are_unordered_and_roundtrip<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    // The delegate is claimed and still in flight when the agent's reply asks for a title.
    assert_eq!(claim(&store, "a", t0()).await.len(), 1);
    let title = NewOutbox {
        id: outbox_id(101),
        payload: OutboxPayload::Title { ask: 2 },
    };
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(ThreadState::Working, vec![], vec![title]),
            )
            .await
            .unwrap(),
    );
    let stored = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(stored.kind, OutboxKind::Title);
    assert_eq!(stored.payload, OutboxPayload::Title { ask: 2 });
    assert_eq!(stored.task_id, None);

    let got = claim(&store, "t", t0()).await;
    assert_eq!(
        got.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![outbox_id(101)],
        "not held back by the inflight delegate"
    );
    assert_eq!(got[0].kind, OutboxKind::Title);

    // A later delegate still waits for the first one, not for the title request.
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Queued, vec![], vec![delegate(102)]),
            )
            .await
            .unwrap(),
    );
    assert!(claim(&store, "a", t0()).await.is_empty());
    assert!(
        store
            .complete_outbox(&lease(1, "a", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
    assert_eq!(
        claim(&store, "a", t0())
            .await
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![outbox_id(102)],
        "the title request is still inflight and does not hold the delegate back"
    );
    // and it ends like any row
    assert!(
        store
            .complete_outbox(&lease(101, "t", 1), OutboxFinal::Delivered, t0())
            .await
            .unwrap()
    );
}

/// A rename is one transaction: the `thread_titled` event, the thread's title and the ledger that
/// remembers whose it is. A commit that does not set a title leaves it; one that is refused (a
/// stale version, a replayed key, a stale claim) writes none.
pub async fn thread_titled_roundtrip<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let title_event = |title: &str, key: Option<&str>| NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::ThreadTitled(orch_core::ThreadTitledData {
            title: title.to_owned(),
            source: orch_core::TitledBy::User,
        }),
        idempotency_key: key.map(str::to_owned),
    };
    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(before.title, "thread 1");

    // The title, the event and the ledger are written together.
    let mut rename = commit(
        ThreadState::Queued,
        vec![title_event("Mine", Some("k"))],
        vec![],
    );
    rename.title = Some("Mine".to_owned());
    rename.job = Some(Job {
        title: orch_core::TitleLedger::of(orch_core::TitleSource::User),
        ..Job::default()
    });
    let (record, events) = applied(store.commit(thread_id(1), 1, rename).await.unwrap());
    assert_eq!(record.title, "Mine");
    assert_eq!(record.job.title.source(), orch_core::TitleSource::User);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), orch_core::EventKind::ThreadTitled);
    let read = store.list_events(thread_id(1), 1, 10).await.unwrap();
    assert_eq!(read, events, "the event reads back as it was written");
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got.title, "Mine");
    let listed = store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
        .await
        .unwrap();
    assert_eq!(listed[0].title, "Mine", "the sidebar's listing says it");

    // A commit with no title leaves it.
    let plain = commit(ThreadState::Queued, vec![user_event("more", None)], vec![]);
    let (record, _) = applied(store.commit(thread_id(1), 2, plain).await.unwrap());
    assert_eq!(record.title, "Mine");

    // A refused commit writes no title: a stale version, a replayed key, a stale claim.
    let mut stale = commit(
        ThreadState::Queued,
        vec![title_event("Stale", None)],
        vec![],
    );
    stale.title = Some("Stale".to_owned());
    let res = store.commit(thread_id(1), 1, stale).await;
    assert_eq!(class_of(&res), Some(ErrorClass::Conflict), "{res:?}");
    let mut replay = commit(
        ThreadState::Queued,
        vec![title_event("Replay", Some("k"))],
        vec![],
    );
    replay.title = Some("Replay".to_owned());
    assert_eq!(
        store.commit(thread_id(1), 3, replay).await.unwrap(),
        CommitOutcome::Duplicate
    );
    let held = claim(&store, "a", t0()).await;
    let lease = held[0].lease().unwrap();
    let mut fenced = under(
        commit(
            ThreadState::Queued,
            vec![title_event("Fenced", None)],
            vec![],
        ),
        Lease {
            attempt: lease.attempt + 1,
            ..lease
        },
    );
    fenced.title = Some("Fenced".to_owned());
    assert_eq!(
        store.commit(thread_id(1), 3, fenced).await.unwrap(),
        CommitOutcome::Fenced
    );
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got.title, "Mine");
    assert_eq!(got.version, 3);

    // Another thread's title is its own.
    seed(&store, &alice(), 2).await;
    let other = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(other.title, "thread 2");
}

/// A description is one transaction like a title: the `thread_described` event, the thread's
/// description (for the listing) and the ledger that remembers whose it is. A commit that does not
/// set one leaves it, `Some("")` clears it, and one that is refused writes none. A `description` row
/// is the orchestrator's own and depends on nothing, like a `title` row, and reads back as written.
/// A fork starts with the description it is given.
pub async fn thread_described_roundtrip<S: ThreadStore>(store: S) {
    use orch_core::{DescribedBy, DescriptionLedger, DescriptionSource, ThreadDescribedData};
    seed(&store, &alice(), 1).await;
    let event = |description: &str, by: DescribedBy, key: Option<&str>| NewEvent {
        at: t0(),
        actor: Actor::system(),
        body: EventBody::ThreadDescribed(ThreadDescribedData {
            description: description.to_owned(),
            source: by,
        }),
        idempotency_key: key.map(str::to_owned),
    };
    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(before.description, None);

    // The description, the event and the ledger are written together.
    let mut describe = commit(
        ThreadState::Queued,
        vec![event(
            "Moving the build to Rust.",
            DescribedBy::Model,
            Some("k"),
        )],
        vec![],
    );
    describe.description = Some("Moving the build to Rust.".to_owned());
    describe.job = Some(Job {
        description: DescriptionLedger::of(DescriptionSource::Model),
        ..Job::default()
    });
    let (record, events) = applied(store.commit(thread_id(1), 1, describe).await.unwrap());
    assert_eq!(
        record.description.as_deref(),
        Some("Moving the build to Rust.")
    );
    assert_eq!(record.job.description.source(), DescriptionSource::Model);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), orch_core::EventKind::ThreadDescribed);
    let read = store.list_events(thread_id(1), 1, 10).await.unwrap();
    assert_eq!(read, events, "the event reads back as it was written");
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(
        got.description.as_deref(),
        Some("Moving the build to Rust.")
    );
    let listed = store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
        .await
        .unwrap();
    assert_eq!(
        listed[0].description.as_deref(),
        Some("Moving the build to Rust."),
        "the sidebar's listing says it"
    );
    assert_eq!(got.title, "thread 1", "the title is not the description");

    // A commit with no description leaves it.
    let plain = commit(ThreadState::Queued, vec![user_event("more", None)], vec![]);
    let (record, _) = applied(store.commit(thread_id(1), 2, plain).await.unwrap());
    assert_eq!(
        record.description.as_deref(),
        Some("Moving the build to Rust.")
    );

    // A refused commit writes none: a stale version, a replayed key, a stale claim.
    let mut stale = commit(
        ThreadState::Queued,
        vec![event("Stale", DescribedBy::Model, None)],
        vec![],
    );
    stale.description = Some("Stale".to_owned());
    let res = store.commit(thread_id(1), 1, stale).await;
    assert_eq!(class_of(&res), Some(ErrorClass::Conflict), "{res:?}");
    let mut replay = commit(
        ThreadState::Queued,
        vec![event("Replay", DescribedBy::Model, Some("k"))],
        vec![],
    );
    replay.description = Some("Replay".to_owned());
    assert_eq!(
        store.commit(thread_id(1), 3, replay).await.unwrap(),
        CommitOutcome::Duplicate
    );
    let held = claim(&store, "a", t0()).await;
    let lease = held[0].lease().unwrap();
    let mut fenced = under(
        commit(
            ThreadState::Queued,
            vec![event("Fenced", DescribedBy::Model, None)],
            vec![],
        ),
        Lease {
            attempt: lease.attempt + 1,
            ..lease
        },
    );
    fenced.description = Some("Fenced".to_owned());
    assert_eq!(
        store.commit(thread_id(1), 3, fenced).await.unwrap(),
        CommitOutcome::Fenced
    );
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(
        got.description.as_deref(),
        Some("Moving the build to Rust.")
    );
    assert_eq!(got.version, 3);

    // A person clears it: the event says so and the thread has none.
    let mut clear = commit(
        ThreadState::Queued,
        vec![event("", DescribedBy::User, None)],
        vec![],
    );
    clear.description = Some(String::new());
    clear.job = Some(Job {
        description: DescriptionLedger::of(DescriptionSource::User),
        ..Job::default()
    });
    let (record, _) = applied(store.commit(thread_id(1), 3, clear).await.unwrap());
    assert_eq!(record.description, None);
    assert_eq!(record.job.description.source(), DescriptionSource::User);
    let listed = store
        .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
        .await
        .unwrap();
    assert_eq!(listed[0].description, None);

    // Another thread's description is its own.
    seed(&store, &alice(), 2).await;
    let other = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(other.description, None);

    // A `description` row depends on nothing: claimable while the delegate is in flight, and it
    // reads back as written.
    let row = NewOutbox {
        id: outbox_id(201),
        payload: OutboxPayload::Description { job: 3 },
    };
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![row]),
            )
            .await
            .unwrap(),
    );
    let stored = store.get_outbox(outbox_id(201)).await.unwrap().unwrap();
    assert_eq!(stored.kind, OutboxKind::Description);
    assert_eq!(stored.payload, OutboxPayload::Description { job: 3 });
    assert_eq!(stored.task_id, None);
    let got = claim(&store, "t", t0()).await;
    assert!(
        got.iter()
            .any(|r| r.id == outbox_id(201) && r.kind == OutboxKind::Description),
        "not held back by the delegate of its thread"
    );
}

/// A fork starts with the description it is given (its parent's), which the listing and the
/// thread resource say.
pub async fn a_fork_starts_with_the_description_it_is_given<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    let mut new = new_thread(&alice(), 2);
    new.description = Some("The parent's description.".to_owned());
    let mut first = commit(
        ThreadState::Done,
        vec![forked_event(1, 2, ForkKind::Fork)],
        vec![],
    );
    first.description = Some("Ignored: a thread is created with the new thread's own.".to_owned());
    let (record, _) = store
        .fork_thread(
            new,
            crate::ForkOrigin {
                parent: thread_id(1),
                cut: 2,
                kind: ForkKind::Fork,
            },
            first,
        )
        .await
        .unwrap();
    assert_eq!(
        record.description.as_deref(),
        Some("The parent's description.")
    );
    let got = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(
        got.description.as_deref(),
        Some("The parent's description.")
    );
    let parent = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(parent.description, None, "the parent is untouched");
}

/// ADR 0026: the references of a message ride the outbox row of its delegation, as the
/// `user_message` event holds them: a row with mentions comes back with them, and a row written
/// without (an older one, or a message that mentions nobody) reads as before.
pub async fn delegate_rows_keep_their_mentions<S: ThreadStore>(store: S) {
    let mentions = vec![
        orch_core::Mention {
            agent_id: AgentId::new("researcher"),
            label: "@researcher".into(),
            start: 3,
            end: 14,
            card_url: Some("http://researcher:8080/.well-known/agent-card.json".into()),
        },
        orch_core::Mention {
            agent_id: AgentId::new("coder"),
            label: "@coder".into(),
            start: 20,
            end: 26,
            card_url: None,
        },
    ];
    let row = |n: u128, mentions: Vec<orch_core::Mention>| NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Delegate {
            text: "\u{1F604} @researcher then @coder".to_owned(),
            release: None,
            new_job: false,
            ui_catalog: None,
            mentions,
        },
    };
    let first = commit(
        ThreadState::Queued,
        vec![user_event("hi", None)],
        vec![row(1, mentions.clone()), row(2, Vec::new())],
    );
    store
        .create_thread(new_thread(&alice(), 1), first)
        .await
        .unwrap();
    let with = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(with.payload, row(1, mentions.clone()).payload);
    let OutboxPayload::Delegate { mentions: got, .. } = &with.payload else {
        panic!("a delegation");
    };
    assert_eq!(got, &mentions);
    let without = store.get_outbox(outbox_id(2)).await.unwrap().unwrap();
    assert_eq!(without.payload, row(2, Vec::new()).payload);
    // the claim hands the row over with them
    let claimed = claim(&store, "worker-1", t0()).await;
    assert!(
        claimed.iter().any(|c| matches!(
            &c.payload,
            OutboxPayload::Delegate { mentions: m, .. } if *m == mentions
        )),
        "{claimed:?}"
    );
}

/// ADR 0026 with ADR 0036: a steer row keeps the references of its message, as a delegation row
/// does: a row with mentions comes back with them (read, claimed), the delegation it becomes when
/// the agent does not take it carries the same references, and a row written without them (an older
/// one, or a message that mentions nobody) reads as before and becomes a delegation without.
pub async fn steer_rows_keep_their_mentions<S: ThreadStore>(store: S) {
    let mentions = vec![
        orch_core::Mention {
            agent_id: AgentId::new("researcher"),
            label: "@researcher".into(),
            start: 3,
            end: 14,
            card_url: Some("http://researcher:8080/.well-known/agent-card.json".into()),
        },
        orch_core::Mention {
            agent_id: AgentId::new("coder"),
            label: "@coder".into(),
            start: 20,
            end: 26,
            card_url: None,
        },
    ];
    let row = |n: u128, mentions: Vec<orch_core::Mention>| NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Steer {
            text: "\u{1F604} @researcher then @coder".to_owned(),
            release: None,
            ui_catalog: None,
            mentions,
        },
    };
    seed(&store, &alice(), 1).await; // delegate 1, claimed below and in flight
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![],
                    vec![row(101, mentions.clone()), row(102, Vec::new())],
                ),
            )
            .await
            .unwrap(),
    );
    let with = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(with.kind, OutboxKind::Steer);
    assert_eq!(with.payload, row(101, mentions.clone()).payload);
    let without = store.get_outbox(outbox_id(102)).await.unwrap().unwrap();
    assert_eq!(without.payload, row(102, Vec::new()).payload);

    // the claim hands the first steer over with them
    let claimed = claim(&store, "a", t0()).await;
    let steer = claimed
        .iter()
        .find(|c| c.id == outbox_id(101))
        .expect("the first steer is claimed beside the delegation in flight");
    assert!(
        matches!(&steer.payload, OutboxPayload::Steer { mentions: m, .. } if *m == mentions),
        "{steer:?}"
    );

    // and the delegation it becomes carries them
    assert!(
        store
            .requeue_as_delegate(&lease(101, "a", 1), at(1))
            .await
            .unwrap()
    );
    let back = store.get_outbox(outbox_id(101)).await.unwrap().unwrap();
    assert_eq!(back.kind, OutboxKind::Delegate);
    assert_eq!(
        back.payload,
        OutboxPayload::Delegate {
            text: "\u{1F604} @researcher then @coder".to_owned(),
            release: None,
            new_job: false,
            ui_catalog: None,
            mentions: mentions.clone(),
        }
    );

    // the one without becomes a delegation without (another thread: its steer is its own to claim)
    seed(&store, &alice(), 2).await;
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![], vec![row(201, Vec::new())]),
            )
            .await
            .unwrap(),
    );
    let claimed = claim(&store, "a", at(2)).await;
    assert!(
        claimed.iter().any(|c| c.id == outbox_id(201)),
        "{claimed:?}"
    );
    assert!(
        store
            .requeue_as_delegate(&lease(201, "a", 1), at(2))
            .await
            .unwrap()
    );
    let back = store.get_outbox(outbox_id(201)).await.unwrap().unwrap();
    assert_eq!(
        back.payload,
        OutboxPayload::Delegate {
            text: "\u{1F604} @researcher then @coder".to_owned(),
            release: None,
            new_job: false,
            ui_catalog: None,
            mentions: Vec::new(),
        }
    );
}

/// The `ui_catalog` event, the thread's catalog ledger and the delivery in an outbox row are
/// stored and read back (ADR 0023): the event in every read of the log (and as the newest of its
/// kind, which is how a thread's current catalog is found), the ledger with the thread, and the
/// delivery, inline or by reference, in the row's payload.
pub async fn ui_catalog_roundtrip<S: ThreadStore>(store: S) {
    let (v1, v2) = (catalog_data(1, "chat"), catalog_data(2, "chat"));
    let catalog_event = |data: &UiCatalogData| NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UiCatalog(data.clone()),
        idempotency_key: None,
    };
    let row = |n: u128, delivery: UiDelivery| NewOutbox {
        id: outbox_id(n),
        payload: OutboxPayload::Delegate {
            text: format!("do {n}"),
            release: None,
            new_job: false,
            ui_catalog: Some(delivery),
            mentions: Vec::new(),
        },
    };

    // A thread whose first message carries a catalog: the event first, the ledger with the
    // thread, the catalog inline in the delegation.
    let mut ledger = UiCatalogLedger::default();
    ledger.accept(Some(&v1));
    let mut first = commit(
        ThreadState::Queued,
        vec![catalog_event(&v1), user_event("hi", None)],
        vec![row(1, UiDelivery::Inline(v1.clone()))],
    );
    first.job = Some(Job {
        catalog: ledger.clone(),
        ..Job::default()
    });
    let (created, events) = store
        .create_thread(new_thread(&alice(), 1), first)
        .await
        .unwrap();
    assert_eq!(created.job.catalog, ledger);
    assert_eq!(events[0].body, EventBody::UiCatalog(v1.clone()));
    assert_eq!(
        store.list_events(thread_id(1), 0, 10).await.unwrap(),
        events
    );
    let stored = store.get_outbox(outbox_id(1)).await.unwrap().unwrap();
    assert_eq!(
        stored.payload,
        OutboxPayload::Delegate {
            text: "do 1".into(),
            release: None,
            new_job: false,
            ui_catalog: Some(UiDelivery::Inline(v1.clone())),
            mentions: Vec::new(),
        }
    );

    // A newer catalog on a later message: a second event, the ledger moves, a reference later.
    ledger.accept(Some(&v2));
    let mut second = commit(
        ThreadState::Queued,
        vec![catalog_event(&v2), user_event("again", None)],
        vec![row(2, UiDelivery::Inline(v2.clone()))],
    );
    second.job = Some(Job {
        catalog: ledger.clone(),
        ..Job::default()
    });
    let (record, _) = applied(store.commit(thread_id(1), 1, second).await.unwrap());
    assert_eq!(record.job.catalog, ledger);
    assert_eq!(
        record.job.catalog.current().map(|c| c.version),
        Some(2),
        "the newest version is current"
    );
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got.job.catalog, ledger);
    let third = commit(
        ThreadState::Queued,
        vec![user_event("third", None)],
        vec![row(3, UiDelivery::Ref(v2.reference()))],
    );
    applied(store.commit(thread_id(1), 2, third).await.unwrap());
    let stored = store.get_outbox(outbox_id(3)).await.unwrap().unwrap();
    assert!(matches!(
        stored.payload,
        OutboxPayload::Delegate { ui_catalog: Some(UiDelivery::Ref(ref r)), .. } if *r == v2.reference()
    ));
    // a commit with no job leaves the ledger
    assert_eq!(
        store
            .get_thread(None, thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .job
            .catalog,
        ledger
    );

    // The newest catalog event of the thread is found without scanning the log.
    let latest = store
        .latest_events(thread_id(1), orch_core::EventKind::UiCatalog, 10)
        .await
        .unwrap();
    let digests: Vec<_> = latest
        .iter()
        .map(|e| match &e.body {
            EventBody::UiCatalog(d) => d.digest.clone(),
            other => panic!("not a catalog: {other:?}"),
        })
        .collect();
    assert_eq!(digests, [v2.digest.clone(), v1.digest.clone()]);
}

/// `ui_catalog_event`: the `ui_catalog` event with a digest is found by the digest, however many
/// events and catalogs come after it, and only on the thread asked for. The ledger records every
/// new digest and only a version at least the current one becomes current, so the current
/// catalog's event can be followed by any number of lower-versioned ones (more than a window of
/// the newest events would hold).
pub async fn ui_catalog_event_by_digest<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    let catalog_event = |data: &UiCatalogData| NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UiCatalog(data.clone()),
        idempotency_key: None,
    };
    // The current catalog (version 50) first, then 70 older ones, each its own digest, then an
    // event of another kind that has a `digest` member of its own.
    let current = catalog_data(50, "chat");
    let older: Vec<UiCatalogData> = (0..70)
        .map(|n| catalog_data(1 + n % 9, &format!("old-{n}")))
        .collect();
    let mut events = vec![catalog_event(&current)];
    events.extend(older.iter().map(catalog_event));
    events.push(user_event("after", None));
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
    // Another thread has a catalog of its own (and one that thread 1 also has).
    let other = catalog_data(7, "other");
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(
                    ThreadState::Working,
                    vec![catalog_event(&other), catalog_event(&older[0])],
                    vec![],
                ),
            )
            .await
            .unwrap(),
    );

    // far behind the newest events, and still found, whole, at its place in the log
    let found = store
        .ui_catalog_event(thread_id(1), &current.digest)
        .await
        .unwrap()
        .expect("the current catalog's event");
    // (the thread's first event is its first message: the catalogs are events 2 to 72)
    assert_eq!(found.seq, 2);
    assert_eq!(found.body, EventBody::UiCatalog(current.clone()));
    assert_eq!(found.thread_id, thread_id(1));
    let newest = store
        .latest_events(thread_id(1), orch_core::EventKind::UiCatalog, 32)
        .await
        .unwrap();
    assert!(
        newest.iter().all(|e| e.seq != 2),
        "the event is outside the newest 32 of its kind: a window would miss it"
    );
    // each of the others too
    for (n, data) in older.iter().enumerate() {
        let found = store
            .ui_catalog_event(thread_id(1), &data.digest)
            .await
            .unwrap()
            .expect("an older catalog's event");
        assert_eq!(found.seq, 3 + n as i64);
        assert_eq!(found.body, EventBody::UiCatalog(data.clone()));
    }
    // only the thread asked for: thread 2's own catalog is not thread 1's, and the one both
    // recorded is each one's own event
    assert!(
        store
            .ui_catalog_event(thread_id(1), &other.digest)
            .await
            .unwrap()
            .is_none()
    );
    let shared = store
        .ui_catalog_event(thread_id(2), &older[0].digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(shared.seq, 3);
    assert_eq!(shared.thread_id, thread_id(2));
    // a digest nobody recorded, one that is only a prefix or another case of a recorded one, and a
    // thread that does not exist
    for digest in [
        "sha256:".to_owned() + &"0".repeat(64),
        current.digest[..current.digest.len() - 1].to_owned(),
        current.digest.to_uppercase(),
        String::new(),
    ] {
        assert!(
            store
                .ui_catalog_event(thread_id(1), &digest)
                .await
                .unwrap()
                .is_none(),
            "{digest}"
        );
    }
    assert!(
        store
            .ui_catalog_event(thread_id(9), &current.digest)
            .await
            .unwrap()
            .is_none(),
        "no thread, no event"
    );
    // the same digest recorded twice (the ledger would not, a store must not care): the newest
    applied(
        store
            .commit(
                thread_id(1),
                2,
                commit(ThreadState::Working, vec![catalog_event(&current)], vec![]),
            )
            .await
            .unwrap(),
    );
    let again = store
        .ui_catalog_event(thread_id(1), &current.digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.seq, 74);
}

/// The events of the gate (and `job_started`) are stored and read back, and `verifying` is a
/// state a thread can be in.
pub async fn gate_events_roundtrip<S: ThreadStore>(store: S) {
    let sha = "b".repeat(40);
    let bodies = vec![
        EventBody::CiResult(CiReport {
            provider: CiProvider::Github,
            repository: "github.com/vymalo/repo".into(),
            sha: sha.clone(),
            branch: Some("agent/x".into()),
            name: "build".into(),
            conclusion: CiConclusion::TimedOut,
            url: Some("https://ci.example/1".into()),
            summary: None,
        }),
        EventBody::CheckResult(CheckResult {
            source: CheckSource::Verifier,
            name: None,
            attempt: 2,
            commit: Some(sha),
            status: CheckStatus::Pending,
            summary: None,
            stale: true,
            findings: vec![],
        }),
        EventBody::Rework(ReworkData {
            attempt: 2,
            max_attempts: 3,
            findings: vec![SourceFindings {
                source: CheckSource::AgentChecks,
                findings: vec!["red".into()],
            }],
        }),
        EventBody::JobStarted(orch_core::JobStartedData { job: 2 }),
    ];
    let events: Vec<NewEvent> = bodies
        .iter()
        .map(|body| NewEvent {
            at: t0(),
            actor: Actor::system(),
            body: body.clone(),
            idempotency_key: None,
        })
        .collect();
    let (record, stored) = store
        .create_thread(
            new_thread(&alice(), 1),
            commit(ThreadState::Verifying, events, vec![]),
        )
        .await
        .unwrap();
    assert_eq!(record.state, ThreadState::Verifying);
    let read = store.list_events(thread_id(1), 0, 10).await.unwrap();
    assert_eq!(read, stored);
    let read_bodies: Vec<EventBody> = read.into_iter().map(|e| e.body).collect();
    assert_eq!(read_bodies, bodies);
}

/// The `agent_step` event is stored and read back (ADR 0025): the start, an update and the end of
/// a nested step, with the path, an icon and a detail, in the order they were written, in the
/// reads of the log and as the newest of their kind.
pub async fn agent_step_roundtrip<S: ThreadStore>(store: S) {
    use orch_core::{AgentStepData, StepKind, StepPhase, StepState};
    let step =
        |id: &str, path: &[&str], kind, state, phase, icon: Option<&str>, detail: Option<&str>| {
            EventBody::AgentStep(AgentStepData {
                id: id.to_owned(),
                path: path.iter().map(|p| (*p).to_owned()).collect(),
                kind,
                label: format!("label of {id}"),
                state,
                phase,
                icon: icon.map(str::to_owned),
                detail: detail.map(str::to_owned),
                input: None,
                output: None,
                io_dropped: false,
            })
        };
    let mut bodies = vec![
        step(
            "t/tool:c1",
            &[],
            StepKind::Subagent,
            StepState::Running,
            StepPhase::Start,
            Some("agent"),
            None,
        ),
        step(
            "t/acp:1",
            &["t/tool:c1"],
            StepKind::Command,
            StepState::Running,
            StepPhase::Start,
            Some("execute"),
            None,
        ),
        step(
            "t/acp:1",
            &["t/tool:c1"],
            StepKind::Command,
            StepState::Waiting,
            StepPhase::Update,
            None,
            Some("waiting for a permission"),
        ),
        step(
            "t/acp:1",
            &["t/tool:c1"],
            StepKind::Command,
            StepState::Failed,
            StepPhase::End,
            Some("execute"),
            Some("1 failed"),
        ),
        step(
            "t/tool:c1",
            &[],
            StepKind::Subagent,
            StepState::Completed,
            StepPhase::End,
            Some("agent"),
            None,
        ),
    ];
    // the start of the command says what it was called with, its end what it returned (ADR 0030)
    if let EventBody::AgentStep(d) = &mut bodies[1] {
        d.input = serde_json::json!({"command": "npm test", "cwd": "web"})
            .as_object()
            .cloned();
    }
    if let EventBody::AgentStep(d) = &mut bodies[3] {
        d.output = Some(orch_core::StepOutput {
            text: "1 failed\nexit 1 \u{1f600}".to_owned(),
            truncated: true,
            bytes: Some(20_000),
            error: true,
        });
        d.io_dropped = true;
    }
    let events: Vec<NewEvent> = bodies
        .iter()
        .map(|body| NewEvent {
            at: t0(),
            actor: Actor::agent(&AgentId::new("coder"), Some("rev-1".into())),
            body: body.clone(),
            idempotency_key: None,
        })
        .collect();
    let (record, stored) = store
        .create_thread(
            new_thread(&alice(), 1),
            commit(ThreadState::Working, events, vec![]),
        )
        .await
        .unwrap();
    assert_eq!(record.state, ThreadState::Working);
    let read = store.list_events(thread_id(1), 0, 10).await.unwrap();
    assert_eq!(read, stored);
    assert_eq!(
        read.iter().map(|e| e.body.clone()).collect::<Vec<_>>(),
        bodies
    );
    let latest = store
        .latest_events(thread_id(1), orch_core::EventKind::AgentStep, 2)
        .await
        .unwrap();
    assert_eq!(
        latest.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [5, 4],
        "newest first"
    );
}

// ---------------------------------------------------------------------------------- inbox

fn inbox_id(n: u128) -> InboxId {
    InboxId(Uuid::from_u128(
        0x0190_0000_0000_7000_a000_0000_0000_0000 + n,
    ))
}

const REPO: &str = "github.com/o/r";

/// The watch key of CI on the commit whose hash is `n` repeated.
fn watch(n: u8) -> WatchKey {
    WatchKey::ci(REPO, &format!("{n:02x}").repeat(20))
}

fn report(n: u8) -> CiReport {
    CiReport {
        provider: CiProvider::Github,
        repository: REPO.to_owned(),
        sha: format!("{n:02x}").repeat(20),
        branch: Some("agent/x".to_owned()),
        name: "build".to_owned(),
        conclusion: CiConclusion::Success,
        url: None,
        summary: Some("green".to_owned()),
    }
}

/// A CI report row from `github` under `key`, matched by the watch `n`.
fn ci_row(id: u128, key: &str, n: u8) -> NewInbox {
    NewInbox {
        id: inbox_id(id),
        source: "github".to_owned(),
        idempotency_key: key.to_owned(),
        payload: InboxPayload::CiReport(report(n)),
        correlation: Some(watch(n).as_str().to_owned()),
    }
}

fn ilease(id: u128, owner: &str, attempt: u32) -> InboxLease {
    InboxLease {
        id: inbox_id(id),
        owner: owner.to_owned(),
        attempt,
    }
}

async fn iclaim<S: ThreadStore>(store: &S, owner: &str, now: Timestamp) -> Vec<crate::InboxItem> {
    store.claim_inbox(owner, now, LEASE, 100).await.unwrap()
}

fn ids(items: &[crate::InboxItem]) -> Vec<InboxId> {
    items.iter().map(|i| i.id).collect()
}

fn ci_deadline(id: u128, after_secs: i64, attempt: u32, verification: u32) -> NewTimer {
    NewTimer {
        id: inbox_id(id),
        after: SignedDuration::from_secs(after_secs),
        timer: Timer::CiDeadline {
            attempt,
            verification,
        },
    }
}

/// The same `(source, key)` twice is one row and the second receive says so, in whatever status
/// the row has become; another source with the same key is another delivery.
pub async fn inbox_dedupes_by_source_and_key<S: ThreadStore>(store: S) {
    let first = ci_row(1, "delivery-1", 1);
    assert_eq!(
        store.receive(first.clone(), t0()).await.unwrap(),
        Received::Stored { id: inbox_id(1) }
    );
    let redelivery = NewInbox {
        id: inbox_id(2),
        ..first.clone()
    };
    assert_eq!(
        store.receive(redelivery.clone(), at(5)).await.unwrap(),
        Received::Duplicate
    );
    assert!(store.get_inbox(inbox_id(2)).await.unwrap().is_none());
    let other_source = NewInbox {
        id: inbox_id(3),
        source: "generic".to_owned(),
        ..first.clone()
    };
    assert_eq!(
        store.receive(other_source, t0()).await.unwrap(),
        Received::Stored { id: inbox_id(3) }
    );

    let row = store
        .find_inbox("github", "delivery-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.id, inbox_id(1));
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!(
        (row.available_at, row.created_at, row.attempts),
        (t0(), t0(), 0)
    );
    assert_eq!(row.kind, "ci_report");
    assert_eq!(row.correlation.as_deref(), Some(watch(1).as_str()));
    assert_eq!(row.decode().unwrap(), first.payload);
    assert!(row.lease().is_none());
    assert_eq!(store.get_inbox(inbox_id(1)).await.unwrap().unwrap(), row);

    // Two rows to claim, not three; and finishing a row does not free its key.
    let got = iclaim(&store, "a", t0()).await;
    assert_eq!(ids(&got), vec![inbox_id(1), inbox_id(3)]);
    assert!(
        store
            .complete_inbox(&ilease(1, "a", 1), InboxFinal::Applied, at(1))
            .await
            .unwrap()
    );
    assert_eq!(
        store.receive(redelivery, at(9)).await.unwrap(),
        Received::Duplicate
    );
}

/// A claim is a lease: nobody else gets the row while it is valid, another worker gets it once
/// it lapsed (with a larger `attempts`), and the first worker's every write is then refused.
pub async fn inbox_claims_are_leases_and_lapse<S: ThreadStore>(store: S) {
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    let got = iclaim(&store, "a", t0()).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].status, InboxStatus::Inflight);
    assert_eq!(got[0].attempts, 1);
    assert_eq!(got[0].lease_owner.as_deref(), Some("a"));
    assert_eq!(got[0].lease_until, Some(at(30)));
    assert_eq!(got[0].lease(), Some(ilease(1, "a", 1)));
    assert!(
        iclaim(&store, "b", at(29)).await.is_empty(),
        "lease still valid"
    );

    let again = iclaim(&store, "b", at(31)).await;
    assert_eq!(ids(&again), vec![inbox_id(1)]);
    assert_eq!(again[0].attempts, 2);
    assert_eq!(again[0].lease_owner.as_deref(), Some("b"));

    let stale = ilease(1, "a", 1);
    assert!(
        !store
            .retry_inbox(&stale, at(40), "late".into())
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_inbox(&stale, InboxFinal::Applied, at(32))
            .await
            .unwrap()
    );
    assert_eq!(
        store.park_inbox(&stale, at(32)).await.unwrap(),
        Parking::Lost
    );
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(
        (row.status, row.lease_owner.as_deref(), row.attempts),
        (InboxStatus::Inflight, Some("b"), 2),
        "the late worker changed nothing"
    );
    assert_eq!(row.last_error, None);

    // The same owner claiming again is a new claim: its old token is stale too.
    let third = iclaim(&store, "b", at(62)).await;
    assert_eq!(third[0].attempts, 3);
    assert!(
        !store
            .complete_inbox(&ilease(1, "b", 2), InboxFinal::Applied, at(63))
            .await
            .unwrap()
    );
    assert!(
        store
            .complete_inbox(&ilease(1, "b", 3), InboxFinal::Applied, at(63))
            .await
            .unwrap()
    );
    assert!(
        !store
            .complete_inbox(&ilease(1, "b", 3), InboxFinal::Applied, at(64))
            .await
            .unwrap(),
        "a finished row has no claim"
    );
    assert!(iclaim(&store, "c", at(1000)).await.is_empty());
}

/// Concurrent claimers get disjoint rows, and together they get all of them.
pub async fn inbox_claimers_never_share_a_row<S: ThreadStore>(store: S) {
    for n in 1..=20 {
        store
            .receive(ci_row(n, &format!("d{n}"), 1), t0())
            .await
            .unwrap();
    }
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for w in 0..4 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            let owner = format!("w{w}");
            let mut mine = Vec::new();
            loop {
                let got = store.claim_inbox(&owner, t0(), LEASE, 3).await.unwrap();
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
    assert_eq!(all.len(), 20);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 20, "a row was claimed twice");
}

/// A report that finds no watch is parked; a commit that adds the watch re-arms it in the same
/// transaction. A commit that is refused adds neither the watch nor the re-arm.
pub async fn inbox_parks_and_rearms_in_one_commit<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let key = watch(1);
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    let got = iclaim(&store, "a", t0()).await;
    assert_eq!(got.len(), 1);
    assert_eq!(store.get_watch(key.as_str()).await.unwrap(), None);
    assert_eq!(
        store.park_inbox(&ilease(1, "a", 1), at(1)).await.unwrap(),
        Parking::Parked
    );
    let parked = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(parked.status, InboxStatus::Parked);
    assert_eq!(parked.parked_at, Some(at(1)));
    assert!(parked.lease().is_none() && parked.lease_until.is_none());
    assert!(
        iclaim(&store, "a", at(100)).await.is_empty(),
        "a parked row is not claimable"
    );

    // Refused commits: the version is wrong, or the idempotency key was used.
    let mut wrong_version = commit(ThreadState::Working, vec![], vec![]);
    wrong_version.watches = vec![key.clone()];
    assert_eq!(
        class_of(&store.commit(thread_id(1), 99, wrong_version).await),
        Some(ErrorClass::Conflict)
    );
    applied(
        store
            .commit(
                thread_id(1),
                1,
                commit(
                    ThreadState::Working,
                    vec![user_event("k", Some("k"))],
                    vec![],
                ),
            )
            .await
            .unwrap(),
    );
    let mut repeat = commit(
        ThreadState::Working,
        vec![user_event("k again", Some("k"))],
        vec![],
    );
    repeat.watches = vec![key.clone()];
    assert_eq!(
        store.commit(thread_id(1), 2, repeat).await.unwrap(),
        CommitOutcome::Duplicate
    );
    assert_eq!(store.get_watch(key.as_str()).await.unwrap(), None);
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Parked,
        "a refused commit re-arms nothing"
    );

    // The commit that lands inserts the watch and re-arms the row, together.
    let mut adds = commit(ThreadState::Working, vec![], vec![]);
    adds.watches = vec![key.clone()];
    adds.now = at(200);
    applied(store.commit(thread_id(1), 2, adds).await.unwrap());
    assert_eq!(
        store.get_watch(key.as_str()).await.unwrap(),
        Some(thread_id(1))
    );
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!((row.available_at, row.parked_at), (at(200), None));
    assert!(iclaim(&store, "a", at(199)).await.is_empty());
    let again = iclaim(&store, "a", at(200)).await;
    assert_eq!(ids(&again), vec![inbox_id(1)]);
    assert_eq!(again[0].attempts, 2);

    // Watching again is harmless.
    let mut repeat = commit(ThreadState::Working, vec![], vec![]);
    repeat.watches = vec![key.clone()];
    applied(store.commit(thread_id(1), 3, repeat).await.unwrap());
    assert_eq!(
        store.get_watch(key.as_str()).await.unwrap(),
        Some(thread_id(1))
    );
}

/// A worker that looked for a watch, found none, and parks after a commit added it: the row is
/// not left parked behind its watch, it goes back to `pending`.
pub async fn inbox_park_finds_a_watch_that_appeared<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    // A row with no correlation cannot be matched by any watch.
    store
        .receive(
            NewInbox {
                correlation: None,
                ..ci_row(2, "d2", 1)
            },
            t0(),
        )
        .await
        .unwrap();
    assert_eq!(iclaim(&store, "a", t0()).await.len(), 2);

    // The row is inflight, so the commit that adds the watch does not see it as parked.
    let mut adds = commit(ThreadState::Working, vec![], vec![]);
    adds.watches = vec![watch(1)];
    adds.now = at(10);
    applied(store.commit(thread_id(1), 1, adds).await.unwrap());
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Inflight
    );

    assert_eq!(
        store.park_inbox(&ilease(1, "a", 1), at(11)).await.unwrap(),
        Parking::Rearmed
    );
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!(row.available_at, at(11));
    assert!(row.lease().is_none() && row.parked_at.is_none());
    assert_eq!(ids(&iclaim(&store, "b", at(11)).await), vec![inbox_id(1)]);

    assert_eq!(
        store.park_inbox(&ilease(2, "a", 1), at(11)).await.unwrap(),
        Parking::Parked
    );
}

/// A commit under an inbox claim marks the row `applied` in the same transaction; once the
/// claim is gone (finished or taken over) the commit is `Fenced`, before the version is looked at.
pub async fn inbox_commit_is_fenced_and_marks_applied<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    iclaim(&store, "a", t0()).await;

    let mut c = commit(
        ThreadState::Working,
        vec![user_event("from the inbox", Some("inbox:1"))],
        vec![],
    );
    c.inbox = Some(ilease(1, "a", 1));
    c.now = at(3);
    let (record, events) = applied(store.commit(thread_id(1), 1, c.clone()).await.unwrap());
    assert_eq!((record.version, events.len()), (2, 1));
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Applied);
    assert!(row.lease().is_none() && row.lease_until.is_none());

    // The row is finished: the same claim is gone, even with a version that would have matched,
    // and even with a wrong one (fenced is decided first, like the outbox claim).
    assert_eq!(
        store.commit(thread_id(1), 2, c.clone()).await.unwrap(),
        CommitOutcome::Fenced
    );
    assert_eq!(
        store.commit(thread_id(1), 99, c).await.unwrap(),
        CommitOutcome::Fenced
    );
    assert!(iclaim(&store, "b", at(1000)).await.is_empty());
    let thread = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(thread.version, 2, "fenced commits wrote nothing");
}

/// A commit under a stale inbox claim writes nothing at all: no event, no state, no watch, no
/// timer, no binding, no outbox row, and the row stays with its current holder.
pub async fn inbox_stale_lease_writes_nothing<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    iclaim(&store, "a", t0()).await;
    // The same owner name claims it again after the lease lapsed: (a, 1) is stale though the
    // owner matches.
    let again = iclaim(&store, "a", at(31)).await;
    assert_eq!(again[0].attempts, 2);

    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    let events = store.list_events(thread_id(1), 0, 100).await.unwrap();
    let binding = store.get_binding(thread_id(1)).await.unwrap().unwrap();
    let open = store.list_open_outbox(thread_id(1)).await.unwrap();
    let mut late = commit(
        ThreadState::Done,
        vec![user_event("late", Some("inbox:1"))],
        vec![delegate(900)],
    );
    late.inbox = Some(ilease(1, "a", 1));
    late.watches = vec![watch(7)];
    late.timers = vec![ci_deadline(50, 10, 1, 1)];
    late.binding = Some(BindingUpdate {
        task_id: Some("late-task".into()),
        ..BindingUpdate::default()
    });
    assert_eq!(
        store
            .commit(thread_id(1), before.version, late.clone())
            .await
            .unwrap(),
        CommitOutcome::Fenced
    );
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store.list_events(thread_id(1), 0, 100).await.unwrap(),
        events
    );
    assert_eq!(
        store.get_binding(thread_id(1)).await.unwrap().unwrap(),
        binding
    );
    assert_eq!(store.list_open_outbox(thread_id(1)).await.unwrap(), open);
    assert_eq!(store.get_watch(watch(7).as_str()).await.unwrap(), None);
    assert!(store.get_inbox(inbox_id(50)).await.unwrap().is_none());
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!((row.status, row.attempts), (InboxStatus::Inflight, 2));

    // The current claim commits the same thing.
    late.inbox = Some(ilease(1, "a", 2));
    applied(
        store
            .commit(thread_id(1), before.version, late)
            .await
            .unwrap(),
    );
    assert_eq!(
        store.get_watch(watch(7).as_str()).await.unwrap(),
        Some(thread_id(1))
    );
    assert!(store.get_inbox(inbox_id(50)).await.unwrap().is_some());
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Applied
    );
}

/// Parked rows expire once they were parked for long enough; an expired row is not re-armed by
/// a watch that comes later.
pub async fn parked_rows_expire<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    store.receive(ci_row(2, "d2", 2), t0()).await.unwrap();
    iclaim(&store, "a", t0()).await;
    assert_eq!(
        store.park_inbox(&ilease(1, "a", 1), at(1)).await.unwrap(),
        Parking::Parked
    );
    assert_eq!(
        store.park_inbox(&ilease(2, "a", 1), at(50)).await.unwrap(),
        Parking::Parked
    );

    assert_eq!(store.expire_parked_inbox(at(0), at(100)).await.unwrap(), 0);
    assert_eq!(store.expire_parked_inbox(at(10), at(100)).await.unwrap(), 1);
    let expired = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(expired.status, InboxStatus::Expired);
    assert_eq!(
        store.get_inbox(inbox_id(2)).await.unwrap().unwrap().status,
        InboxStatus::Parked
    );
    assert_eq!(store.expire_parked_inbox(at(10), at(101)).await.unwrap(), 0);

    // The watch for the expired row comes late: the row stays expired.
    let mut adds = commit(ThreadState::Working, vec![], vec![]);
    adds.watches = vec![watch(1), watch(2)];
    adds.now = at(60);
    applied(store.commit(thread_id(1), 1, adds).await.unwrap());
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Expired
    );
    assert_eq!(
        store.get_inbox(inbox_id(2)).await.unwrap().unwrap().status,
        InboxStatus::Pending,
        "the row that did not expire was re-armed"
    );
    assert!(store.receive(ci_row(3, "d1", 1), at(70)).await.unwrap() == Received::Duplicate);
}

/// A timer is an inbox row due `after` the commit's time; it is not claimed before that, and is
/// claimed from then on.
pub async fn timer_is_claimed_only_when_due<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let timer = ci_deadline(10, 60, 1, 1);
    let key = timer.idempotency_key(thread_id(1));
    let mut c = commit(ThreadState::Working, vec![], vec![]);
    c.timers = vec![timer.clone(), ci_deadline(11, 0, 1, 2)];
    c.now = at(5);
    applied(store.commit(thread_id(1), 1, c).await.unwrap());

    let row = store.find_inbox(TIMER_SOURCE, &key).await.unwrap().unwrap();
    assert_eq!(row.id, inbox_id(10));
    assert_eq!(
        (row.status, row.available_at),
        (InboxStatus::Pending, at(65))
    );
    assert_eq!((row.source.as_str(), row.kind.as_str()), ("timer", "timer"));
    assert_eq!(row.correlation, None);
    assert_eq!(
        row.decode().unwrap(),
        InboxPayload::Timer {
            thread: thread_id(1),
            timer: timer.timer,
        }
    );

    // The timer with no delay is due at once; the other one is not.
    assert_eq!(ids(&iclaim(&store, "a", at(5)).await), vec![inbox_id(11)]);
    assert!(
        store
            .complete_inbox(&ilease(11, "a", 1), InboxFinal::Applied, at(6))
            .await
            .unwrap()
    );
    assert!(iclaim(&store, "a", at(64)).await.is_empty(), "not due yet");
    let due = iclaim(&store, "a", at(65)).await;
    assert_eq!(ids(&due), vec![inbox_id(10)]);
    assert_eq!(due[0].attempts, 1);
}

/// Replaying a commit arms no second timer: the replay is a duplicate, and a commit that arms
/// the same timer again by another route is one row.
pub async fn a_replayed_commit_arms_no_second_timer<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    let mut c = commit(
        ThreadState::Verifying,
        vec![user_event("start", Some("k"))],
        vec![],
    );
    c.timers = vec![ci_deadline(10, 60, 1, 1)];
    applied(store.commit(thread_id(1), 1, c.clone()).await.unwrap());

    // The replay: same key, timer under another id.
    let mut replay = c.clone();
    replay.timers = vec![ci_deadline(11, 60, 1, 1)];
    assert_eq!(
        store.commit(thread_id(1), 2, replay).await.unwrap(),
        CommitOutcome::Duplicate
    );
    assert!(store.get_inbox(inbox_id(11)).await.unwrap().is_none());

    // The same timer armed by a commit that carries no key is still the same row.
    let mut second = commit(ThreadState::Verifying, vec![], vec![]);
    second.timers = vec![ci_deadline(12, 60, 1, 1), ci_deadline(13, 60, 1, 2)];
    applied(store.commit(thread_id(1), 2, second).await.unwrap());
    assert!(store.get_inbox(inbox_id(12)).await.unwrap().is_none());
    assert!(
        store.get_inbox(inbox_id(13)).await.unwrap().is_some(),
        "another verification is another timer"
    );

    let due = iclaim(&store, "a", at(60)).await;
    assert_eq!(ids(&due), vec![inbox_id(10), inbox_id(13)]);
}

/// Retry puts a row back with a delay and the error; complete finishes it; a dead row stays
/// dead; releasing a worker's leases makes its rows claimable at once.
pub async fn inbox_retry_complete_and_release<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        store
            .receive(ci_row(n, &format!("d{n}"), 1), t0())
            .await
            .unwrap();
    }
    assert_eq!(iclaim(&store, "a", t0()).await.len(), 3);

    assert!(
        store
            .retry_inbox(&ilease(1, "a", 1), at(10), "boom".into())
            .await
            .unwrap()
    );
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(
        (row.status, row.available_at, row.last_error.as_deref()),
        (InboxStatus::Pending, at(10), Some("boom"))
    );
    assert!(row.lease().is_none() && row.lease_until.is_none());

    assert!(
        store
            .complete_inbox(
                &ilease(2, "a", 1),
                InboxFinal::Dead {
                    error: "bad payload".into()
                },
                at(1)
            )
            .await
            .unwrap()
    );
    let dead = store.get_inbox(inbox_id(2)).await.unwrap().unwrap();
    assert_eq!(
        (dead.status, dead.last_error.as_deref()),
        (InboxStatus::Dead, Some("bad payload"))
    );

    // Only what `a` still holds is released.
    assert_eq!(store.release_inbox_leases("a", at(3)).await.unwrap(), 1);
    assert_eq!(
        store.release_inbox_leases("nobody", at(3)).await.unwrap(),
        0
    );
    assert_eq!(ids(&iclaim(&store, "b", at(3)).await), vec![inbox_id(3)]);
    assert!(iclaim(&store, "b", at(9)).await.is_empty());
    let retried = iclaim(&store, "b", at(10)).await;
    assert_eq!(ids(&retried), vec![inbox_id(1)]);
    assert_eq!(retried[0].attempts, 2);
    assert!(
        iclaim(&store, "c", at(100_000))
            .await
            .iter()
            .all(|r| r.id != inbox_id(2)),
        "a dead row is never claimed"
    );
}

/// A thread's first commit can arm timers and watches, and re-arms what was parked for them.
pub async fn create_thread_arms_timers_and_watches<S: ThreadStore>(store: S) {
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    iclaim(&store, "a", t0()).await;
    store.park_inbox(&ilease(1, "a", 1), at(1)).await.unwrap();

    let mut first = commit(ThreadState::Queued, vec![user_event("hi", None)], vec![]);
    first.watches = vec![watch(1)];
    first.timers = vec![ci_deadline(10, 30, 1, 1)];
    first.now = at(2);
    store
        .create_thread(new_thread(&alice(), 1), first)
        .await
        .unwrap();
    assert_eq!(
        store.get_watch(watch(1).as_str()).await.unwrap(),
        Some(thread_id(1))
    );
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Pending
    );
    let timer = store.get_inbox(inbox_id(10)).await.unwrap().unwrap();
    assert_eq!(timer.available_at, at(32));
}

/// A key is watched by one thread: the first to insert it keeps it.
pub async fn watches_are_first_come<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &bob(), 2).await;
    let mut one = commit(ThreadState::Working, vec![], vec![]);
    one.watches = vec![watch(1), watch(1)];
    applied(store.commit(thread_id(1), 1, one).await.unwrap());
    let mut two = commit(ThreadState::Working, vec![], vec![]);
    two.watches = vec![watch(1), watch(2)];
    applied(store.commit(thread_id(2), 1, two).await.unwrap());
    assert_eq!(
        store.get_watch(watch(1).as_str()).await.unwrap(),
        Some(thread_id(1))
    );
    assert_eq!(
        store.get_watch(watch(2).as_str()).await.unwrap(),
        Some(thread_id(2))
    );
    assert_eq!(store.get_watch(watch(3).as_str()).await.unwrap(), None);
}

/// Only claims that failed count against the attempt limit. `attempts` is a fencing token and
/// only goes up; `refunded` gives back a claim handed back at shutdown, and every claim up to
/// the one that parked the row (or found the watch there), so neither uses up the limit. A lapse
/// and a retry are failures: they stay counted.
pub async fn inbox_counts_only_the_claims_that_failed<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;

    // A claim handed back at shutdown, over and over (a rolling deploy): never counted.
    store.receive(ci_row(1, "d1", 1), t0()).await.unwrap();
    let first = iclaim(&store, "a", t0()).await;
    assert_eq!(
        (
            first[0].attempts,
            first[0].refunded,
            first[0].counted_attempts()
        ),
        (1, 0, 1)
    );
    let mut holder = "a".to_owned();
    for round in 1..=5_u32 {
        assert_eq!(
            store.release_inbox_leases(&holder, at(1)).await.unwrap(),
            1,
            "round {round}"
        );
        let released = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
        assert_eq!(released.counted_attempts(), 0, "round {round}");
        holder = format!("r{round}");
        let next = iclaim(&store, &holder, at(1)).await;
        assert_eq!(next.len(), 1, "round {round}");
        assert_eq!(next[0].attempts, round + 1, "a token that only goes up");
        assert_eq!(
            next[0].counted_attempts(),
            1,
            "handed-back claims are not counted"
        );
    }
    // Only the owner's own claims are refunded.
    assert_eq!(
        store.release_inbox_leases("nobody", at(2)).await.unwrap(),
        0
    );
    assert_eq!(
        store
            .get_inbox(inbox_id(1))
            .await
            .unwrap()
            .unwrap()
            .counted_attempts(),
        1
    );

    // A lapse is a failure, and so is a retry: both stay counted.
    store.receive(ci_row(2, "d2", 2), at(10)).await.unwrap();
    let a = iclaim(&store, "a", at(10)).await;
    let a = a.iter().find(|r| r.id == inbox_id(2)).unwrap();
    assert_eq!(a.counted_attempts(), 1);
    let b = iclaim(&store, "b", at(41)).await;
    let b = b.iter().find(|r| r.id == inbox_id(2)).unwrap();
    assert_eq!((b.attempts, b.counted_attempts()), (2, 2), "a lapse counts");
    assert!(
        store
            .retry_inbox(&ilease(2, "b", 2), at(50), "boom".into())
            .await
            .unwrap()
    );
    let c = iclaim(&store, "c", at(50)).await;
    let c = c.iter().find(|r| r.id == inbox_id(2)).unwrap();
    assert_eq!((c.attempts, c.counted_attempts()), (3, 3), "a retry counts");

    // Parking is a wait, not a failure: the claims so far are refunded, and a later re-arm by
    // a commit that adds the watch starts the row afresh.
    store.receive(ci_row(3, "d3", 3), at(60)).await.unwrap();
    let a = iclaim(&store, "a", at(60)).await;
    assert_eq!(a.iter().find(|r| r.id == inbox_id(3)).unwrap().attempts, 1);
    let b = iclaim(&store, "b", at(91)).await;
    let b = b.iter().find(|r| r.id == inbox_id(3)).unwrap();
    assert_eq!((b.attempts, b.counted_attempts()), (2, 2));
    assert_eq!(
        store.park_inbox(&ilease(3, "b", 2), at(92)).await.unwrap(),
        Parking::Parked
    );
    let parked = store.get_inbox(inbox_id(3)).await.unwrap().unwrap();
    assert_eq!((parked.attempts, parked.counted_attempts()), (2, 0));
    let mut adds = commit(ThreadState::Queued, vec![], vec![]);
    adds.watches = vec![watch(3)];
    adds.now = at(100);
    applied(store.commit(thread_id(1), 1, adds).await.unwrap());
    let again = iclaim(&store, "c", at(100)).await;
    let again = again.iter().find(|r| r.id == inbox_id(3)).unwrap();
    assert_eq!(
        (again.attempts, again.counted_attempts()),
        (3, 1),
        "parked and re-armed: counted from the start"
    );
    // The same for a park that finds the watch already there.
    store.receive(ci_row(4, "d4", 3), at(110)).await.unwrap();
    let got = iclaim(&store, "a", at(110)).await;
    assert_eq!(
        got.iter().find(|r| r.id == inbox_id(4)).unwrap().attempts,
        1
    );
    let got = iclaim(&store, "b", at(141)).await;
    assert_eq!(
        got.iter()
            .find(|r| r.id == inbox_id(4))
            .unwrap()
            .counted_attempts(),
        2
    );
    assert_eq!(
        store.park_inbox(&ilease(4, "b", 2), at(142)).await.unwrap(),
        Parking::Rearmed
    );
    let row = store.get_inbox(inbox_id(4)).await.unwrap().unwrap();
    assert_eq!((row.attempts, row.counted_attempts()), (2, 0));
}

/// A commit that carries an inbox claim and nothing else finishes the row and leaves the thread
/// alone, but it is still a commit: the claim and the version are checked, so a "nothing to do"
/// decided on a thread that has moved since is refused, not written down as final.
pub async fn inbox_only_commit_finishes_the_row_and_leaves_the_thread_alone<S: ThreadStore>(
    store: S,
) {
    seed(&store, &alice(), 1).await;
    for n in 1..=3 {
        store
            .receive(ci_row(n, &format!("d{n}"), 1), t0())
            .await
            .unwrap();
    }
    assert_eq!(iclaim(&store, "a", t0()).await.len(), 3);
    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    let events = store.list_events(thread_id(1), 0, 100).await.unwrap();

    // A thread that moved since the decision: refused, the row keeps its claim.
    let mut only = commit(before.state, vec![], vec![]);
    only.inbox = Some(ilease(1, "a", 1));
    only.now = at(5);
    assert_eq!(
        class_of(
            &store
                .commit(thread_id(1), before.version + 1, only.clone())
                .await
        ),
        Some(ErrorClass::Conflict)
    );
    assert_eq!(
        store.get_inbox(inbox_id(1)).await.unwrap().unwrap().status,
        InboxStatus::Inflight
    );

    // A claim that is gone: fenced, before the version is looked at.
    let mut lost = only.clone();
    lost.inbox = Some(ilease(1, "someone else", 1));
    assert_eq!(
        store.commit(thread_id(1), 99, lost).await.unwrap(),
        CommitOutcome::Fenced
    );

    // The right version and the claim: the row is applied, the thread is as it was.
    let (record, written) = applied(
        store
            .commit(thread_id(1), before.version, only)
            .await
            .unwrap(),
    );
    assert!(written.is_empty());
    assert_eq!(record, before, "no version bump, no new updated_at");
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        store.list_events(thread_id(1), 0, 100).await.unwrap(),
        events
    );
    let row = store.get_inbox(inbox_id(1)).await.unwrap().unwrap();
    assert_eq!(row.status, InboxStatus::Applied);
    assert!(row.lease().is_none() && row.lease_until.is_none());

    // The claim is spent: the same commit again is fenced.
    let mut again = commit(before.state, vec![], vec![]);
    again.inbox = Some(ilease(1, "a", 1));
    assert_eq!(
        store
            .commit(thread_id(1), before.version, again)
            .await
            .unwrap(),
        CommitOutcome::Fenced
    );

    // Not the same state: it is a state change like any other, written and versioned.
    let mut moves = commit(ThreadState::Working, vec![], vec![]);
    moves.inbox = Some(ilease(2, "a", 1));
    moves.now = at(7);
    let (moved, _) = applied(
        store
            .commit(thread_id(1), before.version, moves)
            .await
            .unwrap(),
    );
    assert_eq!(
        (moved.state, moved.version),
        (ThreadState::Working, before.version + 1)
    );
    assert_eq!(
        store.get_inbox(inbox_id(2)).await.unwrap().unwrap().status,
        InboxStatus::Applied
    );

    // A commit with anything else in it is a full commit.
    let mut full = commit(
        ThreadState::Working,
        vec![user_event("more", Some("inbox:3"))],
        vec![],
    );
    full.inbox = Some(ilease(3, "a", 1));
    let (record, written) = applied(
        store
            .commit(thread_id(1), moved.version, full)
            .await
            .unwrap(),
    );
    assert_eq!((record.version, written.len()), (moved.version + 1, 1));
}

// ---- forks (ADR 0029) -------------------------------------------------------------------------

fn agent_event(text: &str) -> NewEvent {
    NewEvent {
        at: at(1),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::AgentMessage(orch_core::AgentMessageData {
            text: text.to_owned(),
            message_id: format!("m-{text}"),
            is_final: true,
            purpose: None,
            via: None,
        }),
        idempotency_key: None,
    }
}

fn forked_event(parent: u128, cut: i64, kind: orch_core::ForkKind) -> NewEvent {
    NewEvent {
        at: at(5),
        actor: Actor::user(&alice()),
        body: EventBody::ThreadForked(orch_core::ThreadForkedData {
            from: orch_core::ForkSource {
                thread_id: thread_id(parent),
                seq: cut,
            },
            kind,
            title: "thread".to_owned(),
            description: None,
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: None,
            },
        }),
        idempotency_key: None,
    }
}

/// A thread of four events: a message, an answer, a message, an answer; created at `n` seconds.
async fn seed_conversation<S: ThreadStore>(store: &S, owner: &UserId, n: u128, created: i64) {
    let mut new = new_thread(owner, n);
    new.now = at(created);
    store
        .create_thread(
            new,
            commit(
                ThreadState::Done,
                vec![
                    user_event("first", None),
                    agent_event("one"),
                    user_event("second", Some("k2")),
                    agent_event("two"),
                ],
                vec![],
            ),
        )
        .await
        .unwrap();
}

/// A fork of thread `parent` as thread `n`, cut after `cut` events, made at `created` seconds, whose
/// first commit is the `thread_forked` event, then `more`.
#[allow(clippy::too_many_arguments)]
async fn fork<S: ThreadStore>(
    store: &S,
    parent: u128,
    n: u128,
    cut: i64,
    kind: orch_core::ForkKind,
    created: i64,
    more: Vec<NewEvent>,
    outbox: Vec<NewOutbox>,
) -> Result<(orch_core::ThreadRecord, Vec<orch_core::Event>), StoreError> {
    let mut new = new_thread(&alice(), n);
    new.now = at(created);
    let mut events = vec![forked_event(parent, cut, kind)];
    events.extend(more);
    let state = if outbox.is_empty() {
        ThreadState::Done
    } else {
        ThreadState::Queued
    };
    let mut first = commit(state, events, outbox);
    first.now = at(created);
    store
        .fork_thread(
            new,
            crate::ForkOrigin {
                parent: thread_id(parent),
                cut,
                kind,
            },
            first,
        )
        .await
}

/// A fork is a new thread whose log starts with the parent's events up to the cut, as they are,
/// then its own: the same seq, time, actor and data, no idempotency key; the thread says where it
/// came from, has its own binding, and the parent is untouched.
pub async fn fork_copies_the_parents_log_up_to_the_cut<S: ThreadStore>(store: S) {
    use orch_core::{EventKind, ForkKind, ForkedFrom};
    seed_conversation(&store, &alice(), 1, 0).await;
    let parent_before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    let parent_events = store.list_events(thread_id(1), 0, 100).await.unwrap();
    assert_eq!(parent_events.len(), 4);

    let (record, written) = fork(&store, 1, 2, 2, ForkKind::Fork, 10, vec![], vec![])
        .await
        .unwrap();
    // what the commit wrote: the `thread_forked` event, after the copy
    assert_eq!(written.len(), 1);
    assert_eq!(
        (written[0].seq, written[0].kind()),
        (3, EventKind::ThreadForked)
    );
    assert_eq!(written[0].thread_id, thread_id(2));
    assert_eq!(
        (record.last_seq, record.version, record.state),
        (3, 1, ThreadState::Done)
    );
    assert_eq!(
        record.forked_from,
        Some(ForkedFrom {
            thread_id: Some(thread_id(1)),
            seq: 2,
            kind: ForkKind::Fork
        })
    );
    assert_eq!(record.owner, alice());
    assert_eq!(record.created_at, at(10));

    // the log: the parent's first two events, as they are, and the new event
    let events = store.list_events(thread_id(2), 0, 100).await.unwrap();
    assert_eq!(events.len(), 3);
    for (copy, original) in events.iter().zip(&parent_events).take(2) {
        assert_eq!(
            (copy.seq, copy.at, &copy.actor, &copy.body),
            (original.seq, original.at, &original.actor, &original.body)
        );
        assert_eq!(copy.thread_id, thread_id(2));
    }
    assert_eq!(events[2], written[0]);

    // read back, the thread says the same, and has its own context
    assert_eq!(
        store
            .get_thread(Some(&alice()), thread_id(2))
            .await
            .unwrap()
            .unwrap(),
        record
    );
    let binding = store.get_binding(thread_id(2)).await.unwrap().unwrap();
    assert_eq!(
        binding.context_id, None,
        "a context of its own, assigned by the agent"
    );
    assert_eq!(binding.agent_id, AgentId::new("coder"));
    assert_eq!(binding.task_id, None);

    // the parent did not change, and the idempotency key of a copied event did not travel: the
    // same key in the fork's own log is new there
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        parent_before
    );
    assert_eq!(
        store.list_events(thread_id(1), 0, 100).await.unwrap(),
        parent_events
    );
    let (_, again) = applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(
                    ThreadState::Queued,
                    vec![user_event("third", Some("k2"))],
                    vec![],
                ),
            )
            .await
            .unwrap(),
    );
    assert_eq!(again[0].seq, 4);

    // a fork of a fork copies what that one holds: its copy and its own events
    let (grand, _) = fork(&store, 2, 3, 4, ForkKind::Fork, 20, vec![], vec![])
        .await
        .unwrap();
    assert_eq!(grand.last_seq, 5);
    let events = store.list_events(thread_id(3), 0, 100).await.unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(events[2].kind(), EventKind::ThreadForked);
    assert_eq!(events[3].kind(), EventKind::UserMessage);
}

/// An edit's first commit holds the replacing message and its delegation: one transaction, the
/// event after the copy, the outbox row for the fork's own thread.
pub async fn a_fork_commits_its_own_events_and_outbox_after_the_copy<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    let (record, written) = fork(
        &store,
        1,
        2,
        2,
        ForkKind::Edit,
        10,
        vec![user_event("edited", None)],
        vec![delegate(2)],
    )
    .await
    .unwrap();
    assert_eq!(
        written.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!((record.last_seq, record.state), (4, ThreadState::Queued));
    let open = store.list_open_outbox(thread_id(2)).await.unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].thread_id, thread_id(2));
    assert!(
        store
            .list_open_outbox(thread_id(1))
            .await
            .unwrap()
            .is_empty()
    );
    // it is claimable like any row
    let claimed = claim(&store, "w", at(10)).await;
    assert_eq!(
        claimed.iter().map(|r| r.thread_id).collect::<Vec<_>>(),
        vec![thread_id(2)]
    );
}

/// Cutting at 0 copies nothing: the thread starts with its `thread_forked`, as event 1.
pub async fn a_fork_at_zero_copies_nothing<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    let (record, written) = fork(
        &store,
        1,
        2,
        0,
        ForkKind::Edit,
        10,
        vec![user_event("instead", None)],
        vec![delegate(2)],
    )
    .await
    .unwrap();
    assert_eq!(
        written.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(record.last_seq, 2);
    assert_eq!(record.forked_from.map(|f| f.seq), Some(0));
    assert_eq!(
        store.list_events(thread_id(2), 0, 100).await.unwrap().len(),
        2
    );
}

/// A thread that is not the owner's looks like a thread that does not exist, and a cut past the
/// parent's log is refused; either way nothing is written.
pub async fn a_refused_fork_writes_nothing<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    seed_conversation(&store, &bob(), 5, 0).await;

    let mut new = new_thread(&bob(), 2);
    new.now = at(10);
    let foreign = store
        .fork_thread(
            new,
            crate::ForkOrigin {
                parent: thread_id(1),
                cut: 2,
                kind: ForkKind::Fork,
            },
            commit(
                ThreadState::Done,
                vec![forked_event(1, 2, ForkKind::Fork)],
                vec![],
            ),
        )
        .await;
    assert_eq!(class_of(&foreign), Some(ErrorClass::NotFound));

    let missing = fork(&store, 77, 3, 0, ForkKind::Fork, 10, vec![], vec![]).await;
    assert_eq!(class_of(&missing), Some(ErrorClass::NotFound));

    let past = fork(&store, 1, 4, 5, ForkKind::Fork, 10, vec![], vec![]).await;
    assert_eq!(class_of(&past), Some(ErrorClass::Corrupt));
    let negative = fork(&store, 1, 6, -1, ForkKind::Fork, 10, vec![], vec![]).await;
    assert!(negative.is_err());

    // an id that exists is refused too, and the thread that has it is untouched
    let before = store.list_events(thread_id(5), 0, 100).await.unwrap();
    let taken = fork(&store, 1, 1, 2, ForkKind::Fork, 10, vec![], vec![]).await;
    assert!(taken.is_err());
    assert_eq!(
        store.list_events(thread_id(1), 0, 100).await.unwrap().len(),
        4
    );
    assert_eq!(
        store.list_events(thread_id(5), 0, 100).await.unwrap(),
        before
    );

    for n in [2, 3, 4, 6] {
        assert!(
            store
                .get_thread(None, thread_id(n))
                .await
                .unwrap()
                .is_none(),
            "thread {n} was never made"
        );
        assert!(store.get_binding(thread_id(n)).await.unwrap().is_none());
        assert!(
            store
                .list_events(thread_id(n), 0, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// A thread made by an edit is a branch of one the list shows: it is listed only when asked for,
/// and a page is as long as it can be whatever is hidden. A fork of the other kind is listed.
pub async fn list_hides_edits_unless_asked<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    fork(
        &store,
        1,
        2,
        2,
        ForkKind::Edit,
        10,
        vec![user_event("e", None)],
        vec![delegate(2)],
    )
    .await
    .unwrap();
    fork(&store, 1, 3, 4, ForkKind::Fork, 20, vec![], vec![])
        .await
        .unwrap();
    fork(
        &store,
        1,
        4,
        0,
        ForkKind::Edit,
        30,
        vec![user_event("e", None)],
        vec![delegate(4)],
    )
    .await
    .unwrap();
    seed_conversation(&store, &alice(), 5, 40).await;

    let ids = |list: Vec<orch_core::ThreadRecord>| -> Vec<ThreadId> {
        list.into_iter().map(|t| t.id).collect()
    };
    assert_eq!(
        ids(store
            .list_threads(&alice(), crate::ThreadListing::recent(None, 50, false))
            .await
            .unwrap()),
        vec![thread_id(5), thread_id(3), thread_id(1)]
    );
    assert_eq!(
        ids(store
            .list_threads(&alice(), crate::ThreadListing::recent(None, 50, true))
            .await
            .unwrap()),
        vec![
            thread_id(5),
            thread_id(4),
            thread_id(3),
            thread_id(2),
            thread_id(1)
        ]
    );
    // the limit counts what is listed, and a cursor may be a hidden thread
    assert_eq!(
        ids(store
            .list_threads(&alice(), crate::ThreadListing::recent(None, 2, false))
            .await
            .unwrap()),
        vec![thread_id(5), thread_id(3)]
    );
    assert_eq!(
        ids(store
            .list_threads(
                &alice(),
                crate::ThreadListing::recent(Some(thread_id(4)), 50, false)
            )
            .await
            .unwrap()),
        vec![thread_id(3), thread_id(1)]
    );
}

/// The family of edits of a thread: the thread it started from and every edit made from those,
/// oldest first, each with its parent, its cut and the seq of its replacing message. A fork of the
/// other kind starts a family of its own; a thread that is not the owner's has none.
pub async fn fork_family_follows_edits<S: ThreadStore>(store: S) {
    use orch_core::{EditLink, ForkKind};
    seed_conversation(&store, &alice(), 1, 0).await;
    // 2 and 3 replace the message at 3 of 1; 4 replaces the message at 4 of 2 (an edit of an edit,
    // whose replacing message comes after an event of its own, so it is not at cut + 2); 5 is a
    // fork of 1; 6 an edit of 5
    fork(
        &store,
        1,
        2,
        2,
        ForkKind::Edit,
        10,
        vec![user_event("b", None)],
        vec![delegate(2)],
    )
    .await
    .unwrap();
    fork(
        &store,
        1,
        3,
        2,
        ForkKind::Edit,
        20,
        vec![user_event("c", None)],
        vec![delegate(3)],
    )
    .await
    .unwrap();
    // 2 has: 1 2 (copied), 3 thread_forked, 4 "b"; 4 edits that message (cut 3), and an event of
    // its own comes before its replacing message: thread_forked 4, "noise" 5, "d" 6
    fork(
        &store,
        2,
        4,
        3,
        ForkKind::Edit,
        30,
        vec![agent_event("noise"), user_event("d", None)],
        vec![delegate(4)],
    )
    .await
    .unwrap();
    fork(&store, 1, 5, 4, ForkKind::Fork, 40, vec![], vec![])
        .await
        .unwrap();
    fork(
        &store,
        5,
        6,
        3,
        ForkKind::Edit,
        50,
        vec![user_event("e", None)],
        vec![delegate(6)],
    )
    .await
    .unwrap();

    let family = |n: u128| {
        let store = &store;
        async move { store.fork_family(&alice(), thread_id(n)).await.unwrap() }
    };
    let link = |parent: u128, cut: i64, message: i64| {
        Some(EditLink {
            parent: thread_id(parent),
            cut,
            message,
        })
    };
    let expected = vec![
        (thread_id(1), None, at(0)),
        (thread_id(2), link(1, 2, 4), at(10)),
        (thread_id(3), link(1, 2, 4), at(20)),
        (thread_id(4), link(2, 3, 6), at(30)),
    ];
    for member in [1, 2, 3, 4] {
        let got: Vec<_> = family(member)
            .await
            .into_iter()
            .map(|n| (n.id, n.link, n.created))
            .collect();
        assert_eq!(got, expected, "the family of thread {member}");
    }
    // the fork of 1 is a root of its own, with its one edit
    let got: Vec<_> = family(6)
        .await
        .into_iter()
        .map(|n| (n.id, n.link))
        .collect();
    assert_eq!(
        got,
        vec![(thread_id(5), None), (thread_id(6), link(5, 3, 5))]
    );
    assert_eq!(family(5).await.len(), 2);
    // somebody else's, or nothing: no family
    assert!(
        store
            .fork_family(&bob(), thread_id(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(family(77).await.is_empty());
    // a thread that was never forked is a family of one
    seed_conversation(&store, &alice(), 8, 60).await;
    let alone = family(8).await;
    assert_eq!(alone.len(), 1);
    assert_eq!((alone[0].id, alone[0].link), (thread_id(8), None));
}

// ---- sharing (ADR 0040) -------------------------------------------------------------------

fn nonce(n: u8) -> orch_core::ShareNonce {
    orch_core::ShareNonce::new([n; orch_core::NONCE_LEN])
}

/// The commit that shares a thread at `level` with `nonce`, as `Command::SetSharing` makes it:
/// the `thread_shared` event and the row's new share.
fn share_commit(
    state: ThreadState,
    level: orch_core::ShareLevel,
    nonce: orch_core::ShareNonce,
    now: i64,
) -> Commit {
    let mut c = commit(
        state,
        vec![NewEvent {
            at: at(now),
            actor: Actor::user(&alice()),
            body: EventBody::ThreadShared(orch_core::ThreadSharedData {
                visibility: level,
                nonce_sha256: nonce.sha256_hex(),
            }),
            idempotency_key: None,
        }],
        vec![],
    );
    c.now = at(now);
    c.sharing = Some(SharingChange::Set { level, nonce });
    c
}

/// The commit that takes the link down, as `Command::ClearSharing` makes it.
fn unshare_commit(state: ThreadState, now: i64) -> Commit {
    let mut c = commit(
        state,
        vec![NewEvent {
            at: at(now),
            actor: Actor::user(&alice()),
            body: EventBody::ThreadUnshared(orch_core::ThreadUnsharedData {}),
            idempotency_key: None,
        }],
        vec![],
    );
    c.now = at(now);
    c.sharing = Some(SharingChange::Clear);
    c
}

/// A thread is found by the nonce of its share, with the share as the row holds it, and the
/// event that says so is in its log: one commit.
pub async fn a_shared_thread_is_found_by_its_nonce<S: ThreadStore>(store: S) {
    use orch_core::{ShareLevel, ThreadShare, Visibility};
    seed(&store, &alice(), 1).await;
    seed(&store, &bob(), 2).await;
    let before = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(
        (before.share, before.visibility()),
        (None, Visibility::Private)
    );
    assert_eq!(store.thread_by_share_nonce(&[7; 16]).await.unwrap(), None);

    let c = share_commit(ThreadState::Queued, ShareLevel::Internal, nonce(7), 5);
    let (record, events) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!(
        record.share,
        Some(ThreadShare {
            level: ShareLevel::Internal,
            nonce: nonce(7),
            shared_at: at(5),
        })
    );
    assert_eq!(record.visibility(), Visibility::Internal);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind(), orch_core::EventKind::ThreadShared);

    // found by the nonce, and by nothing else
    let found = store.thread_by_share_nonce(&[7; 16]).await.unwrap();
    assert_eq!(found, Some(record.clone()));
    assert_eq!(store.thread_by_share_nonce(&[8; 16]).await.unwrap(), None);
    // read back by the id and in the owner's list, the thread says the same
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        record
    );
    assert_eq!(
        store
            .list_threads(&alice(), crate::ThreadListing::recent(None, 10, false))
            .await
            .unwrap()[0],
        record
    );
    // another person's thread is not shared by it
    let other = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(other.share, None);
    // the event reads back as written
    let log = store.list_events(thread_id(1), 1, 10).await.unwrap();
    assert_eq!(log, events);
}

/// Taking a share down makes the nonce find nothing, makes the thread private again, and the
/// `thread_unshared` event is in the log; a share made after it is another nonce's.
pub async fn a_revoked_share_is_not_found<S: ThreadStore>(store: S) {
    use orch_core::{ShareLevel, Visibility};
    seed(&store, &alice(), 1).await;
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(1), 5);
    let (record, _) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert!(
        store
            .thread_by_share_nonce(&[1; 16])
            .await
            .unwrap()
            .is_some()
    );

    let c = unshare_commit(ThreadState::Queued, 6);
    let (record, events) = applied(store.commit(thread_id(1), record.version, c).await.unwrap());
    assert_eq!(record.share, None);
    assert_eq!(record.visibility(), Visibility::Private);
    assert_eq!(events[0].kind(), orch_core::EventKind::ThreadUnshared);
    assert_eq!(store.thread_by_share_nonce(&[1; 16]).await.unwrap(), None);
    let got = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(got.share, None);

    // sharing again, with a new nonce: the old one stays dead, the new one finds the thread
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(2), 7);
    let (record, _) = applied(store.commit(thread_id(1), record.version, c).await.unwrap());
    assert_eq!(store.thread_by_share_nonce(&[1; 16]).await.unwrap(), None);
    assert_eq!(
        store.thread_by_share_nonce(&[2; 16]).await.unwrap(),
        Some(record)
    );
}

/// A new link (a new nonce) kills the old one and keeps the thread shared; a change of level that
/// keeps the nonce keeps the link and moves `shared_at`.
pub async fn a_reshared_thread_is_found_by_its_new_nonce_only<S: ThreadStore>(store: S) {
    use orch_core::ShareLevel;
    seed(&store, &alice(), 1).await;
    let c = share_commit(ThreadState::Queued, ShareLevel::Internal, nonce(1), 5);
    let (mut record, _) = applied(store.commit(thread_id(1), 1, c).await.unwrap());

    // widened, same nonce
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(1), 6);
    record = applied(store.commit(thread_id(1), record.version, c).await.unwrap()).0;
    let found = store
        .thread_by_share_nonce(&[1; 16])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.share.unwrap().level, ShareLevel::Public);
    assert_eq!(found.share.unwrap().shared_at, at(6));

    // a new link: the old nonce is dead
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(2), 7);
    record = applied(store.commit(thread_id(1), record.version, c).await.unwrap()).0;
    assert_eq!(store.thread_by_share_nonce(&[1; 16]).await.unwrap(), None);
    let found = store
        .thread_by_share_nonce(&[2; 16])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found, record);
    assert_eq!(found.share.unwrap().nonce, nonce(2));
}

/// A fork of a shared thread is private: the parent's nonce finds the parent and not the fork, the
/// fork has no share, and a share that the fork's own first commit carries is not written (a new
/// thread is private). The fork's log has the copied `thread_shared` event as history.
pub async fn a_fork_of_a_shared_thread_is_private<S: ThreadStore>(store: S) {
    use orch_core::{EventKind, ForkKind, ShareLevel, Visibility};
    seed_conversation(&store, &alice(), 1, 0).await;
    let c = share_commit(ThreadState::Done, ShareLevel::Public, nonce(1), 5);
    let (parent, _) = applied(store.commit(thread_id(1), 1, c).await.unwrap());
    assert_eq!(parent.last_seq, 5);

    // the fork copies the whole log, the `thread_shared` event included
    let (fork_record, _) = {
        let mut new = new_thread(&alice(), 2);
        new.now = at(10);
        let mut first = commit(
            ThreadState::Done,
            vec![forked_event(1, 5, ForkKind::Fork)],
            vec![],
        );
        first.now = at(10);
        // a store must ignore this: a new thread is private whatever its first commit says
        first.sharing = Some(SharingChange::Set {
            level: ShareLevel::Public,
            nonce: nonce(9),
        });
        store
            .fork_thread(
                new,
                crate::ForkOrigin {
                    parent: thread_id(1),
                    cut: 5,
                    kind: ForkKind::Fork,
                },
                first,
            )
            .await
            .unwrap()
    };
    assert_eq!(fork_record.share, None);
    assert_eq!(fork_record.visibility(), Visibility::Private);
    let log = store.list_events(thread_id(2), 0, 100).await.unwrap();
    assert_eq!(log[4].kind(), EventKind::ThreadShared, "history, copied");

    let found = store
        .thread_by_share_nonce(&[1; 16])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, thread_id(1));
    assert_eq!(store.thread_by_share_nonce(&[9; 16]).await.unwrap(), None);
    let fork_read = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(fork_read.share, None);
    // the parent is still shared
    let parent_read = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    assert_eq!(parent_read.share, parent.share);
}

/// A thread created with a share in its first commit is private too.
pub async fn a_new_thread_is_private<S: ThreadStore>(store: S) {
    use orch_core::ShareLevel;
    let mut first = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(3), 0);
    first.now = t0();
    let (record, _) = store
        .create_thread(new_thread(&alice(), 1), first)
        .await
        .unwrap();
    assert_eq!(record.share, None);
    assert_eq!(store.thread_by_share_nonce(&[3; 16]).await.unwrap(), None);
}

/// A nonce is one thread's: a second thread cannot take it, and nothing of the refused commit is
/// written (not the event, not the version).
pub async fn a_nonce_belongs_to_one_thread<S: ThreadStore>(store: S) {
    use orch_core::ShareLevel;
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(1), 5);
    applied(store.commit(thread_id(1), 1, c).await.unwrap());

    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(1), 6);
    let res = store.commit(thread_id(2), 1, c).await;
    assert!(res.is_err(), "{res:?}");
    let two = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!((two.version, two.last_seq, two.share), (1, 1, None));
    let found = store
        .thread_by_share_nonce(&[1; 16])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, thread_id(1));
}

/// A refused commit writes no share: a stale version, a replayed idempotency key.
pub async fn a_refused_commit_writes_no_share<S: ThreadStore>(store: S) {
    use orch_core::ShareLevel;
    seed(&store, &alice(), 1).await;
    let c = share_commit(ThreadState::Queued, ShareLevel::Public, nonce(1), 5);
    assert_eq!(
        class_of(&store.commit(thread_id(1), 99, c).await),
        Some(ErrorClass::Conflict)
    );
    assert_eq!(store.thread_by_share_nonce(&[1; 16]).await.unwrap(), None);

    // a replayed key: the first write has it, the second is a duplicate and shares nothing
    let mut first = commit(
        ThreadState::Queued,
        vec![user_event("again", Some("k"))],
        vec![],
    );
    first.sharing = Some(SharingChange::Set {
        level: ShareLevel::Internal,
        nonce: nonce(2),
    });
    applied(store.commit(thread_id(1), 1, first).await.unwrap());
    let mut again = commit(
        ThreadState::Queued,
        vec![user_event("again", Some("k"))],
        vec![],
    );
    again.sharing = Some(SharingChange::Set {
        level: ShareLevel::Public,
        nonce: nonce(3),
    });
    assert!(matches!(
        store.commit(thread_id(1), 2, again).await.unwrap(),
        CommitOutcome::Duplicate
    ));
    assert_eq!(store.thread_by_share_nonce(&[3; 16]).await.unwrap(), None);
    assert!(
        store
            .thread_by_share_nonce(&[2; 16])
            .await
            .unwrap()
            .is_some()
    );
}

// ---- the owner's list: pin, archive, order and nesting (ADR 0042) ----------------------------

/// The number a thread of the cases was made with.
fn number(id: ThreadId) -> u128 {
    id.0.as_u128() - 0x0190_0000_0000_7000_8000_0000_0000_0000
}

/// What a listing says, as the numbers of the threads, in order.
async fn listed<S: ThreadStore>(
    store: &S,
    owner: &UserId,
    listing: crate::ThreadListing,
) -> Vec<u128> {
    store
        .list_threads(owner, listing)
        .await
        .unwrap()
        .into_iter()
        .map(|t| number(t.id))
        .collect()
}

/// The owner's list in their own order, fifty at most, archived left out.
fn rail() -> crate::ThreadListing {
    crate::ThreadListing::recent(None, 50, false).in_rail_order()
}

fn rail_of(archived: crate::ArchivedFilter) -> crate::ThreadListing {
    rail().archived(archived)
}

async fn alice_rail<S: ThreadStore>(store: &S) -> Vec<u128> {
    listed(store, &alice(), rail()).await
}

async fn arrange<S: ThreadStore>(
    store: &S,
    n: u128,
    change: crate::Arrangement,
    secs: i64,
) -> Result<orch_core::ThreadRecord, StoreError> {
    store
        .arrange_thread(&alice(), thread_id(n), change, at(secs))
        .await
}

fn put(place: crate::Place) -> crate::Arrangement {
    crate::Arrangement {
        place: Some(place),
        ..crate::Arrangement::default()
    }
}

fn pin(on: bool) -> crate::Arrangement {
    crate::Arrangement {
        pinned: Some(on),
        ..crate::Arrangement::default()
    }
}

fn archive(on: bool) -> crate::Arrangement {
    crate::Arrangement {
        archived: Some(on),
        ..crate::Arrangement::default()
    }
}

fn eject() -> crate::Arrangement {
    crate::Arrangement {
        unnest: true,
        ..crate::Arrangement::default()
    }
}

fn before(n: u128) -> crate::Place {
    crate::Place::Before(thread_id(n))
}

fn after(n: u128) -> crate::Place {
    crate::Place::After(thread_id(n))
}

/// A thread of `owner` nested under thread `parent`.
async fn seed_nested<S: ThreadStore>(store: &S, n: u128, parent: u128) {
    let mut new = new_thread(&alice(), n);
    new.rail_parent = Some(thread_id(parent));
    store
        .create_thread(
            new,
            commit(ThreadState::Done, vec![user_event("hi", None)], vec![]),
        )
        .await
        .unwrap();
}

/// The code a refusal carries.
fn refusal<T: std::fmt::Debug>(res: Result<T, StoreError>) -> &'static str {
    match res {
        Err(StoreError::Refused(code)) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A new thread goes on top of its owner's list, whatever the owner has done to the list; another
/// owner's threads are another list.
pub async fn a_new_thread_is_on_top_of_the_rail<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed(&store, &bob(), 9).await;
    assert_eq!(alice_rail(&store).await, vec![3, 2, 1]);
    assert_eq!(listed(&store, &bob(), rail()).await, vec![9]);

    arrange(&store, 1, put(crate::Place::Top), 10)
        .await
        .unwrap();
    assert_eq!(alice_rail(&store).await, vec![1, 3, 2]);
    seed(&store, &alice(), 4).await;
    assert_eq!(alice_rail(&store).await, vec![4, 1, 3, 2]);

    // the default order is the one the list always had, whatever was moved
    assert_eq!(
        listed(
            &store,
            &alice(),
            crate::ThreadListing::recent(None, 50, false)
        )
        .await,
        vec![4, 3, 2, 1]
    );
}

/// A thread is put on top, before another or after another; the others keep their order.
pub async fn a_thread_is_placed_on_top_before_or_after_another<S: ThreadStore>(store: S) {
    for n in 1..=5 {
        seed(&store, &alice(), n).await;
    }
    assert_eq!(alice_rail(&store).await, vec![5, 4, 3, 2, 1]);
    arrange(&store, 1, put(crate::Place::Top), 10)
        .await
        .unwrap();
    assert_eq!(alice_rail(&store).await, vec![1, 5, 4, 3, 2]);
    arrange(&store, 5, put(after(3)), 11).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![1, 4, 3, 5, 2]);
    arrange(&store, 2, put(before(1)), 12).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 1, 4, 3, 5]);
    arrange(&store, 3, put(before(2)), 13).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![3, 2, 1, 4, 5]);
    arrange(&store, 3, put(after(5)), 14).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 1, 4, 5, 3]);
    // moving never touches what the conversation is
    let t = store.get_thread(None, thread_id(3)).await.unwrap().unwrap();
    assert_eq!((t.version, t.last_seq, t.updated_at), (1, 1, t0()));
    assert_eq!(
        store.list_events(thread_id(3), 0, 10).await.unwrap().len(),
        1
    );
}

/// A block is a thread and the threads nested under it: it moves as one, its children stay under it
/// newest first, and the limit of a page counts the top-level threads.
pub async fn a_block_moves_with_its_parent<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 4, 1).await;
    seed_nested(&store, 5, 1).await;
    assert_eq!(alice_rail(&store).await, vec![3, 2, 1, 5, 4]);
    let nested = store.get_thread(None, thread_id(5)).await.unwrap().unwrap();
    assert_eq!(nested.rail_parent, Some(thread_id(1)));

    arrange(&store, 1, put(crate::Place::Top), 10)
        .await
        .unwrap();
    assert_eq!(alice_rail(&store).await, vec![1, 5, 4, 3, 2]);
    arrange(&store, 2, put(before(1)), 11).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 1, 5, 4, 3]);
    let two = crate::ThreadListing::recent(None, 2, false).in_rail_order();
    assert_eq!(listed(&store, &alice(), two).await, vec![2, 1, 5, 4]);
    // the order of the rows by creation does not know of blocks
    assert_eq!(
        listed(
            &store,
            &alice(),
            crate::ThreadListing::recent(None, 50, false)
        )
        .await,
        vec![5, 4, 3, 2, 1]
    );
}

/// Pin puts a thread on top of the pinned, unpin on top of the rest, and the pinned come first.
pub async fn pin_and_unpin_go_to_the_top_of_their_sections<S: ThreadStore>(store: S) {
    for n in 1..=4 {
        seed(&store, &alice(), n).await;
    }
    let pinned = arrange(&store, 2, pin(true), 10).await.unwrap();
    assert_eq!(pinned.pinned_at, Some(at(10)));
    assert_eq!(alice_rail(&store).await, vec![2, 4, 3, 1]);
    arrange(&store, 1, pin(true), 11).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![1, 2, 4, 3]);
    let unpinned = arrange(&store, 1, pin(false), 12).await.unwrap();
    assert_eq!(unpinned.pinned_at, None);
    assert_eq!(alice_rail(&store).await, vec![2, 1, 4, 3]);
    // a thread made now is on top of the rest, under what is pinned
    seed(&store, &alice(), 5).await;
    assert_eq!(alice_rail(&store).await, vec![2, 5, 1, 4, 3]);
    // a pinned thread is moved among the pinned
    arrange(&store, 3, pin(true), 13).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![3, 2, 5, 1, 4]);
    arrange(&store, 2, put(before(3)), 14).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 3, 5, 1, 4]);
}

/// An archived thread is left out of a listing unless it asks for them, in either order; the
/// archived come last, the newest archived first; a block is archived with its parent, and a child
/// archived alone is listed on its own.
pub async fn archived_threads_are_listed_only_when_asked<S: ThreadStore>(store: S) {
    use crate::ArchivedFilter::{Exclude, Include, Only};
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    let archived = arrange(&store, 2, archive(true), 100).await.unwrap();
    assert_eq!(archived.archived_at, Some(at(100)));
    arrange(&store, 1, archive(true), 200).await.unwrap();
    let recent = |archived| crate::ThreadListing::recent(None, 50, false).archived(archived);
    assert_eq!(listed(&store, &alice(), recent(Exclude)).await, vec![3]);
    assert_eq!(listed(&store, &alice(), recent(Only)).await, vec![2, 1]);
    assert_eq!(
        listed(&store, &alice(), recent(Include)).await,
        vec![3, 2, 1]
    );
    assert_eq!(listed(&store, &alice(), rail_of(Exclude)).await, vec![3]);
    // the newest archived first, whatever their ids say
    assert_eq!(listed(&store, &alice(), rail_of(Only)).await, vec![1, 2]);
    assert_eq!(
        listed(&store, &alice(), rail_of(Include)).await,
        vec![3, 1, 2]
    );
    // a page of the archived goes on from where the last one ended
    let page = crate::ThreadListing {
        before: Some(thread_id(1)),
        ..rail_of(Only)
    };
    assert_eq!(listed(&store, &alice(), page).await, vec![2]);
    arrange(&store, 1, archive(false), 300).await.unwrap();
    arrange(&store, 2, archive(false), 300).await.unwrap();

    // blocks: 4 and 5 are 1's children
    seed_nested(&store, 4, 1).await;
    seed_nested(&store, 5, 1).await;
    arrange(&store, 4, archive(true), 400).await.unwrap();
    // a child archived alone: the block shows without it, the archived lists it on its own
    assert_eq!(
        listed(&store, &alice(), rail_of(Exclude)).await,
        vec![3, 2, 1, 5]
    );
    assert_eq!(listed(&store, &alice(), rail_of(Only)).await, vec![4]);
    assert_eq!(
        listed(&store, &alice(), rail_of(Include)).await,
        vec![3, 2, 1, 5, 4]
    );
    // the whole block archived: nothing of it is in the list, all of it is in the archived
    arrange(&store, 1, archive(true), 500).await.unwrap();
    assert_eq!(listed(&store, &alice(), rail_of(Exclude)).await, vec![3, 2]);
    assert_eq!(listed(&store, &alice(), rail_of(Only)).await, vec![1, 5, 4]);
    assert_eq!(
        listed(&store, &alice(), rail_of(Include)).await,
        vec![3, 2, 1, 5, 4]
    );
    // a cursor the filter does not list has no page after it
    let cursor = |n, archived| crate::ThreadListing {
        before: Some(thread_id(n)),
        ..rail_of(archived)
    };
    assert_eq!(
        listed(&store, &alice(), cursor(1, Exclude)).await,
        Vec::<u128>::new()
    );
    assert_eq!(
        listed(&store, &alice(), cursor(5, Only)).await,
        Vec::<u128>::new()
    );
}

/// Unarchiving keeps the place the thread had.
pub async fn unarchiving_keeps_the_place<S: ThreadStore>(store: S) {
    for n in 1..=4 {
        seed(&store, &alice(), n).await;
    }
    arrange(&store, 2, put(crate::Place::Top), 10)
        .await
        .unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 4, 3, 1]);
    arrange(&store, 2, archive(true), 11).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![4, 3, 1]);
    arrange(&store, 2, archive(false), 12).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 4, 3, 1]);
    // a pinned thread stays pinned through it
    arrange(&store, 3, pin(true), 13).await.unwrap();
    arrange(&store, 3, archive(true), 14).await.unwrap();
    let back = arrange(&store, 3, archive(false), 15).await.unwrap();
    assert_eq!(back.pinned_at, Some(at(13)));
    assert_eq!(alice_rail(&store).await, vec![3, 2, 4, 1]);
}

/// Eject takes a thread out of its block and puts it right after it; from a pinned or archived
/// block, on top of the unpinned. The lineage is not touched, and a thread that is not nested has
/// nothing to eject.
pub async fn eject_lands_after_the_former_block<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 4, 2).await;
    seed_nested(&store, 5, 2).await;
    assert_eq!(alice_rail(&store).await, vec![3, 2, 5, 4, 1]);
    let ejected = arrange(&store, 5, eject(), 10).await.unwrap();
    assert_eq!(ejected.rail_parent, None);
    assert_eq!(ejected.forked_from, None);
    assert_eq!(alice_rail(&store).await, vec![3, 2, 4, 5, 1]);
    // it moves like any other now
    arrange(&store, 5, put(crate::Place::Top), 11)
        .await
        .unwrap();
    assert_eq!(alice_rail(&store).await, vec![5, 3, 2, 4, 1]);

    // from a pinned block the thread goes on top of the unpinned
    seed_nested(&store, 6, 2).await;
    seed_nested(&store, 7, 2).await;
    arrange(&store, 2, pin(true), 12).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 7, 6, 4, 5, 3, 1]);
    arrange(&store, 7, eject(), 13).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 6, 4, 7, 5, 3, 1]);
    // ejected and placed at once: the place decides
    arrange(
        &store,
        6,
        crate::Arrangement {
            unnest: true,
            place: Some(after(3)),
            ..crate::Arrangement::default()
        },
        14,
    )
    .await
    .unwrap();
    assert_eq!(alice_rail(&store).await, vec![2, 4, 7, 5, 3, 6, 1]);
    // ejected and pinned at once: on top of the pinned
    arrange(
        &store,
        4,
        crate::Arrangement {
            unnest: true,
            pinned: Some(true),
            ..crate::Arrangement::default()
        },
        15,
    )
    .await
    .unwrap();
    assert_eq!(alice_rail(&store).await, vec![4, 2, 7, 5, 3, 6, 1]);
    // nothing to eject from a thread that is not nested
    let same = arrange(&store, 1, eject(), 16).await.unwrap();
    assert_eq!(
        same.rail_rank,
        store
            .get_thread(None, thread_id(1))
            .await
            .unwrap()
            .unwrap()
            .rail_rank
    );
    assert_eq!(alice_rail(&store).await, vec![4, 2, 7, 5, 3, 6, 1]);
}

/// What cannot be done is refused with a name and writes nothing: an anchor that is gone, archived,
/// nested, the thread itself or another owner's; a nested thread pinned or placed without being
/// ejected.
pub async fn a_bad_anchor_or_a_nested_row_is_refused<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed(&store, &bob(), 8).await;
    seed_nested(&store, 4, 1).await;
    arrange(&store, 3, archive(true), 5).await.unwrap();
    let list = alice_rail(&store).await;
    assert_eq!(list, vec![2, 1, 4]);
    let before_state: Vec<_> = store
        .list_threads(&alice(), rail_of(crate::ArchivedFilter::Include))
        .await
        .unwrap();

    for anchor in [99, 2, 3, 4, 8] {
        for place in [before(anchor), after(anchor)] {
            // 2 is the thread itself; 3 is archived; 4 is nested; 8 is bob's; 99 is nobody's
            let code = refusal(arrange(&store, 2, put(place), 10).await);
            assert_eq!(code, "bad_anchor", "anchor {anchor}");
        }
    }
    // a nested thread is neither pinned nor placed
    assert_eq!(
        refusal(arrange(&store, 4, pin(true), 10).await),
        "nested_row"
    );
    assert_eq!(
        refusal(arrange(&store, 4, put(crate::Place::Top), 10).await),
        "nested_row"
    );
    assert_eq!(
        refusal(arrange(&store, 4, put(before(2)), 10).await),
        "nested_row"
    );
    // but it may be archived, and unpinned (there is nothing to unpin)
    assert!(arrange(&store, 4, pin(false), 10).await.is_ok());
    // a refusal is a class the callers can tell
    let err = arrange(&store, 2, put(before(99)), 10).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Rejected);

    assert_eq!(alice_rail(&store).await, list);
    assert_eq!(
        store
            .list_threads(&alice(), rail_of(crate::ArchivedFilter::Include))
            .await
            .unwrap(),
        before_state
    );
}

/// Only the owner arranges a thread: for anyone else it does not exist, and nothing changes.
pub async fn arranging_is_the_owners_alone<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    seed(&store, &bob(), 3).await;
    let before_rows = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    for change in [pin(true), archive(true), put(crate::Place::Top), eject()] {
        let res = store
            .arrange_thread(&bob(), thread_id(1), change, at(10))
            .await;
        assert_eq!(class_of(&res), Some(ErrorClass::NotFound), "{change:?}");
    }
    let missing = store
        .arrange_thread(&alice(), thread_id(99), pin(true), at(10))
        .await;
    assert_eq!(class_of(&missing), Some(ErrorClass::NotFound));
    assert_eq!(
        store.get_thread(None, thread_id(1)).await.unwrap().unwrap(),
        before_rows
    );
    assert_eq!(alice_rail(&store).await, vec![2, 1]);
}

/// A page in the owner's order ends between blocks and the cursor is the last top-level thread,
/// pinned and archived ones included.
pub async fn rail_pages_never_split_a_block<S: ThreadStore>(store: S) {
    use crate::ArchivedFilter::{Exclude, Include, Only};
    for n in 1..=5 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 6, 4).await;
    seed_nested(&store, 7, 4).await;
    assert_eq!(alice_rail(&store).await, vec![5, 4, 7, 6, 3, 2, 1]);
    let page = |before: Option<u128>, limit, archived| crate::ThreadListing {
        before: before.map(thread_id),
        limit,
        ..rail_of(archived)
    };
    assert_eq!(
        listed(&store, &alice(), page(None, 2, Exclude)).await,
        vec![5, 4, 7, 6]
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(4), 2, Exclude)).await,
        vec![3, 2]
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(2), 2, Exclude)).await,
        vec![1]
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(1), 2, Exclude)).await,
        Vec::<u128>::new()
    );
    // a child is no cursor, nor is a thread that is nobody's
    assert_eq!(
        listed(&store, &alice(), page(Some(6), 2, Exclude)).await,
        Vec::<u128>::new()
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(99), 2, Exclude)).await,
        Vec::<u128>::new()
    );

    // the pinned come first, then the rest, then the archived
    arrange(&store, 2, pin(true), 10).await.unwrap();
    arrange(&store, 3, archive(true), 11).await.unwrap();
    assert_eq!(
        listed(&store, &alice(), page(None, 50, Exclude)).await,
        vec![2, 5, 4, 7, 6, 1]
    );
    assert_eq!(
        listed(&store, &alice(), page(None, 2, Exclude)).await,
        vec![2, 5]
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(5), 2, Exclude)).await,
        vec![4, 7, 6, 1]
    );
    assert_eq!(
        listed(&store, &alice(), page(None, 50, Include)).await,
        vec![2, 5, 4, 7, 6, 1, 3]
    );
    assert_eq!(
        listed(&store, &alice(), page(Some(1), 5, Include)).await,
        vec![3]
    );
    assert_eq!(
        listed(&store, &alice(), page(None, 50, Only)).await,
        vec![3]
    );
    // another owner's cursor has no page after it either
    assert_eq!(
        listed(
            &store,
            &bob(),
            crate::ThreadListing {
                before: Some(thread_id(5)),
                ..rail()
            }
        )
        .await,
        Vec::<u128>::new()
    );
}

/// Asking for what the row already is writes nothing: the same rank, the same times.
pub async fn an_arrangement_that_changes_nothing_writes_nothing<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 4, 1).await;
    let read = |n| {
        let store = &store;
        async move { store.get_thread(None, thread_id(n)).await.unwrap().unwrap() }
    };
    arrange(&store, 2, pin(true), 10).await.unwrap();
    arrange(&store, 1, archive(true), 11).await.unwrap();
    arrange(&store, 1, archive(false), 12).await.unwrap();
    let (one, two, three, four) = (read(1).await, read(2).await, read(3).await, read(4).await);
    assert_eq!(two.pinned_at, Some(at(10)));
    assert_eq!(one.archived_at, None);

    // every one of these is the state the row has
    assert_eq!(arrange(&store, 2, pin(true), 20).await.unwrap(), two);
    assert_eq!(arrange(&store, 3, pin(false), 20).await.unwrap(), three);
    assert_eq!(arrange(&store, 3, archive(false), 20).await.unwrap(), three);
    assert_eq!(arrange(&store, 3, eject(), 20).await.unwrap(), three);
    // (a nested thread is not pinned: that is refused, and writes nothing either)
    assert_eq!(
        refusal(arrange(&store, 4, pin(true), 20).await),
        "nested_row"
    );
    // 2 is pinned and first of the pinned; 3 is first of the rest, and 2, then 3, then 1 is the order
    assert_eq!(alice_rail(&store).await, vec![2, 3, 1, 4]);
    assert_eq!(
        arrange(&store, 2, put(crate::Place::Top), 20)
            .await
            .unwrap(),
        two
    );
    assert_eq!(
        arrange(&store, 3, put(crate::Place::Top), 20)
            .await
            .unwrap(),
        three
    );
    assert_eq!(arrange(&store, 3, put(before(1)), 20).await.unwrap(), three);
    assert_eq!(arrange(&store, 1, put(after(3)), 20).await.unwrap(), one);
    // archiving twice does not stamp it again
    let archived = arrange(&store, 1, archive(true), 30).await.unwrap();
    assert_eq!(archived.archived_at, Some(at(30)));
    assert_eq!(
        arrange(&store, 1, archive(true), 40).await.unwrap(),
        archived
    );
    assert_eq!(read(1).await.archived_at, Some(at(30)));
    assert_eq!(read(4).await, four);
    // nothing was ever a commit
    for n in 1..=4 {
        let t = read(n).await;
        assert_eq!((t.version, t.updated_at), (1, t0()), "thread {n}");
    }
}

/// When no key fits between two neighbours, the owner's ranks are written again, spread, in the
/// same step, and the order is the one asked for all along: here, a thread put after the same
/// anchor over and over, which leaves less room each time.
pub async fn a_rank_that_would_pass_the_cap_re_spreads_the_list<S: ThreadStore>(store: S) {
    for n in 1..=4 {
        seed(&store, &alice(), n).await;
    }
    assert_eq!(alice_rail(&store).await, vec![4, 3, 2, 1]);
    let mut lengths = Vec::new();
    for step in 1..=700_u32 {
        let mover = if step % 2 == 1 { 4 } else { 1 };
        let moved = arrange(&store, mover, put(after(3)), i64::from(step) + 10)
            .await
            .unwrap();
        assert!(
            moved.rail_rank.len() <= orch_core::MAX_RANK_LEN,
            "{}",
            moved.rail_rank.len()
        );
        assert!(
            orch_core::is_valid_rank(&moved.rail_rank),
            "{}",
            moved.rail_rank
        );
        lengths.push(moved.rail_rank.len());
        if step >= 2 && step % 100 == 0 {
            let other = if mover == 4 { 1 } else { 4 };
            assert_eq!(
                alice_rail(&store).await,
                vec![3, mover, other, 2],
                "step {step}"
            );
        }
    }
    // the ranks did run out and were spread again (the key got shorter)
    assert!(
        lengths.windows(2).any(|w| w[1] < w[0]),
        "no re-spread in 700 moves: {lengths:?}"
    );
    // every rank of the owner's is a valid key
    for t in store
        .list_threads(&alice(), rail_of(crate::ArchivedFilter::Include))
        .await
        .unwrap()
    {
        assert!(orch_core::is_valid_rank(&t.rail_rank), "{}", t.rail_rank);
    }
}

/// Threads of one rank are in the order of their ids, newest first: a thread made by an edit takes
/// the rank of the first, so it ties with it.
pub async fn ties_of_rank_are_broken_by_newest_first<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    fork(
        &store,
        1,
        2,
        2,
        ForkKind::Edit,
        10,
        vec![user_event("e", None)],
        vec![delegate(2)],
    )
    .await
    .unwrap();
    let one = store.get_thread(None, thread_id(1)).await.unwrap().unwrap();
    let two = store.get_thread(None, thread_id(2)).await.unwrap().unwrap();
    assert_eq!(one.rail_rank, two.rail_rank);
    assert_eq!(two.rail_parent, None);
    let with_edits = crate::ThreadListing {
        include_edits: true,
        ..rail()
    };
    assert_eq!(listed(&store, &alice(), with_edits).await, vec![2, 1]);
    assert_eq!(alice_rail(&store).await, vec![1]);
    seed(&store, &alice(), 3).await;
    assert_eq!(listed(&store, &alice(), with_edits).await, vec![3, 2, 1]);
    assert_eq!(alice_rail(&store).await, vec![3, 1]);
}

/// A thread is nested under a top-level thread of its own owner, or it is not made.
pub async fn a_thread_is_nested_under_a_top_level_thread_of_its_owner<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &bob(), 2).await;
    seed_nested(&store, 3, 1).await;
    for (n, parent) in [(4_u128, 99_u128), (5, 2), (6, 3)] {
        let mut new = new_thread(&alice(), n);
        new.rail_parent = Some(thread_id(parent));
        let res = store
            .create_thread(new, commit(ThreadState::Done, vec![], vec![]))
            .await;
        assert_eq!(
            class_of(&res),
            Some(ErrorClass::Corrupt),
            "{n} under {parent}"
        );
        assert!(
            store
                .get_thread(None, thread_id(n))
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(alice_rail(&store).await, vec![1, 3]);
}

/// A fork may be made nested under its parent: its row says so, and the list shows it in the block.
pub async fn a_fork_can_be_made_nested_under_its_parent<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    seed(&store, &alice(), 2).await;
    let mut new = new_thread(&alice(), 3);
    new.rail_parent = Some(thread_id(1));
    new.now = at(10);
    let mut first = commit(
        ThreadState::Done,
        vec![forked_event(1, 4, ForkKind::Fork)],
        vec![],
    );
    first.now = at(10);
    let (fork, _) = store
        .fork_thread(
            new,
            crate::ForkOrigin {
                parent: thread_id(1),
                cut: 4,
                kind: ForkKind::Fork,
            },
            first,
        )
        .await
        .unwrap();
    assert_eq!(fork.rail_parent, Some(thread_id(1)));
    assert_eq!(
        fork.forked_from.map(|f| f.thread_id),
        Some(Some(thread_id(1)))
    );
    assert_eq!(alice_rail(&store).await, vec![2, 1, 3]);
    // and it stays whole when read back
    let read = store.get_thread(None, thread_id(3)).await.unwrap().unwrap();
    assert_eq!(read.rail_parent, Some(thread_id(1)));
}

// ---- deleting a thread erases it (ADR 0043) ---------------------------------------------------

/// The version a thread of the cases has now: what the caller read, and delete at.
async fn version_of<S: ThreadStore>(store: &S, n: u128) -> i64 {
    store
        .get_thread(None, thread_id(n))
        .await
        .unwrap()
        .unwrap()
        .version
}

/// Deletes alice's threads `numbers` at the versions they have now.
async fn delete_now<S: ThreadStore>(
    store: &S,
    numbers: &[u128],
    secs: i64,
) -> Result<(), StoreError> {
    let mut threads = Vec::new();
    for n in numbers {
        threads.push((thread_id(*n), version_of(store, *n).await));
    }
    store.delete_threads(&alice(), &threads, at(secs)).await
}

async fn exists<S: ThreadStore>(store: &S, n: u128) -> bool {
    store
        .get_thread(None, thread_id(n))
        .await
        .unwrap()
        .is_some()
}

/// A thread with all that hangs on it: events, a delegation, a binding, a watch, a timer and a share.
async fn seed_loaded<S: ThreadStore>(store: &S, n: u128, timer: u128, watch_n: u8) {
    let mut first = commit(
        ThreadState::Queued,
        vec![user_event("hi", None)],
        vec![delegate(n)],
    );
    first.watches = vec![watch(watch_n)];
    first.timers = vec![ci_deadline(timer, 30, 1, 1)];
    first.binding = Some(BindingUpdate {
        task_id: Some(format!("task-{n}")),
        ..BindingUpdate::default()
    });
    store
        .create_thread(new_thread(&alice(), n), first)
        .await
        .unwrap();
    let c = share_commit(
        ThreadState::Queued,
        orch_core::ShareLevel::Internal,
        nonce(u8::try_from(n).unwrap()),
        5,
    );
    applied(store.commit(thread_id(n), 1, c).await.unwrap());
}

/// A delete takes the thread and everything that hangs on it, in one step: the events, the outbox
/// rows, the binding, the watches, the share (so its link finds nothing) and the timer rows of the
/// inbox that name it. A purge row is written for it. Another thread's rows are untouched.
pub async fn delete_removes_the_thread_and_everything_that_hangs_on_it<S: ThreadStore>(store: S) {
    seed_loaded(&store, 1, 10, 1).await;
    seed_loaded(&store, 2, 20, 2).await;
    // a CI report that waits for a watch that is not the doomed thread's, and one that is
    store.receive(ci_row(30, "d-other", 2), t0()).await.unwrap();
    assert!(store.get_binding(thread_id(1)).await.unwrap().is_some());
    assert!(store.get_inbox(inbox_id(10)).await.unwrap().is_some());
    assert!(
        store
            .thread_by_share_nonce(&[1; 16])
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(store.purges_pending().await.unwrap(), 0);

    delete_now(&store, &[1], 50).await.unwrap();

    assert!(!exists(&store, 1).await);
    assert!(
        store
            .list_events(thread_id(1), 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.get_binding(thread_id(1)).await.unwrap().is_none());
    assert!(store.get_outbox(outbox_id(1)).await.unwrap().is_none());
    assert!(
        store
            .list_open_outbox(thread_id(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.get_watch(watch(1).as_str()).await.unwrap(), None);
    assert!(
        store.get_inbox(inbox_id(10)).await.unwrap().is_none(),
        "the timer that names the thread is gone"
    );
    assert_eq!(
        store.thread_by_share_nonce(&[1; 16]).await.unwrap(),
        None,
        "the link of a deleted thread finds nothing"
    );
    assert!(
        listed(
            &store,
            &alice(),
            crate::ThreadListing::recent(None, 50, true)
        )
        .await
        .iter()
        .all(|n| *n != 1)
    );
    assert_eq!(store.purges_pending().await.unwrap(), 1);

    // the other thread is whole: its rows, its timer, its watch and its link, and the report
    assert!(exists(&store, 2).await);
    assert_eq!(
        store.list_events(thread_id(2), 0, 100).await.unwrap().len(),
        2
    );
    assert!(store.get_binding(thread_id(2)).await.unwrap().is_some());
    assert!(store.get_outbox(outbox_id(2)).await.unwrap().is_some());
    assert_eq!(
        store.get_watch(watch(2).as_str()).await.unwrap(),
        Some(thread_id(2))
    );
    assert!(store.get_inbox(inbox_id(20)).await.unwrap().is_some());
    assert!(store.get_inbox(inbox_id(30)).await.unwrap().is_some());
    assert!(
        store
            .thread_by_share_nonce(&[2; 16])
            .await
            .unwrap()
            .is_some()
    );
}

/// A timer a worker holds when its thread goes is gone too, whatever its status: a claimed row is
/// deleted under its worker, whose late completion is a lost lease and never an error.
pub async fn delete_removes_the_timers_of_the_thread_whatever_their_status<S: ThreadStore>(
    store: S,
) {
    seed_loaded(&store, 1, 10, 1).await;
    // claimed (inflight) by a worker, past its due time
    let claimed = iclaim(&store, "w", at(60)).await;
    assert_eq!(ids(&claimed), vec![inbox_id(10)]);
    delete_now(&store, &[1], 70).await.unwrap();
    assert!(store.get_inbox(inbox_id(10)).await.unwrap().is_none());
    // the worker's late `complete_inbox` finds the row gone: a lost lease, never an error
    assert!(
        !store
            .complete_inbox(&ilease(10, "w", 1), InboxFinal::Applied, at(71))
            .await
            .unwrap()
    );
}

/// The edits of a thread go with it, transitively (they are branches the person cannot see); a fork
/// is a conversation of its own and stays, standing alone, and the thread it was made from is not
/// its parent any more.
pub async fn delete_takes_the_edits_with_the_thread_and_keeps_the_forks<S: ThreadStore>(store: S) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    fork(&store, 1, 2, 2, ForkKind::Edit, 10, vec![], vec![])
        .await
        .unwrap();
    fork(&store, 2, 3, 2, ForkKind::Edit, 11, vec![], vec![])
        .await
        .unwrap();
    let fork_of_root = fork(&store, 1, 4, 4, ForkKind::Fork, 12, vec![], vec![])
        .await
        .unwrap()
        .0;
    fork(&store, 2, 5, 3, ForkKind::Fork, 13, vec![], vec![])
        .await
        .unwrap();
    assert_eq!(
        fork_of_root.forked_from.map(|f| f.thread_id),
        Some(Some(thread_id(1)))
    );
    seed(&store, &alice(), 6).await;

    delete_now(&store, &[1, 2, 3], 50).await.unwrap();

    for n in [1, 2, 3] {
        assert!(!exists(&store, n).await, "thread {n}");
    }
    assert_eq!(store.purges_pending().await.unwrap(), 3);
    for n in [4, 5, 6] {
        assert!(exists(&store, n).await, "thread {n} is kept");
    }
    for n in [4, 5] {
        let kept = store.get_thread(None, thread_id(n)).await.unwrap().unwrap();
        assert_eq!(
            kept.forked_from.map(|f| (f.thread_id, f.kind)),
            Some((None, ForkKind::Fork)),
            "a fork stands alone"
        );
        // and whole: the log it copied is its own
        assert!(store.list_events(thread_id(n), 0, 100).await.unwrap().len() >= 4);
    }
    // the family of the survivors is just themselves
    assert!(
        store
            .fork_family(&alice(), thread_id(4))
            .await
            .unwrap()
            .len()
            <= 1
    );
}

/// A thread made by an edit from one that goes, which the caller did not name (it was made after
/// the caller looked), is a conflict, and nothing is deleted: the caller reads again.
pub async fn delete_that_misses_an_edit_is_a_conflict_and_deletes_nothing<S: ThreadStore>(
    store: S,
) {
    use orch_core::ForkKind;
    seed_conversation(&store, &alice(), 1, 0).await;
    fork(&store, 1, 2, 2, ForkKind::Edit, 10, vec![], vec![])
        .await
        .unwrap();

    let err = delete_now(&store, &[1], 50).await;
    assert_eq!(class_of(&err), Some(ErrorClass::Conflict), "{err:?}");
    assert!(exists(&store, 1).await && exists(&store, 2).await);
    assert_eq!(store.purges_pending().await.unwrap(), 0);

    // an edit that is named alone is deletable: its parent stays, with nothing of it
    delete_now(&store, &[2], 51).await.unwrap();
    assert!(exists(&store, 1).await && !exists(&store, 2).await);
    delete_now(&store, &[1], 52).await.unwrap();
    assert!(!exists(&store, 1).await);
}

/// The threads nested under one that goes take its place in the owner's list, in their order
/// (newest first), as top-level threads: the rest of the list is as it was.
pub async fn nested_children_take_the_place_of_a_deleted_parent<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 4, 2).await;
    seed_nested(&store, 5, 2).await;
    assert_eq!(alice_rail(&store).await, vec![3, 2, 5, 4, 1]);

    delete_now(&store, &[2], 50).await.unwrap();

    assert_eq!(alice_rail(&store).await, vec![3, 5, 4, 1]);
    for n in [4, 5] {
        let child = store.get_thread(None, thread_id(n)).await.unwrap().unwrap();
        assert_eq!(child.rail_parent, None);
    }
    // they are in the list's own order now: one can be placed beside another, and the list holds
    arrange(&store, 4, put(before(5)), 60).await.unwrap();
    assert_eq!(alice_rail(&store).await, vec![3, 4, 5, 1]);
    // a thread made after goes on top
    seed(&store, &alice(), 6).await;
    assert_eq!(alice_rail(&store).await, vec![6, 3, 4, 5, 1]);
}

/// They take the section of what they were nested under too: the children of a pinned thread are
/// pinned, those of an archived one are archived (and one archived on its own stays so).
pub async fn nested_children_keep_the_section_of_a_deleted_parent<S: ThreadStore>(store: S) {
    use crate::ArchivedFilter::{Exclude, Only};
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    seed_nested(&store, 4, 1).await;
    seed_nested(&store, 5, 2).await;
    seed_nested(&store, 6, 2).await;
    arrange(&store, 1, pin(true), 10).await.unwrap();
    arrange(&store, 2, archive(true), 11).await.unwrap();
    arrange(&store, 5, archive(true), 12).await.unwrap();

    delete_now(&store, &[1, 2], 50).await.unwrap();

    // 4 was pinned with its block: it is pinned now, on top
    assert_eq!(listed(&store, &alice(), rail_of(Exclude)).await, vec![4, 3]);
    let four = store.get_thread(None, thread_id(4)).await.unwrap().unwrap();
    assert!(four.pinned_at.is_some());
    // 5 and 6 were archived with their block (5 on its own as well): archived now, none lost
    let mut archived = listed(&store, &alice(), rail_of(Only)).await;
    archived.sort_unstable();
    assert_eq!(archived, vec![5, 6]);
}

/// Another owner's thread, and a thread that does not exist, are not found, and nothing named with
/// them is deleted; a second delete of a thread is not found.
pub async fn delete_is_the_owners_alone_and_a_second_delete_is_not_found<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    seed(&store, &bob(), 9).await;

    let versions = [(thread_id(1), 1), (thread_id(9), 1)];
    let err = store.delete_threads(&alice(), &versions, at(50)).await;
    assert_eq!(class_of(&err), Some(ErrorClass::NotFound), "{err:?}");
    let err = store
        .delete_threads(&alice(), &[(thread_id(1), 1), (thread_id(77), 1)], at(50))
        .await;
    assert_eq!(class_of(&err), Some(ErrorClass::NotFound), "{err:?}");
    assert!(exists(&store, 1).await && exists(&store, 9).await);
    assert_eq!(store.purges_pending().await.unwrap(), 0);

    store.delete_threads(&alice(), &[], at(50)).await.unwrap();
    delete_now(&store, &[1], 51).await.unwrap();
    let err = store
        .delete_threads(&alice(), &[(thread_id(1), 1)], at(52))
        .await;
    assert_eq!(class_of(&err), Some(ErrorClass::NotFound), "{err:?}");
    assert_eq!(
        store.purges_pending().await.unwrap(),
        1,
        "no second purge row"
    );
    assert!(exists(&store, 2).await && exists(&store, 9).await);
}

/// A thread that changed since it was read is a conflict, and none of the threads named is
/// deleted.
pub async fn a_version_conflict_deletes_nothing<S: ThreadStore>(store: S) {
    seed(&store, &alice(), 1).await;
    seed(&store, &alice(), 2).await;
    // thread 2 moves on after the read
    applied(
        store
            .commit(
                thread_id(2),
                1,
                commit(ThreadState::Working, vec![user_event("more", None)], vec![]),
            )
            .await
            .unwrap(),
    );
    let err = store
        .delete_threads(&alice(), &[(thread_id(1), 1), (thread_id(2), 1)], at(50))
        .await;
    assert_eq!(class_of(&err), Some(ErrorClass::Conflict), "{err:?}");
    assert!(exists(&store, 1).await && exists(&store, 2).await);
    assert_eq!(store.purges_pending().await.unwrap(), 0);
    assert_eq!(
        store.list_events(thread_id(2), 0, 100).await.unwrap().len(),
        2
    );
    // read again, decide again
    delete_now(&store, &[1, 2], 51).await.unwrap();
    assert!(!exists(&store, 1).await && !exists(&store, 2).await);
}

/// Every thread deleted has a purge row, claimed oldest deletion first under a lease: a row nobody
/// holds is claimed once and not again, a lapsed lease is claimed again (the attempts counted), and
/// finishing a row removes it, once or twice.
pub async fn purges_are_claimed_under_a_lease_and_finished<S: ThreadStore>(store: S) {
    for n in 1..=3 {
        seed(&store, &alice(), n).await;
    }
    delete_now(&store, &[2], 50).await.unwrap();
    delete_now(&store, &[1], 60).await.unwrap();
    delete_now(&store, &[3], 70).await.unwrap();
    assert_eq!(store.purges_pending().await.unwrap(), 3);

    // oldest deletion first, as many as asked for
    let first = store.claim_purges("w1", 2, LEASE, at(100)).await.unwrap();
    assert_eq!(first, vec![thread_id(2), thread_id(1)]);
    // a claimed row is not claimed again while its lease lasts
    let second = store.claim_purges("w2", 10, LEASE, at(101)).await.unwrap();
    assert_eq!(second, vec![thread_id(3)]);
    assert!(
        store
            .claim_purges("w3", 10, LEASE, at(102))
            .await
            .unwrap()
            .is_empty()
    );
    // claiming does not finish: the rows are all there
    assert_eq!(store.purges_pending().await.unwrap(), 3);

    // w1 finishes one and dies with the other: the lease lapses and another worker has it
    store.finish_purge(thread_id(2)).await.unwrap();
    assert_eq!(store.purges_pending().await.unwrap(), 2);
    let later = at(130);
    let reclaimed = store.claim_purges("w3", 10, LEASE, later).await.unwrap();
    assert_eq!(
        reclaimed,
        vec![thread_id(1)],
        "only the row whose lease lapsed"
    );

    // finishing is idempotent, and for a thread that never had a row
    store.finish_purge(thread_id(1)).await.unwrap();
    store.finish_purge(thread_id(1)).await.unwrap();
    store.finish_purge(thread_id(99)).await.unwrap();
    store.finish_purge(thread_id(3)).await.unwrap();
    assert_eq!(store.purges_pending().await.unwrap(), 0);
    assert!(
        store
            .claim_purges("w", 10, LEASE, at(500))
            .await
            .unwrap()
            .is_empty()
    );
}

/// Workers that claim at once never get the same purge.
pub async fn purge_claimers_never_share_a_row<S: ThreadStore>(store: S) {
    for n in 1..=12 {
        seed(&store, &alice(), n).await;
    }
    let all: Vec<u128> = (1..=12).collect();
    delete_now(&store, &all, 50).await.unwrap();
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for w in 0..4 {
        let store = Arc::clone(&store);
        tasks.push(tokio::spawn(async move {
            store
                .claim_purges(&format!("w{w}"), 5, LEASE, at(100))
                .await
                .unwrap()
        }));
    }
    let mut claimed = Vec::new();
    for task in tasks {
        claimed.extend(task.await.unwrap());
    }
    let mut unique = claimed.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), claimed.len(), "a row went to two workers");
    assert_eq!(claimed.len(), 12, "every row went to one");
}
