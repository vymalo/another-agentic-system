//! The generic CI webhook end to end, on both stores, with a clock the test holds: a signed
//! report goes through the real HTTP route, the inbox and the worker to the pure core, and the
//! job ends where the gate says.
//!
//! * a report that beats its watch is parked, matched when the agent's push starts the watch, and
//!   the job is done;
//! * a red report sends the agent back to work (attempt 2, with the findings), and the green one
//!   for the new commit finishes the job;
//! * with no report the deadline comes due, the thread is blocked with `ci_timeout`, and no
//!   attempt is spent.
//!
//! The agent is played through `App::apply` (the fake agent would only get in the way) and the
//! inbox worker is driven one `tick` at a time, so nothing here waits or races: time is the
//! `FixedClock`, which is also what the route reads to judge a timestamp.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_api::{ApiConfig, router_with_surfaces};
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, ApplyOutcome, InboxConfig, InboxWorker, NewThread,
};
use orch_core::{
    AgentId, AgentTarget, AgentTaskState, AgentUpdate, CheckSource, CiPolicy, Event, EventBody,
    EventKind, GatePolicy, Hold, Input, ThreadId, ThreadRecord, ThreadState, UserId,
};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, InboxStatus, PortSet, Ports as _, ThreadStore, Wakeup};
use orch_store_postgres::{PgStore, PgWakeup};
use orch_surface_webhook::signature::sign_generic;
use orch_surface_webhook::{GenericConfig, Secrets, generic};
use orch_testsupport::fake::{VERIFY_REPOSITORY, verify_commit};
use serde_json::{Value, json};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../store-postgres/tests/support/mod.rs"]
mod pgdb;

const SECRET: &str = "e2e-webhook-secret";
const CI_TIMEOUT_SECS: i64 = 3600;
/// 2026-09-30T12:00:00Z.
const T0: i64 = 1_790_769_600;

/// Defines, for each scenario `async fn name<S, W>(rig: Rig<S, W>)`, the tests `name::memory` and
/// `name::postgres` (a no-op unless `ORCH_TEST_DATABASE_URL` is set).
macro_rules! backends {
    ($($name:ident),+ $(,)?) => {
        $(
            mod $name {
                #[tokio::test]
                async fn memory() {
                    super::$name(super::memory_rig().await).await;
                }

                #[tokio::test]
                async fn postgres() {
                    match super::postgres_rig().await {
                        Some(rig) => super::$name(rig).await,
                        None => eprintln!(
                            "skipping the Postgres variant: ORCH_TEST_DATABASE_URL is not set"
                        ),
                    }
                }
            }
        )+
    };
}

type Ports<S, W> = PortSet<S, W, ScriptedAgent, FixedClock, SeqIds>;

