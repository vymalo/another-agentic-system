//! The CI webhooks end to end, on both stores, with a clock the test holds: a signed
//! report goes through the real HTTP route, the inbox and the worker to the pure core, and the
//! job ends where the gate says.
//!
//! * a report that beats its watch is parked, matched when the agent's push starts the watch, and
//!   the job is done;
//! * a red report sends the agent back to work (attempt 2, with the findings), and the green one
//!   for the new commit finishes the job;
//! * with no report the deadline comes due, the thread is blocked with `ci_timeout`, and no
//!   attempt is spent;
//! * GitHub's own deliveries (`POST /webhooks/github`, the synthetic payloads of
//!   `orch-surface-webhook`'s `testdata`) do the same through the same inbox, and only the checks
//!   the gate names (`ci.required`) decide: an unnamed check's `skipped` arriving first, a fork's
//!   green run and a `check_suite` change nothing.
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
use orch_surface_webhook::signature::sign_github;
use orch_surface_webhook::{GenericConfig, GithubConfig, Secrets, generic, github};
use orch_testsupport::fake::{VERIFY_REPOSITORY, verify_commit};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The per-test schema helper of the Postgres store's own tests, shared instead of copied.
#[path = "../../store-postgres/tests/support/mod.rs"]
mod pgdb;

const SECRET: &str = "e2e-webhook-secret-0123456789abcdef0123456789";
const CI_TIMEOUT_SECS: i64 = 3600;
/// 2026-09-30T12:00:00Z.
const T0: i64 = 1_790_769_600;

