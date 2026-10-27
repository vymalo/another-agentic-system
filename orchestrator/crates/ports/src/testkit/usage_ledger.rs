//! `UsageLedger` conformance cases (ADR 0065). Each is `async fn(store)` on a fresh, isolated store
//! that is also a [`ThreadStore`]: the ledger is written by the commits of the store it is read from.
//! The cases that play a log older than the ledger take a second argument, an `async fn(&S)` that
//! makes the store forget its ledger (it empties it and marks no thread as gone through).

use jiff::{SignedDuration, Timestamp};
use orch_core::{
    Actor, AgentId, AgentTarget, Classify, ErrorClass, EventBody, ForkKind, ModelTokens,
    ModelUsageData, ModelUsageTotalData, SandboxEntry, SandboxEnvironment, SandboxShape,
    SandboxUsage, ThreadId, ThreadState, TokenCounts, UnmeasuredSandbox, UsageMonth,
    UsageMonthTotals, UsageSource, UserId, UserMessageData,
};
use uuid::Uuid;

use crate::{
    Commit, CommitOutcome, ForkOrigin, Lease, NewEvent, NewThreadRecord, OutboxId, ThreadStore,
    UsageLedger,
};

/// A store that is both the log and the ledger.
pub trait LogAndLedger: ThreadStore + UsageLedger {}
impl<S: ThreadStore + UsageLedger> LogAndLedger for S {}

fn t0() -> Timestamp {
    "2026-01-15T10:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0().checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn thread_id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn bob() -> UserId {
    UserId::new("bob@example.com")
}

fn jan() -> UsageMonth {
    UsageMonth::new(2026, 1).unwrap()
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn line(provider: Option<&str>, model: &str, tokens: TokenCounts) -> ModelTokens {
    ModelTokens {
        provider: provider.map(Into::into),
        model: model.into(),
        tokens,
    }
}

fn glm(input: u64, output: u64) -> ModelTokens {
    line(Some("openai"), "glm", counts(input, output))
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
        context_id: None,
        rail_parent: None,
        now: t0(),
    }
}

fn message(owner: &UserId) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(owner),
        body: EventBody::UserMessage(UserMessageData::new("hi")),
        idempotency_key: None,
    }
}

/// One `model_usage` of `agent`'s task, on the model `glm` of `openai`.
fn call_at(when: Timestamp, agent: &str, task: &str, id: &str, tokens: TokenCounts) -> NewEvent {
    NewEvent {
        at: when,
        actor: Actor::agent(&AgentId::new(agent), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: agent.into(),
            task: task.into(),
            call: id.into(),
            path: Vec::new(),
            provider: Some("openai".into()),
            model: "glm".into(),
            tokens,
            context_window: None,
        })),
        idempotency_key: Some(format!("usage:{agent}:{task}:{id}")),
    }
}

fn call(agent: &str, task: &str, id: &str, input: u64, output: u64) -> NewEvent {
    call_at(t0(), agent, task, id, counts(input, output))
}

fn total_at(
    when: Timestamp,
    agent: &str,
    task: &str,
    totals: Vec<ModelTokens>,
    sandbox: Option<SandboxUsage>,
) -> NewEvent {
    NewEvent {
        at: when,
        actor: Actor::agent(&AgentId::new(agent), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: agent.into(),
            task: task.into(),
            path: Vec::new(),
            totals,
            sandbox,
        }),
        idempotency_key: None,
    }
}

fn total(agent: &str, task: &str, totals: Vec<ModelTokens>) -> NewEvent {
    total_at(t0(), agent, task, totals, None)
}

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Working,
        job: None,
        events,
        outbox: Vec::new(),
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

/// Creates thread `n` of `owner` with a user message and returns its version.
async fn open<S: LogAndLedger>(store: &S, owner: &UserId, n: u128) -> i64 {
    open_with(store, owner, n, Vec::new()).await
}

async fn open_with<S: LogAndLedger>(
    store: &S,
    owner: &UserId,
    n: u128,
    events: Vec<NewEvent>,
) -> i64 {
    let mut all = vec![message(owner)];
    all.extend(events);
    store
        .create_thread(new_thread(owner, n), commit(all))
        .await
        .unwrap()
        .0
        .version
}

