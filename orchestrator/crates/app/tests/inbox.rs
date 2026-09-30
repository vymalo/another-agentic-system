//! The inbox through the application: watches and timers are written by the commits that ask
//! for them, and the inbox worker applies what comes due. Time is a `FixedClock` the test moves
//! and the worker is driven one `tick` at a time, so nothing here waits or races.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_app::{
    App, AppConfig, AppError, ApplyOutcome, InboxConfig, InboxWorker, NewThread, Received,
};
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, CheckSource, CiConclusion, CiPolicy, CiProvider,
    CiReport, EventBody, EventKind, GatePolicy, Hold, Input, ThreadId, ThreadRecord, ThreadState,
    Timer, WatchKey,
};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    InboxPayload, InboxStatus, NewTimer, PortSet, StoreError, TIMER_SOURCE, ThreadStore,
};
use serde_json::json;
use support::{alice, directory, eventually, target};
use tokio_util::sync::CancellationToken;

const SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";
const REPO: &str = "github.com/o/r";
const CI_TIMEOUT_SECS: i64 = 60;

type Ports = PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds>;
type TestApp = App<Ports>;

fn t0() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

fn app_config() -> AppConfig {
    AppConfig {
        gate: GatePolicy {
            ci: CiPolicy {
                timeout: SignedDuration::from_secs(CI_TIMEOUT_SECS),
                ..CiPolicy::default()
            },
            ..GatePolicy::requiring([CheckSource::Ci])
        },
        ..AppConfig::default()
    }
}

/// One "database" and one clock, shared by every app and worker of a test.
struct Rig {
    store: MemoryStore,
    clock: FixedClock,
    app: Arc<TestApp>,
}

impl Rig {
    fn new() -> Self {
        let store = MemoryStore::new();
        let clock = FixedClock::new(t0());
        let app = Arc::new(
            App::new(
                PortSet {
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: clock.clone(),
                    ids: SeqIds::default(),
                },
                directory(),
                app_config(),
            )
            .expect("a valid gate"),
        );
        Rig { store, clock, app }
    }

    fn worker(&self, owner: &str, cfg: InboxConfig) -> Arc<InboxWorker<Ports>> {
        InboxWorker::new(Arc::clone(&self.app), cfg, owner)
    }

    async fn create(&self) -> ThreadRecord {
        self.app
            .create_thread(
                &alice(),
                NewThread {
                    title: None,
                    target: target("plain"),
                    text: "ship it".to_owned(),
                },
            )
            .await
            .unwrap()
    }

    /// The agent pushes `SHA` (which starts the watch) and says it is done (which starts the
    /// verification and its deadline).
    async fn push_and_complete(&self, id: ThreadId) {
        self.agent(id, branch()).await;
        self.agent(id, completed()).await;
    }

    async fn agent(&self, id: ThreadId, input: Input) {
        let outcome = self.app.apply(id, input, None, None, None).await.unwrap();
        assert!(
            matches!(outcome, ApplyOutcome::Applied { .. }),
            "{outcome:?}"
        );
    }

    async fn thread(&self, id: ThreadId) -> ThreadRecord {
        self.app.get_thread(&alice(), id).await.unwrap()
    }

    async fn ci_cards(&self, id: ThreadId) -> usize {
        self.app
            .list_events(&alice(), id, 0, 500)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind() == EventKind::CiResult)
            .count()
    }

    fn now(&self) -> Timestamp {
        orch_ports::Clock::now(&self.clock)
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
    }
}

