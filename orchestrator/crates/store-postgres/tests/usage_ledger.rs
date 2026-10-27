//! The usage ledger over Postgres (ADR 0065, migration `0023`): what the port's conformance cases
//! cannot say, because it is about the tables. Skipped unless `ORCH_TEST_DATABASE_URL` is set; each
//! test runs in its own schema.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jiff::Timestamp;
use orch_core::{
    Actor, AgentId, AgentTarget, Classify, ErrorClass, EventBody, MAX_LEDGER_COUNT,
    MAX_TOKEN_COUNT, ModelTokens, ModelUsageData, ModelUsageTotalData, SandboxEntry,
    SandboxEnvironment, SandboxUsage, ThreadId, ThreadState, TokenCounts, UsageMonth,
    UsageMonthTotals, UserId, UserMessageData,
};
use orch_ports::{
    Commit, CommitOutcome, NewEvent, NewThreadRecord, ThreadStore, UsageBackfill, UsageLedger,
};
use orch_store_postgres::PgStore;
use support::TestDb;
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use uuid::Uuid;

macro_rules! db_or_skip {
    () => {
        match TestDb::new().await {
            Some(db) => db,
            None => {
                eprintln!("skipped: ORCH_TEST_DATABASE_URL unset");
                return;
            }
        }
    };
}

fn t0() -> Timestamp {
    "2026-01-15T10:00:00Z".parse().unwrap()
}

fn jan() -> UsageMonth {
    UsageMonth::new(2026, 1).unwrap()
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn glm(input: u64, output: u64) -> ModelTokens {
    ModelTokens {
        provider: Some("openai".into()),
        model: "glm".into(),
        tokens: counts(input, output),
    }
}

fn message() -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(&alice()),
        body: EventBody::UserMessage(UserMessageData::new("hi")),
        idempotency_key: None,
    }
}

fn call(task: &str, id: &str, tokens: TokenCounts) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            call: id.into(),
            path: Vec::new(),
            provider: Some("openai".into()),
            model: "glm".into(),
            tokens,
            context_window: None,
        })),
        idempotency_key: None,
    }
}

fn total(task: &str, totals: Vec<ModelTokens>, sandbox: Option<SandboxUsage>) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            path: Vec::new(),
            totals,
            sandbox,
        }),
        idempotency_key: None,
    }
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

async fn open(store: &PgStore, owner: &UserId, events: Vec<NewEvent>) -> (ThreadId, i64) {
    let id = ThreadId(Uuid::now_v7());
    let mut all = vec![message()];
    all.extend(events);
    let (record, _) = store
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
                now: t0(),
            },
            commit(all),
        )
        .await
        .unwrap();
    (id, record.version)
}

async fn append(store: &PgStore, thread: ThreadId, version: i64, events: Vec<NewEvent>) -> i64 {
    match store.commit(thread, version, commit(events)).await.unwrap() {
        CommitOutcome::Applied { thread, .. } => thread.version,
        other => panic!("expected the commit to apply, got {other:?}"),
    }
}