/// Appends `events` to thread `n` at `version` and returns the new version.
async fn append<S: LogAndLedger>(store: &S, n: u128, version: i64, events: Vec<NewEvent>) -> i64 {
    match store
        .commit(thread_id(n), version, commit(events))
        .await
        .unwrap()
    {
        CommitOutcome::Applied { thread, .. } => thread.version,
        other => panic!("expected the commit to apply, got {other:?}"),
    }
}

async fn month_of<S: LogAndLedger>(
    store: &S,
    owner: &UserId,
    month: UsageMonth,
) -> UsageMonthTotals {
    store.month(owner, month).await.unwrap()
}

// ---- cases ------------------------------------------------------------------------------------

/// Calls add up while the task has no total; the total replaces them and is never added to them; a
/// call after the total changes nothing; a later total replaces the earlier one, even a lower one.
pub async fn calls_add_up_and_a_total_replaces_them<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    let v = append(
        &store,
        1,
        v,
        vec![
            call("coder", "t1", "c1", 10, 2),
            call("coder", "t1", "c2", 20, 3),
        ],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (1, vec![glm(30, 5)]));

    // the total is the record: lower than the calls' sum (a retried attempt the agent left out)
    let v = append(&store, 1, v, vec![total("coder", "t1", vec![glm(25, 4)])]).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await.models,
        vec![glm(25, 4)]
    );

    // a call reported after the total is in the total already
    let v = append(&store, 1, v, vec![call("coder", "t1", "late", 99, 99)]).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await.models,
        vec![glm(25, 4)]
    );

    // a later total replaces, and a lower one does too
    append(&store, 1, v, vec![total("coder", "t1", vec![glm(20, 3)])]).await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (1, vec![glm(20, 3)]));

    let rows = store.thread_rows(&alice(), thread_id(1)).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source, UsageSource::Total);
    assert_eq!(rows[0].month, jan());
    assert_eq!(rows[0].key.agent, "coder");
    assert_eq!(rows[0].key.task, "t1");
    assert_eq!(rows[0].tokens, vec![glm(20, 3)]);
}

/// A thread created with usage events in its first commit is counted: the other way events enter
/// the log.
pub async fn a_thread_created_with_usage_events_is_counted<S: LogAndLedger>(store: S) {
    open_with(&store, &alice(), 1, vec![call("coder", "t1", "c1", 7, 1)]).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await.models,
        vec![glm(7, 1)]
    );
}

/// An agent and a task are the key: an asked agent that numbers its tasks as the thread's agent does
/// is counted apart, and a task an agent continues replaces its own total only.
pub async fn agents_and_tasks_are_counted_apart<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    let v = append(
        &store,
        1,
        v,
        vec![
            total("coder", "t1", vec![glm(100, 10)]),
            total("researcher", "t1", vec![glm(40, 4)]),
            total("coder", "t2", vec![glm(1, 1)]),
        ],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (3, vec![glm(141, 15)]));

    // the researcher continues its task: its total (all its calls so far) replaces its own
    append(
        &store,
        1,
        v,
        vec![total("researcher", "t1", vec![glm(60, 6)])],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (3, vec![glm(161, 17)]));
    let rows = store.thread_rows(&alice(), thread_id(1)).await.unwrap();
    let keys: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.key.agent.as_str(), r.key.task.as_str()))
        .collect();
    assert_eq!(
        keys,
        vec![("coder", "t1"), ("coder", "t2"), ("researcher", "t1")]
    );
}