fn from_agent(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

fn branch() -> Input {
    from_agent(AgentUpdate::Artifact {
        name: "branch".to_owned(),
        mime_type: None,
        uri: None,
        text: Some(
            json!({"repository": "https://github.com/o/r.git", "branch": "agent/x", "commit": SHA})
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

fn report(conclusion: CiConclusion) -> CiReport {
    CiReport {
        provider: CiProvider::Github,
        repository: REPO.to_owned(),
        sha: SHA.to_owned(),
        branch: Some("agent/x".to_owned()),
        name: "build".to_owned(),
        conclusion,
        url: None,
        summary: None,
    }
}

fn watch_key() -> String {
    WatchKey::ci(REPO, SHA).to_string()
}

fn timer_key(id: ThreadId, verification: u32) -> String {
    NewTimer {
        id: orch_ports::InboxId(uuid::Uuid::nil()),
        after: SignedDuration::ZERO,
        timer: Timer::CiDeadline {
            attempt: 1,
            verification,
        },
    }
    .idempotency_key(id)
}

fn cfg() -> InboxConfig {
    InboxConfig {
        lease: Duration::from_secs(30),
        backoff_base: Duration::from_secs(5),
        ..InboxConfig::default()
    }
}

#[tokio::test]
async fn a_gated_completion_writes_its_watch_and_its_timer_in_its_own_commit() {
    let rig = Rig::new();
    let t = rig.create().await;
    // Nothing is watched or scheduled before the agent pushes and finishes.
    assert_eq!(rig.store.get_watch(&watch_key()).await.unwrap(), None);

    rig.agent(t.id, branch()).await;
    assert_eq!(
        rig.store.get_watch(&watch_key()).await.unwrap(),
        Some(t.id),
        "the push starts the watch"
    );
    rig.push_and_complete(t.id).await;
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Verifying);

    let row = rig
        .store
        .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
        .await
        .unwrap()
        .expect("the deadline is an inbox row");
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!(
        row.available_at,
        t0().checked_add(SignedDuration::from_secs(CI_TIMEOUT_SECS))
            .unwrap(),
        "the store adds the delay to the commit's time; the core never read a clock"
    );
    assert_eq!(
        row.decode().unwrap(),
        InboxPayload::Timer {
            thread: t.id,
            timer: Timer::CiDeadline {
                attempt: 1,
                verification: 1
            }
        }
    );
}

#[tokio::test]
async fn a_timer_fires_only_when_due_and_blocks_the_thread_once() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;

    assert_eq!(worker.tick().await, 0, "nothing is due");
    rig.advance(u64::try_from(CI_TIMEOUT_SECS).unwrap() - 1);
    assert_eq!(worker.tick().await, 0, "one second early");
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Verifying);

    rig.advance(1);
    assert_eq!(worker.tick().await, 1);
    let blocked = rig.thread(t.id).await;
    assert_eq!(blocked.state, ThreadState::Blocked);
    assert_eq!(blocked.job.hold, Some(Hold::CiTimeout));
    assert_eq!(blocked.job.attempt, 1, "a timeout spends no attempt");
    let events = rig.app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    assert!(events.iter().any(|e| matches!(
        &e.body,
        EventBody::Error(d) if d.message.contains("CI did not report")
    )));

    let row = rig
        .store
        .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status,
        InboxStatus::Applied,
        "marked by the thread's own commit"
    );
    assert!(row.lease().is_none());
    assert_eq!(worker.tick().await, 0, "applied once");
    assert_eq!(rig.thread(t.id).await.version, blocked.version);
}

#[tokio::test]
async fn a_timer_of_a_finished_verification_is_applied_and_changes_nothing() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    // CI answers in time: the thread is done long before the deadline.
    let stored = rig
        .app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap();
    assert!(matches!(stored, Received::Stored { .. }));
    assert_eq!(worker.tick().await, 1);
    let done = rig.thread(t.id).await;
    assert_eq!(done.state, ThreadState::Done);

    rig.advance(3600);
    assert_eq!(
        worker.tick().await,
        1,
        "the deadline is claimed all the same"
    );
    let after = rig.thread(t.id).await;
    assert_eq!(
        (after.state, after.version),
        (ThreadState::Done, done.version)
    );
    let row = rig
        .store
        .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InboxStatus::Applied);
}