async fn scalar(store: &PgStore, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

async fn through(store: &PgStore, thread: ThreadId) -> (i64, i64) {
    sqlx::query_as("SELECT usage_through, last_seq FROM threads WHERE id = $1")
        .bind(thread.0)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

/// The log as a build without the ledger left it: no rows, no thread gone through.
async fn forget(store: &PgStore) {
    sqlx::query("DELETE FROM usage_tasks")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET usage_through = 0")
        .execute(store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_migration_makes_the_ledger_and_a_thread_starts_with_nothing_gone_through() {
    let db = db_or_skip!();
    let store = db.store().await;
    for table in ["usage_tasks", "usage_lines", "usage_sandbox"] {
        let found: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_name = $1",
        )
        .bind(table)
        .fetch_one(store.pool())
        .await
        .unwrap();
        assert_eq!(found, 1, "{table}");
    }
    // no foreign key to threads: a row outlives its thread
    let keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_constraint WHERE conrelid = 'usage_tasks'::regclass AND contype = 'f'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(keys, 0, "usage_tasks references nothing");
}

#[tokio::test]
async fn a_thread_is_gone_through_up_to_its_last_event_after_every_commit_and_a_fork_from_its_cut()
{
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), vec![call("t1", "c1", counts(1, 1))]).await;
    assert_eq!(through(&store, a).await, (2, 2));
    let v = append(&store, a, v, vec![message(), message()]).await;
    assert_eq!(
        through(&store, a).await,
        (4, 4),
        "a commit with no usage moves it too"
    );
    let _ = append(&store, a, v, vec![total("t1", vec![glm(2, 2)], None)]).await;
    assert_eq!(through(&store, a).await, (5, 5));

    let b = ThreadId(Uuid::now_v7());
    let (fork, _) = store
        .fork_thread(
            NewThreadRecord {
                id: b,
                owner: alice(),
                title: "fork".into(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                now: t0(),
            },
            orch_ports::ForkOrigin {
                parent: a,
                cut: 5,
                kind: orch_core::ForkKind::Fork,
            },
            commit(vec![message()]),
        )
        .await
        .unwrap();
    assert_eq!(fork.last_seq, 6);
    assert_eq!(through(&store, b).await, (6, 6));
    // the copies were counted in the parent
    assert_eq!(store.month(&alice(), jan()).await.unwrap().tasks, 1);
}

#[tokio::test]
async fn an_erase_leaves_the_rows_with_no_thread_and_the_person_keeps_the_usage() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), vec![total("t1", vec![glm(10, 1)], None)]).await;
    assert_eq!(
        scalar(
            &store,
            "SELECT count(*) FROM usage_tasks WHERE thread_id IS NOT NULL"
        )
        .await,
        1
    );
    store
        .delete_threads(&alice(), &[(a, v)], t0())
        .await
        .unwrap();
    assert_eq!(scalar(&store, "SELECT count(*) FROM usage_tasks").await, 1);
    assert_eq!(
        scalar(
            &store,
            "SELECT count(*) FROM usage_tasks WHERE thread_id IS NULL"
        )
        .await,
        1
    );
    assert_eq!(scalar(&store, "SELECT count(*) FROM usage_lines").await, 1);
    assert_eq!(
        store.month(&alice(), jan()).await.unwrap().models,
        vec![glm(10, 1)]
    );
}

#[tokio::test]
async fn the_ledger_is_written_in_the_transaction_of_the_event_or_the_commit_fails() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), Vec::new()).await;

    // the ledger cannot be written: the commit fails, and the event is not in the log
    sqlx::query("ALTER TABLE usage_lines RENAME TO usage_lines_away")
        .execute(store.pool())
        .await
        .unwrap();
    let err = store
        .commit(a, v, commit(vec![call("t1", "c1", counts(10, 2))]))
        .await
        .expect_err("a ledger that cannot be written fails the commit");
    assert_eq!(err.class(), ErrorClass::Internal);
    assert_eq!(
        store.list_events(a, 0, 10).await.unwrap().len(),
        1,
        "no usage event in the log"
    );
    let after = store.get_thread(None, a).await.unwrap().unwrap();
    assert_eq!((after.version, after.last_seq), (v, 1));
    assert_eq!(
        scalar(&store, "SELECT count(*) FROM usage_tasks").await,
        0,
        "the task row went with it"
    );

    // and once it can, the same commit applies, event and rows together
    sqlx::query("ALTER TABLE usage_lines_away RENAME TO usage_lines")
        .execute(store.pool())
        .await
        .unwrap();
    append(&store, a, v, vec![call("t1", "c1", counts(10, 2))]).await;
    assert_eq!(store.list_events(a, 0, 10).await.unwrap().len(), 2);
    assert_eq!(
        store.month(&alice(), jan()).await.unwrap().models,
        vec![glm(10, 2)]
    );
}

#[tokio::test]
async fn events_a_build_without_the_ledger_appended_are_caught_up_by_the_next_commit() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), vec![call("t1", "c1", counts(10, 2))]).await;
    // another replica, one without the ledger, appends two events by hand: the log moves, the
    // ledger and `usage_through` stay where they were
    let raw = db.pool("raw", 1).await;
    for (seq, event) in [
        (3_i64, call("t1", "c2", counts(20, 3))),
        (4, total("t0", vec![glm(1000, 100)], None)),
    ] {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(a.0)
        .bind(seq)
        .bind(jiff_sqlx::Timestamp::from(event.at))
        .bind(event.body.kind().as_str())
        .bind(serde_json::to_value(&event.actor).unwrap())
        .bind(event.body.data_value())
        .execute(&raw)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE threads SET last_seq = 4, version = version + 1 WHERE id = $1")
        .bind(a.0)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(through(&store, a).await, (2, 4));
    assert_eq!(
        store.month(&alice(), jan()).await.unwrap().models,
        vec![glm(10, 2)]
    );

    append(&store, a, v + 1, vec![call("t1", "c3", counts(30, 4))]).await;
    let totals = store.month(&alice(), jan()).await.unwrap();
    assert_eq!((totals.tasks, totals.models), (2, vec![glm(1060, 109)]));
    assert_eq!(through(&store, a).await, (5, 5));
}