/// A month sums the tasks of all of a person's threads per provider and model, and the parts of the
/// counts (reasoning, cached) add up and stay unknown until one task says them.
pub async fn a_month_sums_the_threads_and_models_of_a_person<S: LogAndLedger>(store: S) {
    let mut with_parts = counts(1000, 100);
    with_parts.reasoning_tokens = Some(30);
    with_parts.cached_input_tokens = Some(600);
    with_parts.cache_write_input_tokens = Some(50);
    let v1 = open(&store, &alice(), 1).await;
    append(
        &store,
        1,
        v1,
        vec![total(
            "coder",
            "t1",
            vec![
                line(Some("openai"), "glm", with_parts),
                line(None, "small", counts(5, 1)),
            ],
        )],
    )
    .await;
    let v2 = open(&store, &alice(), 2).await;
    append(
        &store,
        2,
        v2,
        vec![total(
            "coder",
            "t9",
            vec![
                line(Some("openai"), "glm", with_parts),
                line(Some("other"), "glm", counts(2, 2)),
            ],
        )],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!(totals.month, jan());
    assert_eq!(totals.tasks, 2);
    // ordered by provider (none first), then model
    let mut parts = counts(2000, 200);
    parts.reasoning_tokens = Some(60);
    parts.cached_input_tokens = Some(1200);
    parts.cache_write_input_tokens = Some(100);
    assert_eq!(
        totals.models,
        vec![
            line(None, "small", counts(5, 1)),
            line(Some("openai"), "glm", parts),
            line(Some("other"), "glm", counts(2, 2)),
        ]
    );
}

/// A row is counted in the UTC month of the latest event that changed it, whole.
pub async fn the_month_is_the_utc_month_of_the_latest_event_that_changed_the_row<
    S: LogAndLedger,
>(
    store: S,
) {
    let end_of_jan: Timestamp = "2026-01-31T23:59:59Z".parse().unwrap();
    let start_of_feb: Timestamp = "2026-02-01T00:00:00Z".parse().unwrap();
    let feb = UsageMonth::new(2026, 2).unwrap();
    let v = open(&store, &alice(), 1).await;
    let v = append(
        &store,
        1,
        v,
        vec![call_at(end_of_jan, "coder", "t1", "c1", counts(10, 2))],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (1, vec![glm(10, 2)]));
    assert_eq!(month_of(&store, &alice(), feb).await.tasks, 0);

    append(
        &store,
        1,
        v,
        vec![call_at(start_of_feb, "coder", "t1", "c2", counts(20, 3))],
    )
    .await;
    assert_eq!(month_of(&store, &alice(), jan()).await.tasks, 0);
    let totals = month_of(&store, &alice(), feb).await;
    assert_eq!((totals.tasks, totals.models), (1, vec![glm(30, 5)]));
    let rows = store.thread_rows(&alice(), thread_id(1)).await.unwrap();
    assert_eq!(rows[0].month, feb);
}

/// One person's usage is never in another's month or breakdown.
pub async fn people_are_kept_apart<S: LogAndLedger>(store: S) {
    let a = open(&store, &alice(), 1).await;
    append(&store, 1, a, vec![total("coder", "t1", vec![glm(100, 10)])]).await;
    let b = open(&store, &bob(), 2).await;
    append(&store, 2, b, vec![total("coder", "t1", vec![glm(7, 1)])]).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await.models,
        vec![glm(100, 10)]
    );
    assert_eq!(
        month_of(&store, &bob(), jan()).await.models,
        vec![glm(7, 1)]
    );
    let nobody = UserId::new("carol@example.com");
    assert_eq!(
        month_of(&store, &nobody, jan()).await,
        UsageMonthTotals::empty(jan())
    );
    // a thread is asked for by its owner
    assert!(
        store
            .thread_rows(&bob(), thread_id(1))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .thread_rows(&alice(), thread_id(1))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .thread_rows(&alice(), thread_id(404))
            .await
            .unwrap()
            .is_empty()
    );
}

/// A commit that is refused writes no usage: a version conflict, a repeated key, a lost claim.
pub async fn a_refused_commit_counts_nothing<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    let v = append(&store, 1, v, vec![call("coder", "t1", "c1", 10, 2)]).await;
    let before = month_of(&store, &alice(), jan()).await;
    assert_eq!(before.models, vec![glm(10, 2)]);

    // a stale version
    let stale = store
        .commit(
            thread_id(1),
            v - 1,
            commit(vec![call("coder", "t1", "c2", 20, 3)]),
        )
        .await;
    assert_eq!(stale.err().map(|e| e.class()), Some(ErrorClass::Conflict));
    assert_eq!(month_of(&store, &alice(), jan()).await, before);

    // the same idempotency key again: the log drops the repeat, and so does the ledger
    let again = store
        .commit(
            thread_id(1),
            v,
            commit(vec![call("coder", "t1", "c1", 10, 2)]),
        )
        .await
        .unwrap();
    assert!(matches!(again, CommitOutcome::Duplicate));
    assert_eq!(month_of(&store, &alice(), jan()).await, before);

    // a claim that is not held
    let mut fenced = commit(vec![call("coder", "t1", "c3", 30, 4)]);
    fenced.lease = Some(Lease {
        id: OutboxId(Uuid::from_u128(0x77)),
        owner: "worker".into(),
        attempt: 1,
    });
    let outcome = store.commit(thread_id(1), v, fenced).await.unwrap();
    assert!(matches!(outcome, CommitOutcome::Fenced));
    assert_eq!(month_of(&store, &alice(), jan()).await, before);

    // and the log has the one event, not three
    let events = store.list_events(thread_id(1), 0, 100).await.unwrap();
    assert_eq!(events.len(), 2);
}