#[tokio::test]
async fn a_report_that_beats_its_watch_is_parked_and_applied_after_a_later_commit() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;

    // CI is faster than the agent's `branch` artifact: no watch exists yet.
    let received = rig
        .app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap();
    let Received::Stored { id } = received else {
        panic!("{received:?}");
    };
    assert_eq!(worker.tick().await, 1);
    let parked = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(parked.status, InboxStatus::Parked);
    assert_eq!(parked.correlation.as_deref(), Some(watch_key().as_str()));
    assert_eq!(worker.tick().await, 0, "a parked row is left alone");
    assert_eq!(rig.ci_cards(t.id).await, 0);

    // The agent pushes: the commit that carries the watch re-arms the row.
    rig.agent(t.id, branch()).await;
    let rearmed = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(rearmed.status, InboxStatus::Pending);
    rig.push_and_complete(t.id).await;
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Verifying);

    assert_eq!(worker.tick().await, 1);
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
    assert_eq!(rig.ci_cards(t.id).await, 1);
    assert_eq!(
        rig.store.get_inbox(id).await.unwrap().unwrap().status,
        InboxStatus::Applied
    );

    // The sender delivers it again: a duplicate, stored nowhere, applied never.
    assert_eq!(
        rig.app
            .receive(
                "github",
                "delivery-1",
                InboxPayload::CiReport(report(CiConclusion::Success))
            )
            .await
            .unwrap(),
        Received::Duplicate
    );
    assert_eq!(worker.tick().await, 0, "nothing is left to claim");
    assert_eq!(rig.ci_cards(t.id).await, 1);
}

#[tokio::test]
async fn parked_reports_expire_after_the_time_to_live() {
    let rig = Rig::new();
    let worker = rig.worker(
        "w1",
        InboxConfig {
            parked_ttl: Duration::from_secs(600),
            ..cfg()
        },
    );
    let t = rig.create().await;
    let Received::Stored { id } = rig
        .app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap()
    else {
        panic!("stored");
    };
    assert_eq!(worker.tick().await, 1);
    rig.advance(599);
    worker.tick().await;
    assert_eq!(
        rig.store.get_inbox(id).await.unwrap().unwrap().status,
        InboxStatus::Parked
    );
    rig.advance(1);
    worker.tick().await;
    assert_eq!(
        rig.store.get_inbox(id).await.unwrap().unwrap().status,
        InboxStatus::Expired
    );

    // The watch that comes too late finds nothing to re-arm.
    rig.agent(t.id, branch()).await;
    assert_eq!(
        rig.store.get_inbox(id).await.unwrap().unwrap().status,
        InboxStatus::Expired
    );
}

#[tokio::test]
async fn a_row_a_dead_worker_held_is_applied_once_by_another_and_the_late_one_is_fenced() {
    let rig = Rig::new();
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    let Received::Stored { id } = rig
        .app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap()
    else {
        panic!("stored");
    };

    // Instance 1 claims the row and dies before it commits.
    let claimed = rig
        .store
        .claim_inbox("instance-1", rig.now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let stale = claimed[0].lease().unwrap();

    let second = rig.worker("instance-2", cfg());
    assert_eq!(second.tick().await, 0, "the lease is still valid");
    rig.advance(31);
    assert_eq!(second.tick().await, 1, "the lapsed claim is taken over");
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
    assert_eq!(rig.ci_cards(t.id).await, 1);
    let row = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!((row.status, row.attempts), (InboxStatus::Applied, 2));

    // Instance 1 wakes up and finishes what it was doing: refused, and nothing changes.
    let version = rig.thread(t.id).await.version;
    let late = rig
        .app
        .apply_from_inbox(
            t.id,
            Input::CiReported(report(CiConclusion::Failure)),
            &stale,
        )
        .await
        .unwrap();
    assert_eq!(late, ApplyOutcome::Fenced);
    assert_eq!(rig.thread(t.id).await.version, version);
    assert_eq!(rig.ci_cards(t.id).await, 1);
}

#[tokio::test]
async fn a_retryable_failure_backs_off_and_the_row_dies_after_its_attempts() {
    let rig = Rig::new();
    let worker = rig.worker(
        "w1",
        InboxConfig {
            max_attempts: 3,
            ..cfg()
        },
    );
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    let Received::Stored { id } = rig
        .app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap()
    else {
        panic!("stored");
    };
    rig.store.fail_next_commits(3, || {
        StoreError::unavailable(std::io::Error::other("db down"))
    });

    assert_eq!(worker.tick().await, 1);
    let first = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(first.status, InboxStatus::Pending);
    assert!(first.last_error.as_deref().unwrap().contains("db down"));
    assert_eq!(
        first.available_at,
        t0().checked_add(SignedDuration::from_secs(5)).unwrap(),
        "the first retry waits the base delay"
    );
    assert_eq!(worker.tick().await, 0, "not before the backoff is over");
    rig.advance(5);
    assert_eq!(worker.tick().await, 1);
    assert_eq!(
        rig.store.get_inbox(id).await.unwrap().unwrap().available_at,
        t0().checked_add(SignedDuration::from_secs(5 + 10)).unwrap(),
        "the delay doubles"
    );
    rig.advance(10);
    assert_eq!(worker.tick().await, 1);
    let dead = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!((dead.status, dead.attempts), (InboxStatus::Dead, 3));
    assert!(
        dead.last_error
            .as_deref()
            .unwrap()
            .contains("gave up after 3 attempts")
    );
    assert_eq!(
        rig.thread(t.id).await.state,
        ThreadState::Verifying,
        "never applied"
    );

    rig.advance(100_000);
    assert_eq!(worker.tick().await, 1, "only the deadline");
}

#[tokio::test]
async fn a_report_for_a_thread_that_cannot_take_it_is_given_up_on_at_once() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    // A timer whose thread does not exist, and a live thread beside it.
    let ghost = ThreadId(uuid::Uuid::from_u128(0xdead));
    let t = rig.create().await;
    let row = orch_ports::NewInbox {
        id: orch_ports::InboxId(uuid::Uuid::from_u128(0xf00d)),
        source: TIMER_SOURCE.to_owned(),
        idempotency_key: "ghost".to_owned(),
        payload: InboxPayload::Timer {
            thread: ghost,
            timer: Timer::CiDeadline {
                attempt: 1,
                verification: 1,
            },
        },
        correlation: None,
    };
    rig.store.receive(row.clone(), t0()).await.unwrap();
    assert_eq!(worker.tick().await, 1);
    let dead = rig.store.get_inbox(row.id).await.unwrap().unwrap();
    assert_eq!(dead.status, InboxStatus::Dead);
    assert!(dead.last_error.unwrap().contains("not found"));
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Queued);
}

