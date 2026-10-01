//! `wait_for_job` without a network: the wait loop against the in-memory stack, on paused time.
//!
//! Nothing here sleeps to synchronise. The clock is paused, so the timeout, the heartbeat and the
//! event stream's poll only move when a test says (`advance`); the events are applied by hand
//! (no dispatcher), one at a time; and a test waits for the loop to have said something by
//! waiting on the sink, not on the clock.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, AppError, Inbound, NewThread};
use orch_core::{
    AgentId, AgentTarget, AgentTaskState, AgentUpdate, Input, ThreadId, ThreadState, UserId,
};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use orch_surface_mcp::wait::{ProgressSink, WaitEnd, WaitRequest, Waited, wait_for_job};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

type Ports = PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds>;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
/// Longer than any test moves the clock: the stream's own poll never decides anything.
const NO_POLL: Duration = Duration::from_secs(86_400);

/// The notifications a call sent, as `(counter, message)`, with a way to wait for the n-th.
#[derive(Clone, Default)]
struct Recorder {
    sent: Arc<Mutex<Vec<(u64, String)>>>,
    changed: Arc<Notify>,
    /// The counter after which the client is "gone" (`send` answers false).
    hang_up_after: Arc<Mutex<Option<u64>>>,
}

impl ProgressSink for Recorder {
    async fn send(&self, progress: u64, message: String) -> bool {
        self.sent.lock().unwrap().push((progress, message));
        self.changed.notify_waiters();
        !self
            .hang_up_after
            .lock()
            .unwrap()
            .is_some_and(|after| progress >= after)
    }
}

impl Recorder {
    fn all(&self) -> Vec<(u64, String)> {
        self.sent.lock().unwrap().clone()
    }

    /// The `seq` each event notification names (`#7 ...`), heartbeats left out.
    fn seqs(&self) -> Vec<i64> {
        self.all()
            .iter()
            .filter_map(|(_, m)| m.strip_prefix('#'))
            .filter_map(|m| m.split(' ').next()?.parse().ok())
            .collect()
    }

    /// Returns once at least `n` notifications have been sent. Driven by the sink alone.
    async fn until(&self, n: usize) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.sent.lock().unwrap().len() >= n {
                return;
            }
            changed.await;
        }
    }
}

struct World {
    store: MemoryStore,
    wakeup: MemoryWakeup,
    agent: ScriptedAgent,
}

impl World {
    fn new() -> Self {
        World {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agent: ScriptedAgent::new(),
        }
    }

    /// A process over the shared store: two of them are two replicas.
    fn replica(&self) -> Arc<App<Ports>> {
        let entry = AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("plain"),
                "https://plain.example.com/.well-known/agent-card.json".to_owned(),
                None,
            ),
            name: "Plain".to_owned(),
        };
        Arc::new(
            App::new(
                PortSet {
                    store: self.store.clone(),
                    wakeup: self.wakeup.clone(),
                    agents: self.agent.clone(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                },
                AgentDirectory::new(vec![entry]),
                AppConfig {
                    stream_poll: NO_POLL,
                    ..AppConfig::default()
                },
            )
            .expect("a valid gate"),
        )
    }
}

fn alice() -> UserId {
    UserId::new(ALICE)
}

/// A new job of Alice's, with its first message (seq 1). Nothing dispatches it: the tests play
/// the agent themselves.
async fn new_job(app: &App<Ports>) -> ThreadId {
    let id = ThreadId(uuid::Uuid::now_v7());
    app.create_thread_as(
        &alice(),
        id,
        NewThread {
            title: None,
            target: AgentTarget {
                agent_id: AgentId::new("plain"),
                release: None,
            },
            text: "do it".to_owned(),
        },
        Inbound::default(),
    )
    .await
    .unwrap();
    id
}

