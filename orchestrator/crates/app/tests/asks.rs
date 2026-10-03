//! Asked agents in the job ledger (ADR 0026) through the application: the commit that logs an ask
//! also writes its outbox row and arms its deadline, a user cannot submit an ask or the end of one,
//! the dispatcher keeps the `ask` rows it cannot send yet, and the deadline ends the ask through
//! the inbox worker. Nothing sends an ask to an agent in this build (the dispatcher's ask path is
//! the next change), so the scripted agent must see no request for one.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_app::{App, AppConfig, AppError, ApplyOutcome, Creation, Inbound, InboxConfig, NewThread};
use orch_core::{
    Actor, AgentId, AgentTaskState, AgentUpdate, AskLimits, AskOutcome, AskRefusal, Caller,
    Classify, ErrorClass, EventBody, EventKind, Input, Mention, ThreadId, ThreadRecord,
    ThreadState, Timer, TransitionError,
};
use orch_ports::memory::{FixedClock, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    InboxPayload, InboxStatus, NewTimer, OutboxKind, OutboxPayload, OutboxStatus, PortSet,
    TIMER_SOURCE, ThreadStore,
};
use support::*;

/// `ask @coder to help`: `@coder` is at 4..10.
const TEXT: &str = "ask @coder to help";

fn coder_mention() -> Mention {
    Mention {
        agent_id: AgentId::new("coder"),
        label: "@coder".to_owned(),
        start: 4,
        end: 10,
        card_url: None,
    }
}

fn working_status() -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update: AgentUpdate::Status {
            state: AgentTaskState::Working,
            detail: None,
        },
    }
}

fn ask_coder(call_key: &str) -> Input {
    ask_of("coder", call_key)
}

fn ask_of(agent: &str, call_key: &str) -> Input {
    Input::Ask {
        actor: Actor::agent(&AgentId::new("plain"), Some("r1".to_owned())),
        caller: Caller::Main,
        agent: AgentId::new(agent),
        text: "find the data".to_owned(),
        call_key: Some(call_key.to_owned()),
        parent_step: None,
        limits: AskLimits::default(),
    }
}

/// A thread addressed to `plain` whose first message mentions `coder`, and whose agent is working.
async fn working_thread<P: orch_ports::Ports>(app: &App<P>) -> ThreadRecord {
    let id = ThreadId(uuid::Uuid::now_v7());
    let Creation::Created { thread, .. } = app
        .create_thread_as(
            &alice(),
            id,
            NewThread {
                title: None,
                target: target("plain"),
                text: TEXT.to_owned(),
            },
            Inbound {
                mentions: vec![coder_mention()],
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("a new thread");
    };
    assert!(thread.job.mentioned.contains(&AgentId::new("coder")));
    let applied = app
        .apply(id, working_status(), None, None, None)
        .await
        .unwrap();
    assert!(matches!(applied, ApplyOutcome::Applied { .. }));
    app.get_thread(&alice(), id).await.unwrap()
}

async fn ask_rows(w: &World, id: ThreadId) -> Vec<orch_ports::OutboxItem> {
    w.store
        .list_open_outbox(id)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Ask)
        .collect()
}

fn deadline_key(id: ThreadId, job: u32, ask: u32) -> String {
    NewTimer {
        id: orch_ports::InboxId(uuid::Uuid::nil()),
        after: SignedDuration::ZERO,
        timer: Timer::AskDeadline { job, ask },
    }
    .idempotency_key(id)
}

#[tokio::test]
async fn an_ask_is_one_commit_of_its_event_its_row_and_its_deadline() {
    let w = World::new();
    let app = w.app();
    let t = working_thread(&app).await;
    let before = events(&app, &alice(), t.id).await.len();

    let outcome = app
        .apply(t.id, ask_coder("ask:t:main:c1"), None, None, None)
        .await
        .unwrap();
    let ApplyOutcome::Applied { events: logged, .. } = outcome else {
        panic!("applied");
    };
    assert_eq!(
        logged.iter().map(|e| e.kind()).collect::<Vec<_>>(),
        [EventKind::AskStarted]
    );
    assert_eq!(
        logged[0].actor,
        Actor::agent(&AgentId::new("plain"), Some("r1".to_owned()))
    );
    assert_eq!(events(&app, &alice(), t.id).await.len(), before + 1);

    // the ledger
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.state, ThreadState::Working);
    assert_eq!(thread.job.asks.len(), 1);
    assert_eq!(
        thread.job.asks[0].call_key.as_deref(),
        Some("ask:t:main:c1")
    );

    // the row
    let rows = ask_rows(&w, t.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, OutboxStatus::Pending);
    assert_eq!(
        rows[0].payload,
        OutboxPayload::Ask {
            job: 1,
            ask: 1,
            agent: AgentId::new("coder"),
            depth: 1,
            text: "find the data".to_owned(),
            continue_task: None,
            reference_task_ids: Vec::new(),
        }
    );

    // the deadline: an inbox row, due after the limit the input carried
    let row = w
        .store
        .find_inbox(TIMER_SOURCE, &deadline_key(t.id, 1, 1))
        .await
        .unwrap()
        .expect("the deadline is an inbox row");
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!(
        row.decode().unwrap(),
        InboxPayload::Timer {
            thread: t.id,
            timer: Timer::AskDeadline { job: 1, ask: 1 }
        }
    );

    // the same call again is the same ask: nothing more is written
    let again = app
        .apply(t.id, ask_coder("ask:t:main:c1"), None, None, None)
        .await
        .unwrap();
    assert!(matches!(again, ApplyOutcome::Applied { .. }));
    assert_eq!(ask_rows(&w, t.id).await.len(), 1);
    assert_eq!(events(&app, &alice(), t.id).await.len(), before + 1);
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().job.asks.len(),
        1
    );
}