#[tokio::test]
async fn receive_refuses_what_a_surface_may_not_send() {
    let rig = Rig::new();
    let ci = || InboxPayload::CiReport(report(CiConclusion::Success));
    let invalid = |r: Result<Received, AppError>| matches!(r, Err(AppError::Invalid(_)));

    // Only the store arms timers: a surface can neither send one nor use their source.
    let timer = InboxPayload::Timer {
        thread: ThreadId(uuid::Uuid::from_u128(1)),
        timer: Timer::CiDeadline {
            attempt: 1,
            verification: 1,
        },
    };
    assert!(invalid(rig.app.receive("github", "k", timer).await));
    assert!(invalid(rig.app.receive(TIMER_SOURCE, "k", ci()).await));
    assert!(invalid(rig.app.receive("", "k", ci()).await));
    assert!(invalid(rig.app.receive("github", "  ", ci()).await));
    assert!(invalid(
        rig.app.receive("github", &"k".repeat(257), ci()).await
    ));
    let bloated = InboxPayload::CiReport(CiReport {
        summary: Some("x".repeat(64 * 1024)),
        ..report(CiConclusion::Success)
    });
    assert!(invalid(rig.app.receive("github", "k", bloated).await));

    // What reaches the logs and spans is printable ASCII: no control character, no line break,
    // no escape sequence, nothing outside ASCII.
    for bad in [
        "a\nb",
        "a\rb",
        "a\tb",
        "a\u{1b}[31mb",
        "a\0b",
        "caf\u{e9}",
        "a\u{202e}b",
    ] {
        assert!(
            invalid(rig.app.receive(bad, "k", ci()).await),
            "source {bad:?}"
        );
        assert!(
            invalid(rig.app.receive("github", bad, ci()).await),
            "key {bad:?}"
        );
    }
    assert!(matches!(
        rig.app
            .receive("my-ci.v2", "run 42/attempt~1: #7", ci())
            .await
            .unwrap(),
        Received::Stored { .. }
    ));

    // The correlation is the watch key of the report, whatever the sender would like: it cannot
    // be given at all.
    let Received::Stored { id } = rig.app.receive("github", "k1", ci()).await.unwrap() else {
        panic!("stored");
    };
    let row = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(row.correlation.as_deref(), Some(watch_key().as_str()));
    assert_eq!(
        (row.status, row.available_at, row.kind.as_str()),
        (InboxStatus::Pending, t0(), "ci_report")
    );
    assert_eq!(
        rig.app.receive("github", "k1", ci()).await.unwrap(),
        Received::Duplicate
    );
    assert!(matches!(
        rig.app.receive("generic", "k1", ci()).await.unwrap(),
        Received::Stored { .. }
    ));
}