/// The application, its inbox worker, the HTTP route and the clock, on one store.
struct Rig<S: ThreadStore, W: Wakeup> {
    app: Arc<App<Ports<S, W>>>,
    worker: Arc<InboxWorker<Ports<S, W>>>,
    clock: FixedClock,
    base: String,
    client: reqwest::Client,
    /// Keeps the Postgres schema's owner alive for the length of the test.
    _db: Option<pgdb::TestDb>,
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn gate() -> GatePolicy {
    GatePolicy {
        ci: CiPolicy {
            timeout: SignedDuration::from_secs(CI_TIMEOUT_SECS),
            ..CiPolicy::default()
        },
        max_attempts: 3,
        ..GatePolicy::requiring([CheckSource::Ci])
    }
}

async fn rig<S: ThreadStore, W: Wakeup>(
    store: S,
    wakeup: W,
    db: Option<pgdb::TestDb>,
) -> Rig<S, W> {
    let clock = FixedClock::new(Timestamp::from_second(T0).unwrap());
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("plain"),
            "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            None,
        ),
        name: "Plain".to_owned(),
    }]);
    let app = Arc::new(
        App::new(
            PortSet {
                store,
                wakeup,
                agents: ScriptedAgent::new(),
                clock: clock.clone(),
                ids: SeqIds::default(),
            },
            directory,
            AppConfig {
                gate: gate(),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    let worker = InboxWorker::new(Arc::clone(&app), InboxConfig::default(), "e2e-inbox");
    let router = router_with_surfaces(
        Arc::clone(&app),
        ApiConfig::default(),
        vec![generic::routes(
            Arc::clone(&app),
            GenericConfig::new(Secrets::parse(SECRET).unwrap()),
        )],
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    Rig {
        app,
        worker,
        clock,
        base,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        _db: db,
    }
}

async fn memory_rig() -> Rig<MemoryStore, MemoryWakeup> {
    rig(MemoryStore::new(), MemoryWakeup::new(), None).await
}

async fn postgres_rig() -> Option<Rig<PgStore, PgWakeup>> {
    let db = pgdb::TestDb::new().await?;
    let store = db.store().await; // migrates
    let wakeup = PgWakeup::start(db.pool("e2e-webhook", 4).await);
    assert!(
        wakeup.wait_listening(Duration::from_secs(10)).await,
        "the wakeup listener did not attach"
    );
    Some(rig(store, wakeup, Some(db)).await)
}

fn from_agent(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

/// The agent says it pushed `commit` to the repository the reports are about.
fn pushed(commit: &str) -> Input {
    from_agent(AgentUpdate::Artifact {
        name: "branch".to_owned(),
        mime_type: None,
        uri: None,
        text: Some(
            json!({"repository": VERIFY_REPOSITORY, "branch": "agent/fix", "commit": commit})
                .to_string(),
        ),
    })
}

fn completed() -> Input {
    from_agent(AgentUpdate::Status {
        state: AgentTaskState::Completed,
        detail: None,
    })
}

/// A generic report about `commit`, as a CI system sends it.
fn report(commit: &str, name: &str, conclusion: &str, summary: Option<&str>) -> Value {
    let mut body = json!({
        "version": 1,
        "repository": "https://github.com/acme/demo",
        "sha": commit,
        "branch": "agent/fix",
        "name": name,
        "conclusion": conclusion,
        "url": format!("https://ci.example.com/runs/{commit}"),
    });
    if let Some(summary) = summary {
        body["summary"] = json!(summary);
    }
    body
}

impl<S: ThreadStore, W: Wakeup> Rig<S, W> {
    /// POSTs `body` to the route as a CI system would, signed at the clock's now, under
    /// `delivery`. Returns the status.
    async fn post(&self, delivery: u128, body: &Value) -> u16 {
        let body = body.to_string();
        let ts = orch_ports::Clock::now(&self.clock).as_second().to_string();
        let signature = sign_generic(SECRET, &ts, body.as_bytes()).unwrap();
        self.client
            .post(format!("{}/webhooks/ci", self.base))
            .header("Content-Type", "application/json")
            .header("X-Vymalo-Delivery", uuid_of(delivery))
            .header("X-Vymalo-Timestamp", ts)
            .header("X-Vymalo-Signature-256", signature)
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// Runs the inbox worker until a pass claims nothing.
    async fn drain(&self) {
        while self.worker.tick().await > 0 {}
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
    }

    async fn create(&self) -> ThreadRecord {
        self.app
            .create_thread(
                &alice(),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "ship it".to_owned(),
                },
            )
            .await
            .unwrap()
    }

    /// Plays the agent.
    async fn agent(&self, thread: ThreadId, input: Input) {
        let outcome = self
            .app
            .apply(thread, input, None, None, None)
            .await
            .unwrap();
        assert!(
            matches!(outcome, ApplyOutcome::Applied { .. }),
            "{outcome:?}"
        );
    }

    async fn thread(&self, id: ThreadId) -> ThreadRecord {
        self.app.get_thread(&alice(), id).await.unwrap()
    }

    async fn events(&self, id: ThreadId) -> Vec<Event> {
        self.app.list_events(&alice(), id, 0, 500).await.unwrap()
    }

    async fn row_status(&self, delivery: u128) -> Option<InboxStatus> {
        self.app
            .ports()
            .store()
            .find_inbox(generic::SOURCE, &uuid_of(delivery))
            .await
            .unwrap()
            .map(|row| row.status)
    }
}

/// A delivery id: the UUID with `n` in its low bytes.
fn uuid_of(n: u128) -> String {
    uuid::Uuid::from_u128(0x0195_c1a2_7b3e_7c11_8f2a_0000_0000_0000 | n).to_string()
}

fn cards(events: &[Event]) -> Vec<&orch_core::CiReport> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::CiResult(report) => Some(report),
            _ => None,
        })
        .collect()
}

fn last_state(events: &[Event]) -> Option<ThreadState> {
    events.iter().rev().find_map(|e| match &e.body {
        EventBody::ThreadState(d) => Some(d.state),
        _ => None,
    })
}

/// CI is faster than the agent: its report reaches the route before any thread watches the
/// commit. It is stored and parked; the agent's push starts the watch and wakes it; the job that
/// had nothing to do but wait for CI is done.
async fn a_report_received_before_its_watch_is_parked_then_matched_and_the_job_is_done<
    S: ThreadStore,
    W: Wakeup,
>(
    rig: Rig<S, W>,
) {
    let thread = rig.create().await;
    let commit = verify_commit(1);

    assert_eq!(
        rig.post(
            1,
            &report(&commit, "ci/build", "success", Some("212 passed"))
        )
        .await,
        202
    );
    assert_eq!(rig.row_status(1).await, Some(InboxStatus::Pending));
    rig.drain().await;
    assert_eq!(
        rig.row_status(1).await,
        Some(InboxStatus::Parked),
        "no thread watches the commit yet"
    );
    // A redelivery of a parked report is acknowledged and changes nothing.
    assert_eq!(
        rig.post(
            1,
            &report(&commit, "ci/build", "success", Some("212 passed"))
        )
        .await,
        202
    );
    rig.drain().await;
    assert_eq!(rig.row_status(1).await, Some(InboxStatus::Parked));

    // The agent pushes (which starts the watch, re-arming the parked report) and finishes (which
    // starts the verification).
    rig.agent(thread.id, pushed(&commit)).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);
    rig.drain().await;

    let record = rig.thread(thread.id).await;
    assert_eq!(record.state, ThreadState::Done);
    assert_eq!(rig.row_status(1).await, Some(InboxStatus::Applied));
    let events = rig.events(thread.id).await;
    let cards = cards(&events);
    assert_eq!(cards.len(), 1, "one card, whatever the redeliveries");
    assert_eq!(cards[0].name, "ci/build");
    assert_eq!(cards[0].summary.as_deref(), Some("212 passed"));
    assert_eq!(cards[0].repository, "github.com/acme/demo");
    assert_eq!(last_state(&events), Some(ThreadState::Done));
}