/// A total carries the parts of the counts and the sandbox: per shape for what was measured, apart
/// for what was not, and nothing for a measured zero.
pub async fn a_total_carries_its_parts_and_its_sandbox<S: LogAndLedger>(store: S) {
    let shape = |cpu, memory, commands, milliseconds| SandboxEntry {
        cpu_millis: cpu,
        memory_bytes: memory,
        commands,
        milliseconds,
    };
    let kube = |entries: Vec<SandboxEntry>| SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(entries),
    };
    let local = SandboxUsage {
        environment: SandboxEnvironment::Local,
        usage: None,
    };
    let v = open(&store, &alice(), 1).await;
    append(
        &store,
        1,
        v,
        vec![
            total_at(
                t0(),
                "coder",
                "t1",
                vec![glm(1, 1)],
                Some(kube(vec![
                    shape(Some(250), Some(1024), 2, 1000),
                    shape(None, None, 1, 5),
                ])),
            ),
            total_at(
                t0(),
                "researcher",
                "t2",
                vec![glm(1, 1)],
                Some(kube(vec![shape(Some(250), Some(1024), 3, 2000)])),
            ),
            total_at(t0(), "coder", "t3", vec![glm(1, 1)], Some(local.clone())),
            total_at(t0(), "coder", "t4", vec![glm(1, 1)], Some(local)),
            // a pod was asked for and no command ran: a measured zero, not an unknown
            total_at(t0(), "coder", "t5", vec![glm(1, 1)], Some(kube(Vec::new()))),
            total_at(t0(), "coder", "t6", vec![glm(1, 1)], None),
        ],
    )
    .await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!(totals.tasks, 6);
    assert_eq!(totals.models, vec![glm(6, 6)]);
    assert_eq!(
        totals.sandbox,
        vec![
            SandboxShape {
                environment: SandboxEnvironment::Kubernetes,
                cpu_millis: None,
                memory_bytes: None,
                commands: 1,
                milliseconds: 5,
            },
            SandboxShape {
                environment: SandboxEnvironment::Kubernetes,
                cpu_millis: Some(250),
                memory_bytes: Some(1024),
                commands: 5,
                milliseconds: 3000,
            },
        ]
    );
    assert_eq!(
        totals.unmeasured,
        vec![UnmeasuredSandbox {
            environment: SandboxEnvironment::Local,
            tasks: 2
        }]
    );
    // the rows read back the sandbox as it was reported
    let rows = store.thread_rows(&alice(), thread_id(1)).await.unwrap();
    let t1 = rows.iter().find(|r| r.key.task == "t1").unwrap();
    assert_eq!(
        t1.sandbox,
        Some(kube(vec![
            shape(Some(250), Some(1024), 2, 1000),
            shape(None, None, 1, 5)
        ]))
    );
    let t5 = rows.iter().find(|r| r.key.task == "t5").unwrap();
    assert_eq!(t5.sandbox, Some(kube(Vec::new())));
    let t6 = rows.iter().find(|r| r.key.task == "t6").unwrap();
    assert_eq!(t6.sandbox, None);
}