#[tokio::test]
async fn a_running_worker_applies_a_received_report_at_once_and_stops_on_shutdown() {
    let rig = Rig::new();
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    let worker = rig.worker(
        "w1",
        InboxConfig {
            // The safety poll is far away, so a prompt answer is the wakeup's (or the first
            // pass's, if the report was stored before the worker began).
            poll_interval: Duration::from_secs(3600),
            ..cfg()
        },
    );
    let stop = CancellationToken::new();
    let running = tokio::spawn(Arc::clone(&worker).run(stop.clone()));
    rig.app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap();
    eventually("the report is applied", || async {
        (rig.thread(t.id).await.state == ThreadState::Done).then_some(())
    })
    .await;

    stop.cancel();
    running.await.unwrap();
}

#[tokio::test]
async fn shutdown_releases_the_claims_so_another_replica_takes_the_rows_at_once() {
    let rig = Rig::new();
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    rig.app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap();
    let held = rig
        .store
        .claim_inbox("w1", rig.now(), Duration::from_secs(30), 10)
        .await
        .unwrap();
    assert_eq!(held.len(), 1);
    assert!(
        rig.store
            .claim_inbox("w2", rig.now(), Duration::from_secs(30), 10)
            .await
            .unwrap()
            .is_empty()
    );

    // w1 is told to stop before it does anything.
    let stop = CancellationToken::new();
    stop.cancel();
    rig.worker("w1", cfg()).run(stop).await;

    let w2 = rig.worker("w2", cfg());
    assert_eq!(w2.tick().await, 1, "no need to wait out the 30 s lease");
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
}