#[tokio::test]
async fn a_refused_ask_writes_nothing_and_says_why() {
    let w = World::new();
    let app = w.app();
    let t = working_thread(&app).await;
    let before = app.get_thread(&alice(), t.id).await.unwrap();

    // an agent nobody mentioned
    let err = app
        .apply(t.id, ask_of("x", "k"), None, None, None)
        .await
        .unwrap_err();
    let AppError::Transition(TransitionError::AskRefused(AskRefusal::NotMentioned {
        mentioned,
        ..
    })) = &err
    else {
        panic!("a refusal: {err:?}");
    };
    assert_eq!(mentioned, &[AgentId::new("coder")]);
    assert_eq!(err.class(), ErrorClass::Invalid);

    // a thread that is over takes none
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: None,
            },
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let err = app
        .apply(t.id, ask_coder("k2"), None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppError::Transition(TransitionError::AskRefused(AskRefusal::TaskOver))
    ));
    assert_eq!(err.class(), ErrorClass::Rejected);

    let after = app.get_thread(&alice(), t.id).await.unwrap();
    assert!(after.job.asks.is_empty());
    assert!(ask_rows(&w, t.id).await.is_empty());
    assert_eq!(after.state, ThreadState::Done);
    assert!(
        before.version < after.version,
        "only the completion moved it"
    );
}

#[tokio::test]
async fn a_user_cannot_submit_an_ask_or_the_end_of_one() {
    let w = World::new();
    let app = w.app();
    let t = working_thread(&app).await;
    app.apply(t.id, ask_coder("k"), None, None, None)
        .await
        .unwrap();
    let before = app.get_thread(&alice(), t.id).await.unwrap();
    let forged = [
        ask_coder("k2"),
        Input::AskSent {
            ask: 1,
            task_id: "t".to_owned(),
        },
        Input::AskFinished {
            ask: 1,
            revision: None,
            result: orch_core::AskResult::of(AskOutcome::Completed),
        },
        Input::AskFailed {
            ask: 1,
            reason: "gone".to_owned(),
        },
    ];
    for input in forged {
        let name = input.name();
        // the owner, who may write to the thread, is refused as a user; so is anyone else, who
        // does not even see the thread
        let err = app
            .submit(&alice(), t.id, input.clone(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Invalid(_)), "{name}: {err:?}");
        assert_eq!(err.class(), ErrorClass::Invalid, "{name}");
        let err = app.submit(&bob(), t.id, input, None).await.unwrap_err();
        assert!(matches!(err, AppError::NotFound), "{name}: {err:?}");
    }
    let after = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(after.version, before.version, "nothing was written");
    assert_eq!(after.job.asks, before.job.asks);
}

#[tokio::test]
async fn the_dispatcher_keeps_an_ask_row_it_cannot_send_and_does_not_spin_on_it() {
    let w = World::new();
    let app = w.app();
    let t = working_thread(&app).await;
    app.apply(t.id, ask_coder("k"), None, None, None)
        .await
        .unwrap();
    // the thread's own delegation is not what this test is about
    w.store
        .skip_unsent_delegates(t.id, orch_ports::Clock::now(&orch_ports::SystemClock))
        .await
        .unwrap();
    let version = app.get_thread(&alice(), t.id).await.unwrap().version;
    let sends_before = w.agent.sends().len();
    let run = spawn_dispatcher(&app, fast(), "d1");

    // claimed once, and put back with the reason: pending, not finished, not dead
    let row = eventually("the ask row is claimed and kept", || async {
        let rows = ask_rows(&w, t.id).await;
        let row = rows.into_iter().next()?;
        (row.attempts == 1 && row.status == OutboxStatus::Pending && row.last_error.is_some())
            .then_some(row)
    })
    .await;
    assert_eq!(row.last_error.as_deref(), Some("asks are not sent yet"));
    assert_eq!(row.sent_at, None);
    assert!(row.lease_owner.is_none());
    // due in a minute, not in a moment: a dispatcher that cannot send an ask does not busy-loop
    let wait = row
        .next_attempt_at
        .duration_since(orch_ports::Clock::now(&orch_ports::SystemClock));
    assert!(
        wait > SignedDuration::from_secs(30),
        "next attempt in {wait:?}"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    let later = ask_rows(&w, t.id).await;
    assert_eq!(later.len(), 1, "never finished");
    assert_eq!(later[0].attempts, 1, "never claimed again");
    assert_eq!(later[0].status, OutboxStatus::Pending);

    // nothing was sent to anybody and nothing was written to the thread
    assert_eq!(w.agent.sends().len(), sends_before);
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.version, version);
    assert_eq!(thread.job.asks.len(), 1);
    assert!(thread.job.asks[0].is_running());
    run.shutdown().await;
}

