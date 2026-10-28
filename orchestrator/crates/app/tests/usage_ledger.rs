//! The usage ledger through the application (ADR 0065, PR 2): a person's month priced with the
//! configured table, and the retention sweep that removes the months older than the retention.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_app::{App, AppConfig, UsageSettings, UsageSweepConfig, UsageSweepWorker};
use orch_core::{
    Actor, AgentId, AgentTarget, Currency, EventBody, LinePrice, Micros, ModelPrice,
    ModelUsageData, ModelUsageTotalData, PriceTable, ThreadId, ThreadState, TokenCounts, Unpriced,
    UsageMonth, UserId, UserMessageData,
};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    Commit, NewEvent, NewThreadRecord, PortSet, RefuseAll, ThreadStore, UsageLedger as _,
};
use support::{alice, bob, directory, eventually};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type UsagePorts = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    FixedClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    RefuseAll,
>;

/// The clock of every test: late October 2026, so the current month is `2026-10`.
const NOW: &str = "2026-10-28T12:00:00Z";

fn month(text: &str) -> UsageMonth {
    text.parse().unwrap()
}

fn app(store: &MemoryStore, usage: UsageSettings) -> Arc<App<UsagePorts>> {
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
            AppConfig {
                usage,
                ..AppConfig::default()
            },
        )
        .expect("a valid configuration"),
    )
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

/// A call of task `task` made at `when`, on the model `glm` of `openai`.
fn call(when: &str, task: &str, tokens: TokenCounts) -> NewEvent {
    NewEvent {
        at: when.parse::<Timestamp>().unwrap(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            call: format!("c-{task}"),
            path: Vec::new(),
            provider: Some("openai".into()),
            model: "glm".into(),
            tokens,
            context_window: None,
        })),
        idempotency_key: None,
    }
}

fn total(when: &str, task: &str, model: &str, tokens: TokenCounts) -> NewEvent {
    NewEvent {
        at: when.parse::<Timestamp>().unwrap(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            path: Vec::new(),
            totals: vec![orch_core::ModelTokens {
                provider: Some("openai".into()),
                model: model.into(),
                tokens,
            }],
            sandbox: None,
        }),
        idempotency_key: None,
    }
}