/// A report as a webhook would deliver it, for `delivery`.
async fn receive_success(rig: &Rig, delivery: &str) -> orch_ports::InboxId {
    let received = rig
        .app
        .receive(
            "github",
            delivery,
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap();
    let Received::Stored { id } = received else {
        panic!("{received:?}");
    };
    id
}

#[tokio::test]
async fn a_row_whose_workers_keep_dying_is_dead_lettered_without_being_delivered() {
    let rig = Rig::new();
    let max = 3;
    let worker = rig.worker(
        "survivor",
        InboxConfig {
            max_attempts: max,
            ..cfg()
        },
    );
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    let id = receive_success(&rig, "delivery-1").await;

    // Workers claim the row and die with it in hand: no error path ran, the lease just lapses.
    // (Short leases keep the CI deadline, 60 s away, out of it.)
    for n in 1..=max {
        let claimed = rig
            .store
            .claim_inbox(
                &format!("doomed-{n}"),
                rig.now(),
                Duration::from_secs(5),
                10,
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1, "claim {n}");
        assert_eq!(claimed[0].attempts, n);
        rig.advance(6);
    }

    // The next claim is one more than the limit allows: the row is given up on, not delivered.
    assert_eq!(worker.tick().await, 1);
    let dead = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!((dead.status, dead.attempts), (InboxStatus::Dead, max + 1));
    let error = dead.last_error.unwrap();
    assert!(
        error.contains("3 earlier claims ended without an answer"),
        "{error}"
    );
    assert_eq!(
        rig.thread(t.id).await.state,
        ThreadState::Verifying,
        "the report was not applied"
    );
    assert_eq!(rig.ci_cards(t.id).await, 0);
    assert_eq!(worker.tick().await, 0, "a dead row is never claimed again");
}

#[tokio::test]
async fn a_row_claimed_exactly_as_often_as_allowed_is_still_delivered() {
    let rig = Rig::new();
    let max = 3;
    let worker = rig.worker(
        "survivor",
        InboxConfig {
            max_attempts: max,
            ..cfg()
        },
    );
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    receive_success(&rig, "delivery-1").await;
    for n in 1..max {
        rig.store
            .claim_inbox(
                &format!("doomed-{n}"),
                rig.now(),
                Duration::from_secs(5),
                10,
            )
            .await
            .unwrap();
        rig.advance(6);
    }
    assert_eq!(worker.tick().await, 1, "the last claim it may have");
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
}

#[tokio::test]
async fn claims_handed_back_at_shutdown_do_not_use_up_the_attempts() {
    let rig = Rig::new();
    let max = 2;
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    let id = receive_success(&rig, "delivery-1").await;

    // Deploys roll: each replica claims the row and is told to stop before it gets to it.
    // Far more of these than `max_attempts` allows.
    for n in 0..5 {
        let owner = format!("replica-{n}");
        let held = rig
            .store
            .claim_inbox(&owner, rig.now(), Duration::from_secs(30), 10)
            .await
            .unwrap();
        assert_eq!(held.len(), 1, "cycle {n}");
        let stop = CancellationToken::new();
        stop.cancel();
        rig.worker(
            &owner,
            InboxConfig {
                max_attempts: max,
                ..cfg()
            },
        )
        .run(stop)
        .await;
    }
    let worker = rig.worker(
        "last",
        InboxConfig {
            max_attempts: max,
            ..cfg()
        },
    );
    assert_eq!(worker.tick().await, 1);
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
    let row = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!((row.status, row.attempts), (InboxStatus::Applied, 6));
    assert_eq!(row.counted_attempts(), 1);
}

#[tokio::test]
async fn a_report_is_matched_however_its_repository_and_sha_are_spelled() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;

    // A provider that writes the URL and the hash in its own way: the watch is
    // `ci:github.com/o/r@cccc...`.
    let spelled = CiReport {
        repository: "https://github.com/O/R.git".to_owned(),
        sha: SHA.to_uppercase(),
        ..report(CiConclusion::Success)
    };
    let Received::Stored { id } = rig
        .app
        .receive("github", "delivery-1", InboxPayload::CiReport(spelled))
        .await
        .unwrap()
    else {
        panic!("stored");
    };
    let row = rig.store.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(row.correlation.as_deref(), Some(watch_key().as_str()));
    let InboxPayload::CiReport(stored) = row.decode().unwrap() else {
        panic!("a report");
    };
    assert_eq!(
        (stored.repository.as_str(), stored.sha.as_str()),
        (REPO, SHA)
    );

    assert_eq!(worker.tick().await, 1);
    assert_eq!(rig.thread(t.id).await.state, ThreadState::Done);
    assert_eq!(rig.ci_cards(t.id).await, 1);
}

#[tokio::test]
async fn a_report_that_can_never_match_a_watch_is_refused_not_parked() {
    let rig = Rig::new();
    let refused = |repository: &str, sha: &str| {
        let report = CiReport {
            repository: repository.to_owned(),
            sha: sha.to_owned(),
            ..report(CiConclusion::Success)
        };
        let app = Arc::clone(&rig.app);
        async move {
            app.receive("github", "delivery", InboxPayload::CiReport(report))
                .await
        }
    };
    for (repository, sha) in [
        ("", SHA),
        ("not a repository", SHA),
        ("github.com/o/../r", SHA),
        (REPO, ""),
        (REPO, "abc123"),
        (REPO, "main"),
        (REPO, &"g".repeat(40)),
        (REPO, &SHA[..39]),
    ] {
        assert!(
            matches!(refused(repository, sha).await, Err(AppError::Invalid(_))),
            "{repository:?} @ {sha:?}"
        );
    }
    assert!(
        rig.store
            .find_inbox("github", "delivery")
            .await
            .unwrap()
            .is_none(),
        "nothing was stored"
    );
}