async fn agent_says(app: &App<Ports>, id: ThreadId, update: AgentUpdate) {
    app.apply(
        id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update,
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
}

async fn status(app: &App<Ports>, id: ThreadId, state: AgentTaskState) {
    agent_says(
        app,
        id,
        AgentUpdate::Status {
            state,
            detail: None,
        },
    )
    .await;
}

async fn artifact(app: &App<Ports>, id: ThreadId, name: &str) {
    agent_says(
        app,
        id,
        AgentUpdate::Artifact {
            name: name.to_owned(),
            mime_type: None,
            uri: None,
            text: None,
        },
    )
    .await;
}

fn request(after_seq: Option<i64>, timeout_secs: u64) -> WaitRequest {
    WaitRequest {
        after_seq,
        timeout: Duration::from_secs(timeout_secs),
        heartbeat: Duration::from_secs(60),
    }
}

type Call = tokio::task::JoinHandle<Result<Waited, AppError>>;

/// Starts a call in the background.
fn spawn_wait(
    app: &Arc<App<Ports>>,
    id: ThreadId,
    request: WaitRequest,
    sink: &Recorder,
    cancel: &CancellationToken,
) -> Call {
    let (app, sink, cancel) = (Arc::clone(app), sink.clone(), cancel.clone());
    tokio::spawn(async move { wait_for_job(&app, &alice(), id, &request, &sink, &cancel).await })
}

async fn finished(call: Call) -> Waited {
    call.await.unwrap().unwrap()
}

#[tokio::test(start_paused = true)]
async fn progress_counts_up_one_per_event_and_the_last_event_returns_the_job() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    let sink = Recorder::default();
    let call = spawn_wait(
        &app,
        id,
        request(Some(1), 300),
        &sink,
        &CancellationToken::new(),
    );

    status(&app, id, AgentTaskState::Working).await; // seq 2
    sink.until(1).await;
    artifact(&app, id, "pull_request").await; // seq 3
    sink.until(2).await;
    assert!(!call.is_finished(), "the job is not over");
    status(&app, id, AgentTaskState::Completed).await; // seq 4 (status) and 5 (done)

    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::Finished);
    assert_eq!(waited.thread.state, ThreadState::Done);
    assert_eq!(waited.resume_after_seq, 5);
    let sent = sink.all();
    assert_eq!(
        sent.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        [1, 2, 3, 4],
        "a counter that only increases, from 1"
    );
    assert_eq!(
        sink.seqs(),
        [2, 3, 4, 5],
        "one notification per event, in order"
    );
    assert_eq!(sent[0].1, "#2 agent working");
    assert_eq!(sent[1].1, "#3 artifact: pull_request");
    assert_eq!(sent[3].1, "#5 job done");
}

#[tokio::test(start_paused = true)]
async fn a_timeout_says_where_to_resume_and_the_next_call_loses_nothing() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    let first = Recorder::default();
    let call = spawn_wait(
        &app,
        id,
        request(Some(1), 300),
        &first,
        &CancellationToken::new(),
    );
    status(&app, id, AgentTaskState::Working).await; // 2
    artifact(&app, id, "branch").await; // 3
    first.until(2).await;

    tokio::time::advance(Duration::from_secs(301)).await;
    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::TimedOut);
    assert_eq!(waited.resume_after_seq, 3);
    assert_eq!(waited.thread.state, ThreadState::Working);

    // Things happen while nobody waits, then the caller comes back with the cursor it was given.
    artifact(&app, id, "pull_request").await; // 4
    status(&app, id, AgentTaskState::Completed).await; // 5, 6
    let second = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(Some(waited.resume_after_seq), 300),
        &second,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(waited.end, WaitEnd::Finished);
    assert_eq!(waited.resume_after_seq, 6);

    // Between the two calls every event was reported once, none twice, none missed.
    let mut seen = first.seqs();
    seen.extend(second.seqs());
    assert_eq!(seen, [2, 3, 4, 5, 6]);
}

#[tokio::test(start_paused = true)]
async fn a_timeout_of_zero_reports_what_the_log_holds_and_returns() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    status(&app, id, AgentTaskState::Working).await; // 2
    let sink = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(Some(0), 0),
        &sink,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(waited.end, WaitEnd::TimedOut);
    assert_eq!(waited.resume_after_seq, 2);
    assert_eq!(sink.seqs(), [1, 2], "the catch-up has no deadline");
}