/// Defines, for each scenario `async fn name<S, W>(rig: Rig<S, W>)`, the tests `name::memory` and
/// `name::postgres` (a no-op unless `ORCH_TEST_DATABASE_URL` is set), each on a gate that names
/// the CI checks after the arrow.
macro_rules! backends {
    ($($name:ident => $required:expr),+ $(,)?) => {
        $(
            mod $name {
                #[tokio::test]
                async fn memory() {
                    super::$name(super::memory_rig($required).await).await;
                }

                #[tokio::test]
                async fn postgres() {
                    match super::postgres_rig($required).await {
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

type Ports<S, W> = PortSet<
    S,
    W,
    ScriptedAgent,
    FixedClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    orch_auth_header::HeaderAuth,
>;

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

/// The gate: CI, and the checks that count. A gate that requires CI names them.
fn gate(required: &[&str]) -> GatePolicy {
    GatePolicy {
        ci: CiPolicy {
            required: required.iter().map(|n| (*n).to_owned()).collect(),
            timeout: SignedDuration::from_secs(CI_TIMEOUT_SECS),
        },
        max_attempts: 3,
        ..GatePolicy::requiring([CheckSource::Ci])
    }
}

async fn rig<S: ThreadStore, W: Wakeup>(
    store: S,
    wakeup: W,
    db: Option<pgdb::TestDb>,
    required: &[&str],
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
                artifacts: orch_ports::NoArtifacts,
                store,
                wakeup,
                agents: ScriptedAgent::new(),
                clock: clock.clone(),
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: orch_auth_header::HeaderAuth::new(),
                registry: directory.fixed_registry(),
            },
            directory,
            AppConfig {
                gate: gate(required),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    let worker = InboxWorker::new(Arc::clone(&app), InboxConfig::default(), "e2e-inbox");
    let router = router_with_surfaces(
        Arc::clone(&app),
        ApiConfig::default(),
        vec![
            generic::routes(
                Arc::clone(&app),
                GenericConfig::new(Secrets::parse(SECRET).unwrap()),
            ),
            github::routes(
                Arc::clone(&app),
                GithubConfig::new(Secrets::parse(SECRET).unwrap()),
            ),
        ],
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

async fn memory_rig(required: &[&str]) -> Rig<MemoryStore, MemoryWakeup> {
    rig(MemoryStore::new(), MemoryWakeup::new(), None, required).await
}

async fn postgres_rig(required: &[&str]) -> Option<Rig<PgStore, PgWakeup>> {
    let db = pgdb::TestDb::new().await?;
    let store = db.store().await; // migrates
    let wakeup = PgWakeup::start(db.pool("e2e-webhook", 4).await);
    assert!(
        wakeup.wait_listening(Duration::from_secs(10)).await,
        "the wakeup listener did not attach"
    );
    Some(rig(store, wakeup, Some(db), required).await)
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
    /// POSTs `body` to the route as a CI system would, signed at the clock's now. Returns the
    /// status. (The same body at the same time is the same delivery.)
    async fn post(&self, body: &Value) -> u16 {
        let body = body.to_string();
        let ts = orch_ports::Clock::now(&self.clock).as_second().to_string();
        let signature = sign_generic(SECRET, &ts, body.as_bytes()).unwrap();
        self.client
            .post(format!("{}/webhooks/ci", self.base))
            .header("Content-Type", "application/json")
            .header("X-Vymalo-Timestamp", ts)
            .header("X-Vymalo-Signature-256", signature)
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }

    /// POSTs a GitHub delivery of `event`, signed as GitHub signs (the raw body). Returns the
    /// status.
    async fn post_github(&self, event: &str, body: &Value) -> u16 {
        let body = body.to_string();
        self.client
            .post(format!("{}/webhooks/github", self.base))
            .header("X-GitHub-Event", event)
            .header("X-GitHub-Delivery", uuid_of(1))
            .header(
                "X-Hub-Signature-256",
                sign_github(SECRET, body.as_bytes()).unwrap(),
            )
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

    async fn row_status_under(&self, source: &str, key: &str) -> Option<InboxStatus> {
        self.app
            .ports()
            .store()
            .find_inbox(source, key)
            .await
            .unwrap()
            .map(|row| row.status)
    }

    /// The row of the generic report `body` posted now: keyed by the digest of what was signed.
    async fn row_status(&self, body: &Value) -> Option<InboxStatus> {
        let ts = orch_ports::Clock::now(&self.clock).as_second();
        let mut hasher = Sha256::new();
        hasher.update(format!("{ts}.").as_bytes());
        hasher.update(body.to_string().as_bytes());
        let key: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.row_status_under(generic::SOURCE, &key).await
    }
}

/// A delivery id: the UUID with `n` in its low bytes.
fn uuid_of(n: u128) -> String {
    uuid::Uuid::from_u128(0x0195_c1a2_7b3e_7c11_8f2a_0000_0000_0000 | n).to_string()
}

/// The synthetic GitHub payload `file` (`orch-surface-webhook/testdata/github`), made about
/// `commit` in the repository the fake agent pushes to.
fn github_payload(file: &str, commit: &str, edit: impl FnOnce(&mut Value)) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../surface-webhook/testdata/github")
        .join(format!("{file}.json"));
    let mut v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    v["repository"]["html_url"] = json!("https://github.com/acme/demo");
    for pointer in [
        "/check_suite/head_sha",
        "/check_run/head_sha",
        "/workflow_run/head_sha",
    ] {
        if let Some(sha) = v.pointer_mut(pointer) {
            *sha = json!(commit);
        }
    }
    edit(&mut v);
    v
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

    let green = report(&commit, "ci/build", "success", Some("212 passed"));
    assert_eq!(rig.post(&green).await, 202);
    assert_eq!(rig.row_status(&green).await, Some(InboxStatus::Pending));
    rig.drain().await;
    assert_eq!(
        rig.row_status(&green).await,
        Some(InboxStatus::Parked),
        "no thread watches the commit yet"
    );
    // A redelivery of a parked report is acknowledged and changes nothing.
    assert_eq!(rig.post(&green).await, 202);
    rig.drain().await;
    assert_eq!(rig.row_status(&green).await, Some(InboxStatus::Parked));

    // The agent pushes (which starts the watch, re-arming the parked report) and finishes (which
    // starts the verification).
    rig.agent(thread.id, pushed(&commit)).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);
    rig.drain().await;

    let record = rig.thread(thread.id).await;
    assert_eq!(record.state, ThreadState::Done);
    assert_eq!(rig.row_status(&green).await, Some(InboxStatus::Applied));
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
        rig.post(&report(&first, "ci/build", "failure", Some(summary)))
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
        rig.post(&report(&first, "ci/build", "failure", None)).await,
        202
    );
    rig.drain().await;
    assert_eq!(
        rig.thread(thread.id).await.state,
        ThreadState::Verifying,
        "a report about the old commit is a card and nothing else"
    );
    assert_eq!(
        rig.post(&report(&second, "ci/build", "success", None))
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
        rig.post(&report(&verify_commit(1), "ci/build", "success", None))
            .await,
        202
    );
    rig.drain().await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Blocked);
    assert_eq!(cards(&rig.events(thread.id).await).len(), 1);
}

/// GitHub's own deliveries through the same path, on a gate that names the check `build`: only
/// that check decides. On the first attempt a fork's green `build`, an unnamed workflow's run, a
/// `check_suite` and a `skipped` lint arrive before the red `build` and change nothing, the red
/// `build` for the pushed commit sends the agent back (the report's summary is in the findings,
/// as text), and on the second attempt a `push` event stores nothing and a green `build` for the
/// new commit ends the job. A `workflow_run` named `CI` is a card and does not count.
async fn github_deliveries_send_the_agent_back_and_end_the_job<S: ThreadStore, W: Wakeup>(
    rig: Rig<S, W>,
) {
    let thread = rig.create().await;
    let first = verify_commit(1);
    let second = verify_commit(2);
    rig.agent(thread.id, pushed(&first)).await;
    rig.agent(thread.id, completed()).await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);

    // What arrives first, and must not decide.
    let fork = github_payload("check_run.completed.fork_pull_request", &first, |v| {
        v["check_run"]["conclusion"] = json!("success");
    });
    assert_eq!(rig.post_github("check_run", &fork).await, 202);
    let fork_run = github_payload("workflow_run.completed.fork", &first, |_| {});
    assert_eq!(rig.post_github("workflow_run", &fork_run).await, 202);
    let unnamed = github_payload("workflow_run.completed.unnamed", &first, |v| {
        v["workflow_run"]["conclusion"] = json!("success");
    });
    assert_eq!(rig.post_github("workflow_run", &unnamed).await, 202);
    let suite = github_payload("check_suite.completed.success", &first, |_| {});
    assert_eq!(rig.post_github("check_suite", &suite).await, 202);
    let lint = github_payload("check_run.completed.success_no_summary", &first, |v| {
        v["check_run"]["conclusion"] = json!("skipped");
        v["check_run"]["id"] = json!(501);
    });
    assert_eq!(rig.post_github("check_run", &lint).await, 202);
    let workflow = github_payload("workflow_run.completed.success", &first, |_| {});
    assert_eq!(rig.post_github("workflow_run", &workflow).await, 202);
    rig.drain().await;
    assert_eq!(
        rig.thread(thread.id).await.state,
        ThreadState::Verifying,
        "nothing that is not the named check decides"
    );
    assert_eq!(
        cards(&rig.events(thread.id).await).len(),
        2,
        "the skipped lint and the CI workflow are cards; the fork, the unnamed run and the suite are not stored"
    );

    let red = github_payload("check_run.completed.failure", &first, |_| {});
    assert_eq!(rig.post_github("check_run", &red).await, 202);
    rig.drain().await;
    let record = rig.thread(thread.id).await;
    assert_eq!((record.state, record.job.attempt), (ThreadState::Queued, 2));
    let events = rig.events(thread.id).await;
    let rework = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Rework(data) => Some(data.clone()),
            _ => None,
        })
        .expect("a rework event");
    assert!(
        rework.findings[0]
            .findings
            .iter()
            .any(|f| f.contains("build") && f.contains("2 tests failed")),
        "{:?}",
        rework.findings
    );
    let card = cards(&events);
    assert_eq!(card[0].provider, orch_core::CiProvider::Github);
    assert_eq!(card[0].repository, "github.com/acme/demo");

    rig.agent(thread.id, pushed(&second)).await;
    rig.agent(thread.id, completed()).await;
    let old = github_payload("check_run.completed.timed_out", &first, |v| {
        v["check_run"]["name"] = json!("build");
        v["check_run"]["id"] = json!(502);
    });
    assert_eq!(rig.post_github("check_run", &old).await, 202);
    let push = json!({"ref": "refs/heads/agent/fix", "after": second});
    assert_eq!(rig.post_github("push", &push).await, 202);
    rig.drain().await;
    assert_eq!(
        rig.thread(thread.id).await.state,
        ThreadState::Verifying,
        "a report about the old commit is a card and nothing else"
    );

    let green = github_payload("check_run.completed.failure", &second, |v| {
        v["check_run"]["conclusion"] = json!("success");
        v["check_run"]["id"] = json!(503);
    });
    assert_eq!(rig.post_github("check_run", &green).await, 202);
    rig.drain().await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Done);
    assert_eq!(
        cards(&rig.events(thread.id).await).len(),
        5,
        "every stored report has a card"
    );
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
        .header("X-Vymalo-Timestamp", ts)
        .header("X-Vymalo-Signature-256", signature)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    assert_eq!(
        rig.row_status(&report(&commit, "ci/build", "success", None))
            .await,
        None
    );
    rig.drain().await;
    assert_eq!(rig.thread(thread.id).await.state, ThreadState::Verifying);
    assert_eq!(rig.events(thread.id).await.len(), before);
}

backends!(
    a_report_received_before_its_watch_is_parked_then_matched_and_the_job_is_done => &["ci/build"],
    a_red_report_reworks_and_the_green_one_for_the_new_commit_finishes => &["ci/build"],
    a_ci_deadline_blocks_the_thread => &["ci/build"],
    github_deliveries_send_the_agent_back_and_end_the_job => &["build"],
    a_refused_delivery_changes_nothing => &["ci/build"],
);