#[tokio::test]
async fn an_input_that_changes_nothing_finishes_its_row_in_a_commit_that_checks_the_thread() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    receive_success(&rig, "delivery-1").await;
    assert_eq!(worker.tick().await, 1);
    let done = rig.thread(t.id).await;
    assert_eq!(done.state, ThreadState::Done);

    // The deadline of the verification comes due on a finished thread: nothing to write. The
    // store is down for the one commit that finishes the row; that commit is what the row
    // waits for, so it is retried rather than marked applied on a decision nobody checked.
    rig.advance(3600);
    rig.store.fail_next_commits(1, || {
        StoreError::unavailable(std::io::Error::other("db down"))
    });
    assert_eq!(worker.tick().await, 1);
    let deadline = rig
        .store
        .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deadline.status, InboxStatus::Pending);
    assert!(deadline.last_error.as_deref().unwrap().contains("db down"));

    rig.advance(5);
    assert_eq!(worker.tick().await, 1);
    let deadline = rig
        .store
        .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deadline.status, InboxStatus::Applied);
    let after = rig.thread(t.id).await;
    assert_eq!(
        (after.state, after.version, after.updated_at),
        (done.state, done.version, done.updated_at),
        "the thread was left alone"
    );
}

#[tokio::test]
async fn a_no_op_decided_on_a_thread_that_has_moved_is_decided_again() {
    let rig = Rig::new();
    let worker = rig.worker("w1", cfg());
    let t = rig.create().await;
    rig.push_and_complete(t.id).await;
    receive_success(&rig, "delivery-1").await;
    worker.tick().await;
    rig.advance(3600);
    // The finishing commit is refused once as if another writer had got in first; the input is
    // decided again against the thread as it is now, and the row is finished.
    rig.store
        .fail_next_commits(2, || StoreError::VersionConflict);
    assert_eq!(worker.tick().await, 1);
    assert_eq!(
        rig.store
            .find_inbox(TIMER_SOURCE, &timer_key(t.id, 1))
            .await
            .unwrap()
            .unwrap()
            .status,
        InboxStatus::Applied
    );
}

/// A store that runs a hook between the worker's look at the watches and its park, which is the
/// window a commit adding the watch can land in.
#[derive(Clone)]
struct RacyStore {
    inner: MemoryStore,
    hook: Arc<std::sync::Mutex<Option<futures::future::BoxFuture<'static, ()>>>>,
}

macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => {
        $(
            async fn $name(&self, $($arg: $ty),*) -> $ret {
                self.inner.$name($($arg),*).await
            }
        )*
    };
}

impl ThreadStore for RacyStore {
    async fn get_watch(&self, key: &str) -> Result<Option<ThreadId>, StoreError> {
        let found = self.inner.get_watch(key).await?;
        if found.is_none() {
            let hook = self.hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook.await;
            }
        }
        Ok(found)
    }

    delegate! {
        ping() -> Result<(), StoreError>;
        create_thread(new: orch_ports::NewThreadRecord, first: orch_ports::Commit)
            -> Result<(ThreadRecord, Vec<orch_core::Event>), StoreError>;
        get_thread(owner: Option<&orch_core::UserId>, id: ThreadId)
            -> Result<Option<ThreadRecord>, StoreError>;
        list_threads(owner: &orch_core::UserId, before: Option<ThreadId>, limit: u32)
            -> Result<Vec<ThreadRecord>, StoreError>;
        commit(thread: ThreadId, expected_version: i64, commit: orch_ports::Commit)
            -> Result<orch_ports::CommitOutcome, StoreError>;
        list_events(thread: ThreadId, after: i64, limit: u32)
            -> Result<Vec<orch_core::Event>, StoreError>;
        latest_events(thread: ThreadId, kind: orch_core::EventKind, limit: u32)
            -> Result<Vec<orch_core::Event>, StoreError>;
        get_binding(thread: ThreadId) -> Result<Option<orch_ports::AgentBinding>, StoreError>;
        claim_outbox(owner: &str, now: Timestamp, lease: Duration, limit: u32)
            -> Result<Vec<orch_ports::OutboxItem>, StoreError>;
        renew_lease(lease: &orch_ports::Lease, until: Timestamp) -> Result<bool, StoreError>;
        mark_sent(lease: &orch_ports::Lease, binding: orch_ports::BindingUpdate, now: Timestamp)
            -> Result<bool, StoreError>;
        mark_verify_sent(lease: &orch_ports::Lease, task_id: String, now: Timestamp)
            -> Result<bool, StoreError>;
        retry_outbox(lease: &orch_ports::Lease, next_attempt_at: Timestamp, error: String)
            -> Result<bool, StoreError>;
        complete_outbox(lease: &orch_ports::Lease, outcome: orch_ports::OutboxFinal, now: Timestamp)
            -> Result<bool, StoreError>;
        skip_unsent_delegates(thread: ThreadId, now: Timestamp) -> Result<u32, StoreError>;
        release_leases(owner: &str, now: Timestamp) -> Result<u32, StoreError>;
        outbox_stats(now: Timestamp) -> Result<orch_ports::OutboxStats, StoreError>;
        get_outbox(id: orch_ports::OutboxId) -> Result<Option<orch_ports::OutboxItem>, StoreError>;
        list_open_outbox(thread: ThreadId) -> Result<Vec<orch_ports::OutboxItem>, StoreError>;
        receive(row: orch_ports::NewInbox, now: Timestamp) -> Result<Received, StoreError>;
        claim_inbox(owner: &str, now: Timestamp, lease: Duration, limit: u32)
            -> Result<Vec<orch_ports::InboxItem>, StoreError>;
        park_inbox(lease: &orch_ports::InboxLease, now: Timestamp)
            -> Result<orch_ports::Parking, StoreError>;
        retry_inbox(lease: &orch_ports::InboxLease, available_at: Timestamp, error: String)
            -> Result<bool, StoreError>;
        complete_inbox(lease: &orch_ports::InboxLease, outcome: orch_ports::InboxFinal, now: Timestamp)
            -> Result<bool, StoreError>;
        expire_parked_inbox(parked_at_or_before: Timestamp, now: Timestamp)
            -> Result<u32, StoreError>;
        release_inbox_leases(owner: &str, now: Timestamp) -> Result<u32, StoreError>;
        get_inbox(id: orch_ports::InboxId) -> Result<Option<orch_ports::InboxItem>, StoreError>;
        find_inbox(source: &str, idempotency_key: &str)
            -> Result<Option<orch_ports::InboxItem>, StoreError>;
    }
}