#[tokio::test(start_paused = true)]
async fn a_heartbeat_goes_out_every_interval_and_shares_the_counter() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    let sink = Recorder::default();
    let call = spawn_wait(
        &app,
        id,
        request(Some(1), 3600),
        &sink,
        &CancellationToken::new(),
    );

    // The cursor is given (1, the first message), so where the call starts does not depend on
    // when the task is first polled.
    tokio::time::advance(Duration::from_secs(60)).await;
    sink.until(1).await;
    status(&app, id, AgentTaskState::Working).await; // seq 2
    sink.until(2).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    sink.until(3).await;

    let sent = sink.all();
    assert_eq!(sent.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(
        sent[0].1.starts_with("still waiting (job queued"),
        "{}",
        sent[0].1
    );
    assert_eq!(sent[1].1, "#2 agent working");
    assert!(
        sent[2]
            .1
            .starts_with("still waiting (job working, last event #2"),
        "{}",
        sent[2].1
    );
    // The state the heartbeat reports is the thread as the last event left it, not as it was when
    // the call began.
    // The timeout ends it.
    tokio::time::advance(Duration::from_secs(3600)).await;
    assert_eq!(finished(call).await.end, WaitEnd::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn a_finished_job_returns_at_once_and_after_zero_replays_it() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    status(&app, id, AgentTaskState::Working).await; // 2
    status(&app, id, AgentTaskState::Completed).await; // 3, 4

    let begun = tokio::time::Instant::now();
    let quiet = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(None, 3600),
        &quiet,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(waited.end, WaitEnd::Finished);
    assert_eq!(waited.resume_after_seq, 4);
    assert!(quiet.all().is_empty(), "nothing happens after the end");
    assert_eq!(
        begun.elapsed(),
        Duration::ZERO,
        "it returned without waiting"
    );

    // From the start, the whole log is reported, then the same end. A cursor past the end is the end.
    let replay = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(Some(0), 3600),
        &replay,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        (waited.end, waited.resume_after_seq),
        (WaitEnd::Finished, 4)
    );
    assert_eq!(replay.seqs(), [1, 2, 3, 4]);
    let beyond = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(Some(9_999), 3600),
        &beyond,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        (waited.end, waited.resume_after_seq),
        (WaitEnd::Finished, 4)
    );
    assert!(beyond.all().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_blocked_job_returns_blocked_and_waiting_again_after_the_answer_waits() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    let sink = Recorder::default();
    let call = spawn_wait(
        &app,
        id,
        request(Some(1), 300),
        &sink,
        &CancellationToken::new(),
    );
    status(&app, id, AgentTaskState::Working).await; // 2
    status(&app, id, AgentTaskState::InputRequired).await; // 3 and 4 (blocked)
    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::Blocked);
    assert_eq!(waited.thread.state, ThreadState::Blocked);
    assert_eq!(waited.resume_after_seq, 4);

    // Asked again while the job still waits for the answer: it says so at once.
    let again = Recorder::default();
    let waited = wait_for_job(
        &app,
        &alice(),
        id,
        &request(None, 300),
        &again,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(waited.end, WaitEnd::Blocked);

    // The answer moves it on, and the next call follows it to the end.
    app.submit(
        &alice(),
        id,
        Input::UserMessage {
            user: alice(),
            text: "main".to_owned(),
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Mcp,
            catalog: None,
        },
        None,
    )
    .await
    .unwrap(); // 5
    let after_answer = Recorder::default();
    let call = spawn_wait(
        &app,
        id,
        request(Some(waited.resume_after_seq), 300),
        &after_answer,
        &CancellationToken::new(),
    );
    after_answer.until(1).await; // the answer itself
    status(&app, id, AgentTaskState::Working).await;
    status(&app, id, AgentTaskState::Completed).await;
    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::Finished);
    assert_eq!(waited.thread.state, ThreadState::Done);
}

#[tokio::test(start_paused = true)]
async fn a_shutdown_ends_the_call_and_another_replica_takes_it_from_there() {
    let world = World::new();
    let dying = world.replica();
    let id = new_job(&dying).await;
    let sink = Recorder::default();
    let call = spawn_wait(
        &dying,
        id,
        request(Some(1), 3600),
        &sink,
        &CancellationToken::new(),
    );
    status(&dying, id, AgentTaskState::Working).await; // 2
    sink.until(1).await;

    // The stream notices at its next poll, once it has caught up: no event needed.
    dying.set_shutting_down();
    tokio::time::advance(NO_POLL + Duration::from_secs(1)).await;
    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::Interrupted);
    assert_eq!(waited.resume_after_seq, 2);

    // The job goes on elsewhere; a call on another replica with the cursor loses nothing.
    let survivor = world.replica();
    artifact(&survivor, id, "pull_request").await; // 3
    status(&survivor, id, AgentTaskState::Completed).await; // 4, 5
    let next = Recorder::default();
    let waited = wait_for_job(
        &survivor,
        &alice(),
        id,
        &request(Some(waited.resume_after_seq), 3600),
        &next,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(waited.end, WaitEnd::Finished);
    let mut seen = sink.seqs();
    seen.extend(next.seqs());
    assert_eq!(seen, [2, 3, 4, 5]);
}

#[tokio::test(start_paused = true)]
async fn a_client_that_goes_away_ends_the_call() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;

    // Its connection is dropped: the request's cancellation token fires.
    let sink = Recorder::default();
    let cancel = CancellationToken::new();
    let call = spawn_wait(&app, id, request(Some(1), 3600), &sink, &cancel);
    status(&app, id, AgentTaskState::Working).await;
    sink.until(1).await;
    cancel.cancel();
    let waited = finished(call).await;
    assert_eq!(
        (waited.end, waited.resume_after_seq),
        (WaitEnd::Interrupted, 2)
    );

    // Its notifications fail: the transport is closed.
    let closed = Recorder::default();
    *closed.hang_up_after.lock().unwrap() = Some(1);
    let call = spawn_wait(
        &app,
        id,
        request(Some(0), 3600),
        &closed,
        &CancellationToken::new(),
    );
    let waited = finished(call).await;
    assert_eq!(waited.end, WaitEnd::Interrupted);
    assert_eq!(closed.all().len(), 1, "it stopped at the first failure");
}

#[tokio::test(start_paused = true)]
async fn another_users_job_and_an_unknown_one_are_not_found() {
    let world = World::new();
    let app = world.replica();
    let id = new_job(&app).await;
    let sink = Recorder::default();
    for (user, job) in [
        (UserId::new(BOB), id),
        (alice(), ThreadId(uuid::Uuid::now_v7())),
    ] {
        let err = wait_for_job(
            &app,
            &user,
            job,
            &request(Some(0), 60),
            &sink,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::NotFound), "{err}");
    }
    assert!(sink.all().is_empty(), "nothing of the job leaked");
}