#[tokio::test]
async fn a_usage_event_that_does_not_decode_is_left_out_and_blocks_nothing() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), Vec::new()).await;
    sqlx::query(
        "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
         VALUES ($1, 2, $2, 'model_usage', $3, '{\"not\": \"a usage event\"}')",
    )
    .bind(a.0)
    .bind(jiff_sqlx::Timestamp::from(t0()))
    .bind(serde_json::to_value(Actor::agent(&AgentId::new("coder"), None)).unwrap())
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET last_seq = 2, version = version + 1 WHERE id = $1")
        .bind(a.0)
        .execute(store.pool())
        .await
        .unwrap();
    // the conversation goes on, and what is countable is counted
    append(&store, a, v + 1, vec![call("t1", "c1", counts(10, 2))]).await;
    assert_eq!(
        store.month(&alice(), jan()).await.unwrap().models,
        vec![glm(10, 2)]
    );
    assert_eq!(through(&store, a).await, (3, 3));
}

#[tokio::test]
async fn the_sums_the_database_makes_are_the_sums_of_the_rows() {
    let db = db_or_skip!();
    let store = db.store().await;
    let shape = |cpu, memory, commands, milliseconds| SandboxEntry {
        cpu_millis: cpu,
        memory_bytes: memory,
        commands,
        milliseconds,
    };
    let kube = |entries| SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(entries),
    };
    let mut parts = counts(1000, 100);
    parts.reasoning_tokens = Some(30);
    parts.cached_input_tokens = Some(600);
    let with_parts = ModelTokens {
        provider: Some("openai".into()),
        model: "glm".into(),
        tokens: parts,
    };
    let bare = ModelTokens {
        provider: None,
        model: "small".into(),
        tokens: counts(3, 3),
    };
    let mut threads = Vec::new();
    for n in 0..5_u64 {
        let (id, v) = open(&store, &alice(), Vec::new()).await;
        let sandbox = match n % 3 {
            0 => Some(kube(vec![
                shape(Some(250), Some(1024), n + 1, 100 * n),
                shape(None, None, 1, 1),
            ])),
            1 => Some(SandboxUsage {
                environment: SandboxEnvironment::Devcontainer,
                usage: None,
            }),
            _ => None,
        };
        append(
            &store,
            id,
            v,
            vec![
                total(
                    &format!("t{n}"),
                    vec![with_parts.clone(), bare.clone(), glm(n, n)],
                    sandbox,
                ),
                call("open", "c1", counts(n + 1, 1)),
            ],
        )
        .await;
        threads.push(id);
    }
    let mut rows = Vec::new();
    for id in threads {
        rows.extend(store.thread_rows(&alice(), id).await.unwrap());
    }
    assert_eq!(rows.len(), 10);
    let wanted = UsageMonthTotals::of(jan(), &rows);
    assert_eq!(store.month(&alice(), jan()).await.unwrap(), wanted);
    assert_eq!(wanted.tasks, 10);
    assert!(
        wanted
            .models
            .iter()
            .any(|m| m.tokens.reasoning_tokens == Some(150))
    );
    assert_eq!(wanted.unmeasured.len(), 1);
}