/// The worker looks for the watch and finds none; before it parks the row, the agent pushes and
/// the commit that carries the watch lands. The row had not been parked yet, so that commit had
/// nothing to re-arm: the park itself must see the watch and put the row back.
#[tokio::test]
async fn a_watch_that_lands_between_the_lookup_and_the_park_is_not_missed() {
    let hook = Arc::default();
    let store = RacyStore {
        inner: MemoryStore::new(),
        hook: Arc::clone(&hook),
    };
    let clock = FixedClock::new(t0());
    let app = Arc::new(
        App::new(
            PortSet {
                store: store.clone(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: clock.clone(),
                ids: SeqIds::default(),
            },
            directory(),
            app_config(),
        )
        .expect("a valid gate"),
    );
    let worker = InboxWorker::new(Arc::clone(&app), cfg(), "w1");
    let t = app
        .create_thread(
            &alice(),
            NewThread {
                title: None,
                target: target("plain"),
                text: "ship it".to_owned(),
            },
        )
        .await
        .unwrap();
    let Received::Stored { id } = app
        .receive(
            "github",
            "delivery-1",
            InboxPayload::CiReport(report(CiConclusion::Success)),
        )
        .await
        .unwrap()
    else {
        panic!("stored");
    };
    let racing = Arc::clone(&app);
    let thread = t.id;
    *hook.lock().unwrap() = Some(Box::pin(async move {
        racing
            .apply(thread, branch(), None, None, None)
            .await
            .unwrap();
        racing
            .apply(thread, completed(), None, None, None)
            .await
            .unwrap();
    }));

    assert_eq!(worker.tick().await, 1);
    assert!(
        hook.lock().unwrap().is_none(),
        "the commit ran in the window"
    );
    let row = store.inner.get_inbox(id).await.unwrap().unwrap();
    assert_eq!(
        row.status,
        InboxStatus::Pending,
        "put back by the park, not stranded behind its watch"
    );
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Verifying
    );

    assert_eq!(worker.tick().await, 1);
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().state,
        ThreadState::Done
    );
    assert_eq!(
        store.inner.get_inbox(id).await.unwrap().unwrap().status,
        InboxStatus::Applied
    );
}