/// A red report sends the agent back to work with the findings, and spends an attempt; the green
/// report for the new commit finishes the job.
async fn a_red_report_reworks_and_the_green_one_for_the_new_commit_finishes<
    S: ThreadStore,
    W: Wakeup,
>(
    rig: Rig<S, W>,
) {
    let thread = rig.create().await;
    let first = verify_commit(1);
    let second = verify_commit(2);
    rig.agent(thread.id, pushed(&first)).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);

    let summary = "2 tests failed: parser::empty, parser::utf8";
    assert_eq!(
        rig.post(1, &report(&first, "ci/build", "failure", Some(summary)))
            .await,
        202
    );
    rig.drain().await;

    let record = rig.thread(thread.id).await;
    assert_eq!(record.state, ThreadState::Queued, "back to the agent");
    assert_eq!(record.job.attempt, 2);
    let events = rig.events(thread.id).await;
    let rework = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Rework(data) => Some(data.clone()),
            _ => None,
        })
        .expect("a rework event");
    assert_eq!(rework.attempt, 2);
    let findings = &rework.findings[0];
    assert_eq!(findings.source, CheckSource::Ci);
    assert!(
        findings
            .findings
            .iter()
            .any(|f| f.contains("ci/build") && f.contains(summary)),
        "the report is in the findings: {:?}",
        findings.findings
    );
    assert_eq!(cards(&events).len(), 1);

    // Attempt 2: the agent pushes a new commit; the old commit's late report changes nothing,
    // the new commit's green one decides.
    rig.agent(thread.id, pushed(&second)).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(
        rig.post(2, &report(&first, "ci/build", "failure", None))
            .await,
        202
    );
    rig.drain().await;
    assert_eq!(
        rig.thread(thread.id).await.state,
        ThreadState::Verifying,
        "a report about the old commit is a card and nothing else"
    );
    assert_eq!(
        rig.post(3, &report(&second, "ci/build", "success", None))
            .await,
        202
    );
    rig.drain().await;

    let record = rig.thread(thread.id).await;
    assert_eq!(record.state, ThreadState::Done);
    assert_eq!(record.job.attempt, 2);
    assert_eq!(
        cards(&rig.events(thread.id).await).len(),
        3,
        "every report has a card"
    );
}