/// A thread of `owner` whose log holds `events`.
async fn thread(store: &MemoryStore, owner: &UserId, events: Vec<NewEvent>) -> ThreadId {
    let id = ThreadId(Uuid::now_v7());
    let mut all = vec![NewEvent {
        at: NOW.parse().unwrap(),
        actor: Actor::user(owner),
        body: EventBody::UserMessage(UserMessageData::new("hi")),
        idempotency_key: None,
    }];
    all.extend(events);
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
                now: NOW.parse().unwrap(),
            },
            Commit {
                new_state: ThreadState::Working,
                job: None,
                events: all,
                outbox: Vec::new(),
                binding: None,
                now: NOW.parse().unwrap(),
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

fn glm_price() -> PriceTable {
    PriceTable::new(
        Currency::new("EUR").unwrap(),
        vec![ModelPrice {
            provider: Some("openai".into()),
            model: "glm".into(),
            input: Micros::new(400_000),
            cached_input: Micros::new(100_000),
            output: Micros::new(1_600_000),
        }],
        None,
    )
    .unwrap()
}

fn priced(prices: Option<PriceTable>) -> UsageSettings {
    UsageSettings {
        prices,
        ..UsageSettings::default()
    }
}

// ---- the month ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_person_reads_their_month_priced_with_the_configured_table() {
    let store = MemoryStore::new();
    thread(
        &store,
        &alice(),
        vec![
            total(
                "2026-10-03T09:00:00Z",
                "t1",
                "glm",
                counts(1_000_000, 500_000),
            ),
            // a model with no price
            total("2026-10-04T09:00:00Z", "t2", "glm-mini", counts(10, 10)),
            // another month
            total("2026-09-04T09:00:00Z", "t3", "glm", counts(3, 3)),
        ],
    )
    .await;
    thread(
        &store,
        &bob(),
        vec![total("2026-10-05T09:00:00Z", "t1", "glm", counts(7, 7))],
    )
    .await;
    let app = app(&store, priced(Some(glm_price())));

    // the current month by default: the clock's
    let answer = app.usage_month(&alice(), None).await.unwrap();
    assert_eq!(answer.month, month("2026-10"));
    assert_eq!(answer.tasks, 2, "alice's own, not bob's");
    assert_eq!(answer.currency, Some(Currency::new("EUR").unwrap()));
    assert_eq!(answer.prices.as_ref(), Some(glm_price().id()));
    assert_eq!(answer.models.len(), 2);
    // 1 000 000 in at 0.40, 500 000 out at 1.60: 0.40 + 0.80
    assert_eq!(answer.models[0].usage.model, "glm");
    assert_eq!(
        answer.models[0].price,
        LinePrice::Priced(Micros::new(1_200_000))
    );
    assert_eq!(answer.models[1].usage.model, "glm-mini");
    assert_eq!(
        answer.models[1].price,
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(answer.total.amount, Micros::new(1_200_000));
    assert!(!answer.total.complete);

    // a named month, and a month in which the person used nothing
    let september = app
        .usage_month(&alice(), Some(month("2026-09")))
        .await
        .unwrap();
    assert_eq!(september.month, month("2026-09"));
    assert_eq!(september.tasks, 1);
    let nothing = app
        .usage_month(&alice(), Some(month("2026-08")))
        .await
        .unwrap();
    assert_eq!((nothing.tasks, nothing.models.len()), (0, 0));
    assert!(nothing.total.complete);
    assert_eq!(nothing.total.amount, Micros::ZERO);

    // another person sees their own
    let theirs = app.usage_month(&bob(), None).await.unwrap();
    assert_eq!(theirs.tasks, 1);
    assert_eq!(theirs.models[0].usage.tokens.input_tokens, 7);
    // somebody who used nothing at all
    let carol = UserId::new("carol@example.com");
    assert_eq!(app.usage_month(&carol, None).await.unwrap().tasks, 0);
}

#[tokio::test]
async fn without_a_table_nothing_is_priced_and_the_answer_says_so() {
    let store = MemoryStore::new();
    thread(
        &store,
        &alice(),
        vec![total("2026-10-03T09:00:00Z", "t1", "glm", counts(5, 5))],
    )
    .await;
    let app = app(&store, UsageSettings::default());
    let answer = app.usage_month(&alice(), None).await.unwrap();
    assert_eq!((answer.currency, answer.prices), (None, None));
    assert_eq!(
        answer.models[0].price,
        LinePrice::Unpriced(Unpriced::NoTable)
    );
    assert_eq!(answer.total.amount, Micros::ZERO);
    assert!(!answer.total.complete);
}

// ---- the sweep ---------------------------------------------------------------------------------

fn fast_sweep() -> UsageSweepConfig {
    UsageSweepConfig {
        interval: Duration::from_millis(20),
        batch: 1000,
        pause: Duration::ZERO,
    }
}

#[tokio::test]
async fn the_sweep_keeps_the_current_month_as_one_of_the_months_of_the_retention() {
    let store = MemoryStore::new();
    // 24 months kept from 2026-10: 2024-11 is the first, 2024-10 is the last to go
    thread(
        &store,
        &alice(),
        vec![
            total("2024-09-15T09:00:00Z", "gone-1", "glm", counts(1, 1)),
            total("2024-10-31T23:59:59Z", "gone-2", "glm", counts(1, 1)),
            total("2024-11-01T00:00:00Z", "kept-1", "glm", counts(1, 1)),
            total("2025-06-15T09:00:00Z", "kept-2", "glm", counts(1, 1)),
            total("2026-10-03T09:00:00Z", "kept-3", "glm", counts(1, 1)),
        ],
    )
    .await;
    thread(
        &store,
        &bob(),
        vec![total("2023-01-15T09:00:00Z", "gone-3", "glm", counts(1, 1))],
    )
    .await;
    let app = app(&store, UsageSettings::default());
    assert_eq!(app.usage_settings().retention_months, 24);
    let worker = UsageSweepWorker::new(Arc::clone(&app), fast_sweep());

    assert_eq!(worker.sweep().await, 3);
    for (who, m, tasks) in [
        (alice(), "2024-09", 0),
        (alice(), "2024-10", 0),
        (alice(), "2024-11", 1),
        (alice(), "2025-06", 1),
        (alice(), "2026-10", 1),
        (bob(), "2023-01", 0),
    ] {
        assert_eq!(
            store.month(&who, month(m)).await.unwrap().tasks,
            tasks,
            "{who} {m}"
        );
    }
    assert_eq!(app.usage_stats().tasks_swept, 3);

    // again: nothing, and the counter stands
    assert_eq!(worker.sweep().await, 0);
    assert_eq!(app.usage_stats().tasks_swept, 3);
}

#[tokio::test]
async fn the_retention_is_the_deployments() {
    let store = MemoryStore::new();
    thread(
        &store,
        &alice(),
        vec![
            total("2026-08-15T09:00:00Z", "aug", "glm", counts(1, 1)),
            total("2026-09-15T09:00:00Z", "sep", "glm", counts(1, 1)),
            total("2026-10-03T09:00:00Z", "oct", "glm", counts(1, 1)),
        ],
    )
    .await;
    let app = app(
        &store,
        UsageSettings {
            retention_months: 2,
            ..UsageSettings::default()
        },
    );
    let worker = UsageSweepWorker::new(app, fast_sweep());
    assert_eq!(
        worker.sweep().await,
        1,
        "this month and the one before stay"
    );
    assert_eq!(
        store.month(&alice(), month("2026-08")).await.unwrap().tasks,
        0
    );
    assert_eq!(
        store.month(&alice(), month("2026-09")).await.unwrap().tasks,
        1
    );
    assert_eq!(
        store.month(&alice(), month("2026-10")).await.unwrap().tasks,
        1
    );
}

#[tokio::test]
async fn a_backlog_is_removed_a_batch_at_a_time_to_the_end() {
    let store = MemoryStore::new();
    let events: Vec<NewEvent> = (0..7)
        .map(|i| {
            call(
                &format!("2020-{:02}-15T09:00:00Z", i + 1),
                &format!("t{i}"),
                counts(1, 1),
            )
        })
        .chain([call("2026-10-03T09:00:00Z", "new", counts(1, 1))])
        .collect();
    thread(&store, &alice(), events).await;
    let app = app(&store, UsageSettings::default());
    // seven old tasks, a batch of three: three full batches' worth and a short one
    let worker = UsageSweepWorker::new(
        Arc::clone(&app),
        UsageSweepConfig {
            batch: 3,
            ..fast_sweep()
        },
    );
    assert_eq!(worker.sweep().await, 7);
    assert_eq!(app.usage_stats().tasks_swept, 7);
    assert_eq!(
        store.month(&alice(), month("2026-10")).await.unwrap().tasks,
        1
    );
    // a backlog of exactly one batch is taken and the next, empty, ends the pass
    thread(
        &store,
        &bob(),
        (0..3)
            .map(|i| {
                call(
                    &format!("2020-{:02}-15T09:00:00Z", i + 1),
                    &format!("b{i}"),
                    counts(1, 1),
                )
            })
            .collect(),
    )
    .await;
    assert_eq!(worker.sweep().await, 3);
}

#[tokio::test]
async fn the_worker_sweeps_when_it_starts_and_again_and_stops_when_told() {
    let store = MemoryStore::new();
    thread(
        &store,
        &alice(),
        vec![call("2020-01-15T09:00:00Z", "old", counts(1, 1))],
    )
    .await;
    let app = app(&store, UsageSettings::default());
    let stop = CancellationToken::new();
    let run = tokio::spawn(UsageSweepWorker::new(Arc::clone(&app), fast_sweep()).run(stop.clone()));

    eventually("the old task is swept", || async {
        (app.usage_stats().tasks_swept == 1).then_some(())
    })
    .await;
    // usage that turns old while the process runs is found by a later pass
    thread(
        &store,
        &bob(),
        vec![call("2020-02-15T09:00:00Z", "later", counts(1, 1))],
    )
    .await;
    eventually("the next pass sweeps it", || async {
        (app.usage_stats().tasks_swept == 2).then_some(())
    })
    .await;

    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("the worker stops when told")
        .unwrap();
}