#[tokio::test]
async fn a_counter_that_fills_stops_at_the_largest_bigint_and_the_commit_succeeds() {
    let db = db_or_skip!();
    let store = db.store().await;
    let (a, v) = open(&store, &alice(), Vec::new()).await;
    // 1030 calls of 2^53 - 1: past what a bigint holds, which an agent that lies could report
    let calls: Vec<NewEvent> = (0..1030)
        .map(|i| call("t1", &format!("c{i}"), counts(MAX_TOKEN_COUNT, 0)))
        .collect();
    append(&store, a, v, calls).await;
    let totals = store.month(&alice(), jan()).await.unwrap();
    assert_eq!(totals.models[0].tokens.input_tokens, MAX_LEDGER_COUNT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_backfill_runs_beside_commits_and_loses_nothing() {
    let db = db_or_skip!();
    let store = db.store().await;
    let mut threads = Vec::new();
    for n in 0..12_u64 {
        let (id, v) = open(&store, &alice(), vec![call("t1", "c1", counts(10 + n, 1))]).await;
        let v = append(&store, id, v, vec![call("t1", "c2", counts(1, 1))]).await;
        threads.push((id, v));
    }
    let expected_before = store.month(&alice(), jan()).await.unwrap();
    forget(&store).await;
    assert_eq!(store.month(&alice(), jan()).await.unwrap().tasks, 0);

    // the service goes on writing to half of the threads while the backfill goes through all of them
    let writers = {
        let store = store.clone();
        let threads = threads.clone();
        tokio::spawn(async move {
            for (i, (id, v)) in threads.into_iter().enumerate() {
                if i % 2 == 0 {
                    append(&store, id, v, vec![call("t1", "c3", counts(100, 1))]).await;
                }
            }
        })
    };
    let backfill = {
        let store = store.clone();
        tokio::spawn(async move {
            let mut after = None;
            let mut folded = UsageBackfill::default();
            loop {
                let page = store.backfill(after, 3).await.unwrap();
                folded.threads += page.threads;
                folded.events += page.events;
                match page.next {
                    Some(next) => after = Some(next),
                    None => return folded,
                }
            }
        })
    };
    writers.await.unwrap();
    let folded = backfill.await.unwrap();
    // each thread was gone through once, by the backfill or by the writer's commit
    assert!(folded.threads <= 12);

    let totals = store.month(&alice(), jan()).await.unwrap();
    // 12 threads: the sum of their 10 + n and 1, 1; plus 100 + 1 on six of them
    let sum: u64 = (0..12).map(|n| 10 + n + 1).sum::<u64>() + 6 * 100;
    assert_eq!(totals.tasks, 12);
    assert_eq!(totals.models[0].tokens.input_tokens, sum);
    assert_eq!(expected_before.tasks, 12);

    // and a second pass finds nothing
    let again = store.backfill(None, 100).await.unwrap();
    assert_eq!((again.threads, again.events), (0, 0));
    assert_eq!(store.month(&alice(), jan()).await.unwrap(), totals);
}

/// Migration 0023 on a database that has run 0001 to 0022 and holds threads with usage in their
/// logs, one of them a fork that copied the first's: nothing is in the ledger until the backfill goes
/// through them, and then the usage is counted once, the copied events not at all (ADR 0065).
#[tokio::test]
async fn migration_0023_upgrades_a_database_that_holds_logs_and_the_backfill_fills_the_ledger() {
    let db = db_or_skip!();
    let pool = db.pool("orch-test-upgrade", 4).await;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("orch-migrations-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.as_str() < "0023" {
            std::fs::copy(&path, dir.join(&name)).unwrap();
        }
    }
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    // threads and logs as a build without the ledger wrote them
    let parent = ThreadId(Uuid::now_v7());
    let fork = ThreadId(Uuid::now_v7());
    let new_thread = |id: ThreadId, last_seq: i64, forked: bool| {
        sqlx::query(
            "INSERT INTO threads (id, owner, title, agent_id, state, version, last_seq, created_at, \
             updated_at, rail_rank, forked_from, forked_at, fork_kind) \
             VALUES ($1, 'alice@example.com', 't', 'coder', 'done', 1, $2, now(), now(), 'i', \
             $3, $4, $5)",
        )
        .bind(id.0)
        .bind(last_seq)
        .bind(forked.then_some(parent.0))
        .bind(forked.then_some(3_i64))
        .bind(forked.then_some("fork"))
    };
    new_thread(parent, 3, false).execute(&pool).await.unwrap();
    new_thread(fork, 4, true).execute(&pool).await.unwrap();
    let insert = |thread: ThreadId, seq: i64, event: &NewEvent| {
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(thread.0)
        .bind(seq)
        .bind(jiff_sqlx::Timestamp::from(event.at))
        .bind(event.body.kind().as_str())
        .bind(serde_json::to_value(&event.actor).unwrap())
        .bind(event.body.data_value())
    };
    let first = call("t1", "c1", counts(10, 2));
    let second = total("t1", vec![glm(12, 3)], None);
    insert(parent, 1, &message()).execute(&pool).await.unwrap();
    insert(parent, 2, &first).execute(&pool).await.unwrap();
    insert(parent, 3, &second).execute(&pool).await.unwrap();
    // the fork copied all three and went on with a call of its own
    insert(fork, 1, &message()).execute(&pool).await.unwrap();
    insert(fork, 2, &first).execute(&pool).await.unwrap();
    insert(fork, 3, &second).execute(&pool).await.unwrap();
    insert(fork, 4, &call("t9", "c1", counts(5, 5)))
        .execute(&pool)
        .await
        .unwrap();

    let store = PgStore::from_pool(pool);
    store.migrate().await.unwrap();
    assert_eq!(
        through(&store, parent).await,
        (0, 3),
        "nothing has been gone through"
    );
    assert_eq!(store.month(&alice(), jan()).await.unwrap().tasks, 0);

    let mut after = None;
    let mut done = UsageBackfill::default();
    loop {
        let page = store.backfill(after, 1).await.unwrap();
        done.threads += page.threads;
        done.events += page.events;
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    assert_eq!(
        (done.threads, done.events),
        (2, 3),
        "the parent's two and the fork's own one"
    );
    let totals = store.month(&alice(), jan()).await.unwrap();
    assert_eq!(totals.tasks, 2);
    assert_eq!(
        totals.models,
        vec![glm(17, 8)],
        "12 + 3 of the parent's total and 5 + 5 of the fork's call"
    );
    assert_eq!(through(&store, parent).await, (3, 3));
    assert_eq!(through(&store, fork).await, (4, 4));
    let again = store.backfill(None, 10).await.unwrap();
    assert_eq!((again.threads, again.events), (0, 0));
    assert_eq!(store.month(&alice(), jan()).await.unwrap(), totals);
}

/// Counts the statements the driver logs (`sqlx::query`) while a future runs.
struct Statements(Arc<AtomicUsize>);

impl<S: tracing::Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many statements `work` sends to the database.
async fn statements_of<F: std::future::Future>(work: F) -> (usize, F::Output) {
    let counted = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(Statements(counted.clone()));
    let out = work.with_subscriber(subscriber).await;
    (counted.load(Ordering::Relaxed), out)
}

/// `n` usage events (calls of `tasks` tasks on two models) written to the log as a build without the
/// ledger would write them, after the thread's seq `first - 1`.
async fn write_calls_by_hand(
    store: &PgStore,
    thread: ThreadId,
    first: i64,
    n: usize,
    tasks: usize,
) -> i64 {
    let (mut seqs, mut ats, mut kinds, mut actors, mut datas) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..n {
        let mut event = call(
            &format!("t{}", i % tasks),
            &format!("c{first}-{i}"),
            counts(10, 2),
        );
        if let EventBody::ModelUsage(data) = &mut event.body {
            data.model = if i % 2 == 0 { "glm" } else { "small" }.into();
        }
        seqs.push(first + i64::try_from(i).unwrap());
        ats.push(event.at.to_string());
        kinds.push(event.body.kind().as_str().to_owned());
        actors.push(serde_json::to_value(&event.actor).unwrap());
        datas.push(event.body.data_value());
    }
    for start in (0..n).step_by(1000) {
        let end = (start + 1000).min(n);
        sqlx::query(
            "INSERT INTO events (thread_id, seq, at, kind, actor, data) \
             SELECT $1, s, a::timestamptz, k, ac, d \
             FROM unnest($2::bigint[], $3::text[], $4::text[], $5::jsonb[], $6::jsonb[]) \
             AS u(s, a, k, ac, d)",
        )
        .bind(thread.0)
        .bind(&seqs[start..end])
        .bind(&ats[start..end])
        .bind(&kinds[start..end])
        .bind(&actors[start..end])
        .bind(&datas[start..end])
        .execute(store.pool())
        .await
        .unwrap();
    }
    let last = first + i64::try_from(n).unwrap() - 1;
    sqlx::query("UPDATE threads SET last_seq = $2, version = version + 1 WHERE id = $1")
        .bind(thread.0)
        .bind(last)
        .execute(store.pool())
        .await
        .unwrap();
    last
}

/// A catch-up costs statements by the tasks it touches and the pages of the log it reads, not by the
/// events it folds: the same number of tasks costs about the same for twice the events. (One
/// statement per event made a thread of 10 000 events a commit of many seconds under the thread's
/// lock.)
#[tokio::test]
async fn a_catch_up_costs_statements_by_task_and_page_not_by_event() {
    let db = db_or_skip!();
    let store = db.store().await;
    let mut cost = Vec::new();
    for n in [1200_usize, 2400] {
        let (id, v) = open(&store, &alice(), Vec::new()).await;
        let last = write_calls_by_hand(&store, id, 2, n, 3).await;
        assert_eq!(through(&store, id).await, (1, last));
        let (statements, _) = statements_of(append(&store, id, v + 1, vec![message()])).await;
        assert_eq!(through(&store, id).await, (last + 1, last + 1));
        cost.push(statements);
    }
    // three tasks and a few pages: tens of statements; one per event would be thousands
    assert!(
        cost[0] < 100,
        "1200 events on 3 tasks: {} statements",
        cost[0]
    );
    assert!(
        cost[1] <= cost[0] + 6,
        "twice the events cost only the pages more: {cost:?}"
    );
    let totals = store.month(&alice(), jan()).await.unwrap();
    assert_eq!(totals.tasks, 6);
    assert_eq!(totals.models.len(), 2);
}