/// No report ever comes. Nothing happens before the deadline; at the deadline the thread is
/// blocked, with the interrupt reason `ci_timeout`, and the attempt is not spent.
async fn a_ci_deadline_blocks_the_thread<S: ThreadStore, W: Wakeup>(rig: Rig<S, W>) {
    let thread = rig.create().await;
    rig.agent(thread.id, pushed(&verify_commit(1))).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);

    rig.advance(u64::try_from(CI_TIMEOUT_SECS).unwrap() - 1);
    rig.drain().await;
    assert_eq!(
        rig.thread(thread.id).await.state,
        ThreadState::Verifying,
        "one second early"
    );
    rig.advance(1);
    rig.drain().await;

    let record = rig.thread(thread.id).await;
    assert_eq!(record.state, ThreadState::Blocked);
    assert_eq!(record.job.hold, Some(Hold::CiTimeout));
    assert_eq!(record.job.attempt, 1, "a timeout spends no attempt");
    let events = rig.events(thread.id).await;
    assert!(events.iter().any(|e| e.kind() == EventKind::Error));
    assert_eq!(last_state(&events), Some(ThreadState::Blocked));

    // A report that comes after the deadline is still a card, and the thread stays blocked for
    // the user to answer.
    assert_eq!(
        rig.post(1, &report(&verify_commit(1), "ci/build", "success", None))
            .await,
        202
    );
    rig.drain().await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Blocked);
    assert_eq!(cards(&rig.events(thread.id).await).len(), 1);
}

/// A refused delivery reaches neither the inbox nor a thread.
async fn a_refused_delivery_changes_nothing<S: ThreadStore, W: Wakeup>(rig: Rig<S, W>) {
    let thread = rig.create().await;
    let commit = verify_commit(1);
    rig.agent(thread.id, pushed(&commit)).await;
    rig.agent(thread.id, completed()).await;
    let before = rig.events(thread.id).await.len();

    let body = report(&commit, "ci/build", "success", None).to_string();
    // A stale timestamp, signed as sent.
    let ts = (orch_ports::Clock::now(&rig.clock).as_second() - 301).to_string();
    let signature = sign_generic(SECRET, &ts, body.as_bytes()).unwrap();
    let res = rig
        .client
        .post(format!("{}/webhooks/ci", rig.base))
        .header("X-Vymalo-Delivery", uuid_of(9))
        .header("X-Vymalo-Timestamp", ts)
        .header("X-Vymalo-Signature-256", signature)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // The right secret is not enough with the wrong one's signature.
    let ts = orch_ports::Clock::now(&rig.clock).as_second().to_string();
    let signature = sign_generic("guess", &ts, body.as_bytes()).unwrap();
    let res = rig
        .client
        .post(format!("{}/webhooks/ci", rig.base))
        .header("X-Vymalo-Delivery", uuid_of(9))
        .header("X-Vymalo-Timestamp", ts)
        .header("X-Vymalo-Signature-256", signature)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    assert_eq!(rig.row_status(9).await, None);
    rig.drain().await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);
    assert_eq!(rig.events(thread.id).await.len(), before);
}

backends!(
    a_report_received_before_its_watch_is_parked_then_matched_and_the_job_is_done,
    a_red_report_reworks_and_the_green_one_for_the_new_commit_finishes,
    a_ci_deadline_blocks_the_thread,
    a_refused_delivery_changes_nothing,
);