// ---- the deadline, through the inbox worker -------------------------------------------------

type FixedPorts =
    PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, FixedClock, SeqIds, orch_ports::NoModel>;

struct Rig {
    store: MemoryStore,
    clock: FixedClock,
    app: Arc<App<FixedPorts>>,
}

fn t0() -> Timestamp {
    "2026-01-01T00:00:00Z".parse().unwrap()
}

impl Rig {
    fn new() -> Self {
        let store = MemoryStore::new();
        let clock = FixedClock::new(t0());
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: clock.clone(),
                    ids: SeqIds::default(),
                    model: orch_ports::NoModel,
                    auth: orch_ports::RefuseAll,
                    registry: directory().fixed_registry(),
                },
                directory(),
                AppConfig::default(),
            )
            .unwrap(),
        );
        Rig { store, clock, app }
    }
}

fn ask_for(secs: i64) -> Input {
    let Input::Ask {
        actor,
        caller,
        agent,
        text,
        call_key,
        parent_step,
        ..
    } = ask_coder("k")
    else {
        panic!("an ask");
    };
    Input::Ask {
        actor,
        caller,
        agent,
        text,
        call_key,
        parent_step,
        limits: AskLimits {
            timeout: SignedDuration::from_secs(secs),
            ..AskLimits::default()
        },
    }
}

fn finished_asks(events: &[orch_core::Event]) -> Vec<(u32, AskOutcome, Actor)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::AskFinished(d) => Some((d.ask, d.state, e.actor.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_deadline_ends_the_ask_once_when_it_is_due_and_not_before() {
    let rig = Rig::new();
    let worker = orch_app::InboxWorker::new(Arc::clone(&rig.app), InboxConfig::default(), "w1");
    let t = working_thread(&rig.app).await;
    rig.app
        .apply(t.id, ask_for(90), None, None, None)
        .await
        .unwrap();

    assert_eq!(worker.tick().await, 0, "nothing is due");
    rig.clock.advance(Duration::from_secs(89));
    assert_eq!(worker.tick().await, 0, "one second early");
    assert!(
        rig.app.get_thread(&alice(), t.id).await.unwrap().job.asks[0].is_running(),
        "still running"
    );

    rig.clock.advance(Duration::from_secs(1));
    assert_eq!(worker.tick().await, 1);
    let thread = rig.app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        thread.state,
        ThreadState::Working,
        "the job is not the ask's to end"
    );
    assert_eq!(thread.job.asks[0].outcome, Some(AskOutcome::TimedOut));
    let log = rig.app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    assert_eq!(
        finished_asks(&log),
        [(1, AskOutcome::TimedOut, Actor::system())]
    );
    let row = rig
        .store
        .find_inbox(TIMER_SOURCE, &deadline_key(t.id, 1, 1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InboxStatus::Applied);
    assert_eq!(worker.tick().await, 0, "applied once");
    assert_eq!(
        rig.app.get_thread(&alice(), t.id).await.unwrap().version,
        thread.version
    );
}

#[tokio::test]
async fn the_deadline_of_an_ask_the_job_ended_is_applied_and_changes_nothing() {
    let rig = Rig::new();
    let worker = orch_app::InboxWorker::new(Arc::clone(&rig.app), InboxConfig::default(), "w1");
    let t = working_thread(&rig.app).await;
    rig.app
        .apply(t.id, ask_for(90), None, None, None)
        .await
        .unwrap();
    // the asking task ends first: the ask ends with it, canceled
    rig.app
        .apply(
            t.id,
            Input::Agent {
                agent: AgentId::new("plain"),
                revision: None,
                update: AgentUpdate::Status {
                    state: AgentTaskState::Completed,
                    detail: None,
                },
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let done = rig.app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(done.state, ThreadState::Done);
    assert_eq!(done.job.asks[0].outcome, Some(AskOutcome::Canceled));

    rig.clock.advance(Duration::from_secs(3600));
    assert_eq!(
        worker.tick().await,
        1,
        "the deadline is claimed all the same"
    );
    let after = rig.app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        (after.state, after.version),
        (ThreadState::Done, done.version)
    );
    let log = rig.app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    assert_eq!(
        finished_asks(&log),
        [(1, AskOutcome::Canceled, Actor::system())],
        "once"
    );
}