/// Erasing a thread keeps the person's usage and forgets which thread it came from; the person's
/// other threads are untouched.
pub async fn erasing_a_thread_keeps_its_usage_and_forgets_the_thread<S: LogAndLedger>(store: S) {
    let v1 = open(&store, &alice(), 1).await;
    let v1 = append(
        &store,
        1,
        v1,
        vec![total("coder", "t1", vec![glm(100, 10)])],
    )
    .await;
    let v2 = open(&store, &alice(), 2).await;
    append(&store, 2, v2, vec![call("coder", "t1", "c1", 7, 1)]).await;
    let before = month_of(&store, &alice(), jan()).await;
    assert_eq!(
        (before.tasks, before.models.clone()),
        (2, vec![glm(107, 11)])
    );

    store
        .delete_threads(&alice(), &[(thread_id(1), v1)], at(60))
        .await
        .unwrap();

    assert!(
        store
            .get_thread(None, thread_id(1))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        month_of(&store, &alice(), jan()).await,
        before,
        "the usage is the person's"
    );
    assert!(
        store
            .thread_rows(&alice(), thread_id(1))
            .await
            .unwrap()
            .is_empty(),
        "the rows no longer name the erased thread"
    );
    assert_eq!(
        store
            .thread_rows(&alice(), thread_id(2))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Usage that comes after an erase is counted on top of what was kept, and a task id of the erased
/// thread starts a task of its own in a new one.
pub async fn usage_after_an_erase_is_counted_on_top_of_what_was_kept<S: LogAndLedger>(store: S) {
    let v1 = open(&store, &alice(), 1).await;
    let v1 = append(
        &store,
        1,
        v1,
        vec![total("coder", "t1", vec![glm(100, 10)])],
    )
    .await;
    store
        .delete_threads(&alice(), &[(thread_id(1), v1)], at(60))
        .await
        .unwrap();
    let v2 = open(&store, &alice(), 2).await;
    append(&store, 2, v2, vec![total("coder", "t1", vec![glm(5, 1)])]).await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (2, vec![glm(105, 11)]));
}

/// A fork does not count the events it copied from its parent: their usage was counted there. What
/// the fork logs itself is counted, and erasing the parent leaves both.
pub async fn a_fork_does_not_count_the_events_it_copied<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    let v = append(
        &store,
        1,
        v,
        vec![
            call("coder", "t1", "c1", 10, 2),
            total("coder", "t1", vec![glm(12, 3)]),
        ],
    )
    .await;
    let parent_only = month_of(&store, &alice(), jan()).await;
    assert_eq!(parent_only.models, vec![glm(12, 3)]);

    // the fork copies the whole log (cut 3: message, call, total)
    let (fork, _) = store
        .fork_thread(
            new_thread(&alice(), 2),
            ForkOrigin {
                parent: thread_id(1),
                cut: 3,
                kind: ForkKind::Fork,
            },
            commit(Vec::new()),
        )
        .await
        .unwrap();
    assert_eq!(fork.last_seq, 3);
    assert_eq!(month_of(&store, &alice(), jan()).await, parent_only);
    assert!(
        store
            .thread_rows(&alice(), thread_id(2))
            .await
            .unwrap()
            .is_empty()
    );

    // the fork works: its own usage is its own, even under the task id its parent used
    let v2 = fork.version;
    append(&store, 2, v2, vec![total("coder", "t1", vec![glm(1, 1)])]).await;
    let both = month_of(&store, &alice(), jan()).await;
    assert_eq!((both.tasks, both.models.clone()), (2, vec![glm(13, 4)]));

    // the parent goes: its usage and the fork's stay
    store
        .delete_threads(&alice(), &[(thread_id(1), v)], at(60))
        .await
        .unwrap();
    assert_eq!(month_of(&store, &alice(), jan()).await, both);
    assert_eq!(
        store
            .thread_rows(&alice(), thread_id(2))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Usage the ledger has not been through (a build without it wrote the log, or the log is older
/// than the ledger) is folded in by the next commit of the thread, before the commit's own events:
/// the ledger is complete whichever builds wrote and in whatever order.
pub async fn a_commit_catches_up_with_the_log_the_ledger_has_not_been_through<S: LogAndLedger>(
    store: S,
    forget: impl AsyncFnOnce(&S),
) {
    let v = open(&store, &alice(), 1).await;
    let v = append(
        &store,
        1,
        v,
        vec![
            call("coder", "t1", "c1", 10, 2),
            call("coder", "t1", "c2", 20, 3),
        ],
    )
    .await;
    let v = append(
        &store,
        1,
        v,
        vec![total("coder", "t0", vec![glm(1000, 100)])],
    )
    .await;
    let untouched = open(&store, &alice(), 2).await;
    append(
        &store,
        2,
        untouched,
        vec![total("coder", "t1", vec![glm(7, 1)])],
    )
    .await;
    forget(&store).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await,
        UsageMonthTotals::empty(jan()),
        "the ledger forgot"
    );

    // the next commit of thread 1 brings the log it had in, and then its own event
    append(&store, 1, v, vec![call("coder", "t1", "c3", 30, 4)]).await;
    let totals = month_of(&store, &alice(), jan()).await;
    assert_eq!((totals.tasks, totals.models), (2, vec![glm(1060, 109)]));
    // thread 2 is as the ledger left it, until the backfill goes through it
    assert!(
        store
            .thread_rows(&alice(), thread_id(2))
            .await
            .unwrap()
            .is_empty()
    );
    let rows = store.thread_rows(&alice(), thread_id(1)).await.unwrap();
    let t1 = rows.iter().find(|r| r.key.task == "t1").unwrap();
    assert_eq!(
        (t1.source, t1.tokens.clone()),
        (UsageSource::Calls, vec![glm(60, 9)])
    );
}

/// The backfill folds in the usage the ledger has not been through, thread by thread in pages, and
/// running it again changes nothing. Nothing a fork copied is counted twice.
pub async fn backfill_folds_in_what_the_ledger_has_not_been_through_and_again_changes_nothing<
    S: LogAndLedger,
>(
    store: S,
    forget: impl AsyncFnOnce(&S),
) {
    let v1 = open(&store, &alice(), 1).await;
    append(
        &store,
        1,
        v1,
        vec![
            call("coder", "t1", "c1", 10, 2),
            total("coder", "t1", vec![glm(12, 3)]),
        ],
    )
    .await;
    let (fork, _) = store
        .fork_thread(
            new_thread(&alice(), 2),
            ForkOrigin {
                parent: thread_id(1),
                cut: 3,
                kind: ForkKind::Fork,
            },
            commit(Vec::new()),
        )
        .await
        .unwrap();
    append(
        &store,
        2,
        fork.version,
        vec![call("coder", "t5", "c1", 5, 5)],
    )
    .await;
    let v3 = open(&store, &alice(), 3).await;
    append(
        &store,
        3,
        v3,
        vec![total(
            "coder",
            "t2",
            vec![line(None, "small", counts(3, 3))],
        )],
    )
    .await;
    let v4 = open(&store, &bob(), 4).await;
    append(
        &store,
        4,
        v4,
        vec![total("coder", "t1", vec![glm(100, 100)])],
    )
    .await;

    let alices = month_of(&store, &alice(), jan()).await;
    let bobs = month_of(&store, &bob(), jan()).await;
    assert_eq!(alices.tasks, 3);
    assert_eq!(bobs.tasks, 1);
    forget(&store).await;
    assert_eq!(month_of(&store, &alice(), jan()).await.tasks, 0);

    // pages of one thread
    let mut pages = 0;
    let (mut threads, mut events) = (0, 0);
    let mut after = None;
    loop {
        let page = store.backfill(after, 1).await.unwrap();
        pages += 1;
        threads += page.threads;
        events += page.events;
        assert!(pages <= 10, "the pages end");
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert!(pages >= 4, "one thread to a page: {pages} pages");
    assert_eq!(threads, 4, "the four threads were gone through");
    // the usage events of the logs: two of the parent, none of the fork's copies and one of its
    // own, one of the third thread and one of bob's
    assert_eq!(events, 5);
    assert_eq!(month_of(&store, &alice(), jan()).await, alices);
    assert_eq!(month_of(&store, &bob(), jan()).await, bobs);

    // again: nothing is left to go through
    let again = store.backfill(None, 100).await.unwrap();
    assert_eq!((again.threads, again.events, again.next), (0, 0, None));
    assert_eq!(month_of(&store, &alice(), jan()).await, alices);
    assert_eq!(month_of(&store, &bob(), jan()).await, bobs);
}

/// A thread the ledger is through is not gone through again, and a page asks for at least one
/// thread.
pub async fn backfill_leaves_alone_what_the_ledger_is_through<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    append(&store, 1, v, vec![total("coder", "t1", vec![glm(1, 1)])]).await;
    let page = store.backfill(None, 0).await.unwrap();
    assert_eq!((page.threads, page.events, page.next), (0, 0, None));
    assert_eq!(
        month_of(&store, &alice(), jan()).await.models,
        vec![glm(1, 1)]
    );
}

/// A user message, a state change or any other event changes nothing in the ledger.
pub async fn events_that_are_not_usage_change_nothing<S: LogAndLedger>(store: S) {
    let v = open(&store, &alice(), 1).await;
    append(&store, 1, v, vec![message(&alice())]).await;
    assert_eq!(
        month_of(&store, &alice(), jan()).await,
        UsageMonthTotals::empty(jan())
    );
    assert!(
        store
            .thread_rows(&alice(), thread_id(1))
            .await
            .unwrap()
            .is_empty()
    );
}
