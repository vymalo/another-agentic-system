//! The ask path of the dispatcher (ADR 0026) against the scripted agent and the in-memory store:
//! what an `ask` outbox row does, one case per way it can end.
//!
//! The job's agent is played by the test (`App::apply`, no dispatcher delivers the thread's own
//! delegation, except where a test says so), so that the only rows the dispatcher has are the
//! asks. `coder` and `scout` are the scripted agents that get asked; the first word of the
//! question picks the script (`echo` answers, `ask` waits for an answer, `slow` runs until
//! cancelled, `gate` until the test says, `failed` fails, `fail` and `down` refuse the request).
//!
//! An ask is put on the row **before** a dispatcher runs, so that the test holds the row that is
//! about to be claimed and can read what became of it afterwards.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::SignedDuration;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Creation, Dispatcher, DispatcherConfig, Inbound,
    InboxConfig, InboxWorker, NewThread,
};
use orch_core::{
    Actor, AgentId, AgentSource, AgentTaskState, AgentUpdate, AskFinishedData, AskLimits,
    AskOutcome, Caller, EventBody, EventKind, Input, Mention, ThreadId, ThreadRecord, ThreadState,
    ask_context,
};
use orch_ports::memory::{Call, MemoryRegistry};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentError, Clock as _, CompositeRegistry, FixedRegistry, Lease,
    OutboxItem, OutboxKind, OutboxStatus, PortSet, RegistryEntry, SendContent, SendRequest,
    SystemClock, ThreadStore,
};
use support::*;
use tokio_util::sync::CancellationToken;

/// `@coder` is at 4..10 and `@scout` at 15..21.
const TEXT: &str = "ask @coder and @scout to help";

fn entry(id: &str, name: &str) -> AgentEntry {
    AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
    }
}

fn directory3() -> AgentDirectory {
    AgentDirectory::new(vec![
        entry("coder", "Coder"),
        entry("plain", "Plain"),
        entry("scout", "Scout"),
    ])
}

fn mentions() -> Vec<Mention> {
    let at = |id: &str, start, end| Mention {
        agent_id: AgentId::new(id),
        label: format!("@{id}"),
        start,
        end,
        card_url: None,
    };
    vec![at("coder", 4, 10), at("scout", 15, 21)]
}

fn app3(w: &World) -> Arc<TestApp> {
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory3().fixed_registry(),
            },
            directory3(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn plain_says(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: None,
        update,
    }
}

fn status(state: AgentTaskState) -> AgentUpdate {
    AgentUpdate::Status {
        state,
        detail: None,
    }
}

/// A thread addressed to `plain` whose first message mentions `coder` and `scout`, with `plain`
/// working on it. Nothing is delivered to `plain`.
async fn working_thread<P: orch_ports::Ports>(
    app: &App<P>,
    store: &impl ThreadStore,
) -> ThreadRecord {
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
                mentions: mentions(),
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("a new thread");
    };
    assert_eq!(thread.job.mentioned.len(), 2);
    // the thread's own delegation is not what these tests are about
    store
        .skip_unsent_delegates(id, SystemClock.now())
        .await
        .unwrap();
    app.apply(
        id,
        plain_says(status(AgentTaskState::Working)),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    app.get_thread(&alice(), id).await.unwrap()
}

fn ask_by(caller: Caller, agent: &str, text: &str, key: &str, limits: AskLimits) -> Input {
    Input::Ask {
        actor: Actor::agent(&AgentId::new("plain"), None),
        caller,
        agent: AgentId::new(agent),
        text: text.to_owned(),
        call_key: Some(key.to_owned()),
        parent_step: None,
        limits,
    }
}

fn ask_of(agent: &str, text: &str, key: &str) -> Input {
    ask_by(Caller::Main, agent, text, key, AskLimits::default())
}

/// Puts the ask on the thread and returns the row that sends it. No dispatcher may be running:
/// the row is the one that is open and was not before.
async fn put<P: orch_ports::Ports>(
    app: &App<P>,
    store: &impl ThreadStore,
    thread: ThreadId,
    input: Input,
) -> OutboxItem {
    let before: Vec<_> = ask_rows(store, thread).await.iter().map(|r| r.id).collect();
    app.apply(thread, input, None, None, None).await.unwrap();
    let mut fresh: Vec<_> = ask_rows(store, thread)
        .await
        .into_iter()
        .filter(|r| !before.contains(&r.id))
        .collect();
    assert_eq!(fresh.len(), 1, "the ask wrote one row");
    fresh.remove(0)
}

async fn ask_rows(store: &impl ThreadStore, thread: ThreadId) -> Vec<OutboxItem> {
    store
        .list_open_outbox(thread)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Ask)
        .collect()
}

async fn row_of(w: &World, row: &OutboxItem) -> OutboxItem {
    w.store.get_outbox(row.id).await.unwrap().unwrap()
}

async fn row_ends(w: &World, row: &OutboxItem) -> OutboxItem {
    eventually("the row ends", || async {
        let now = row_of(w, row).await;
        (!now.status.is_open()).then_some(now)
    })
    .await
}

/// The `ask_finished` of ask `n`, once it is in the log, and who wrote it.
async fn finished(app: &TestApp, thread: ThreadId, n: u32) -> (AskFinishedData, Actor) {
    eventually(&format!("ask {n} is finished"), || async {
        events(app, &alice(), thread)
            .await
            .into_iter()
            .find_map(|e| match e.body {
                EventBody::AskFinished(d) if d.ask == n => Some((d, e.actor)),
                _ => None,
            })
    })
    .await
}

async fn finished_count(app: &TestApp, thread: ThreadId) -> usize {
    events(app, &alice(), thread)
        .await
        .iter()
        .filter(|e| e.kind() == EventKind::AskFinished)
        .count()
}

fn sends_to(w: &World, agent: &str) -> Vec<Call> {
    w.agent
        .sends()
        .into_iter()
        .filter(|c| matches!(c, Call::Send { agent: a, .. } if a.as_str() == agent))
        .collect()
}

fn cancels(w: &World) -> Vec<String> {
    w.agent
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Cancel { task_id } => Some(task_id),
            _ => None,
        })
        .collect()
}

fn finds(w: &World) -> usize {
    w.agent
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Find { .. }))
        .count()
}

fn no_heartbeat() -> DispatcherConfig {
    DispatcherConfig {
        heartbeat: Duration::from_secs(3600),
        ..fast()
    }
}

// ---- completed ----------------------------------------------------------------------------

#[tokio::test]
async fn an_ask_is_sent_in_a_context_of_its_own_and_the_answer_ends_it() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "echo find the data", "k1"),
    )
    .await;
    let before = events(&app, &alice(), t.id).await;
    let binding = w.store.get_binding(t.id).await.unwrap().unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");

    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(done.text.as_deref(), Some("echo: echo find the data"));
    assert_eq!(done.question, None);
    assert_eq!(done.error, None);
    assert_eq!(
        done.artifacts
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["echo"]
    );
    // what the asked agent said is its own: it wrote the answer, the revision it ran as with it
    assert_eq!(
        actor,
        Actor::agent(&AgentId::new("coder"), Some("rev-1".to_owned()))
    );

    // the message: its own context, its own task, the row's id, a grant of its own, nothing of the
    // person's screen or mentions
    let sent = sends_to(&w, "coder");
    assert_eq!(sent.len(), 1);
    let Call::Send {
        message_id,
        context_id,
        task_id,
        reference_task_ids,
        text,
        release,
        ui_catalog,
        thread_tools,
        history,
        steer,
        mentions,
        ..
    } = &sent[0]
    else {
        unreachable!()
    };
    assert_eq!(message_id, &row.id.to_string());
    assert_eq!(context_id, &ask_context(t.id, &AgentId::new("coder")));
    assert_ne!(context_id, &binding.context_id);
    assert_eq!((task_id, reference_task_ids.is_empty()), (&None, true));
    assert_eq!(text, "echo find the data");
    assert_eq!(
        (release, ui_catalog.is_none(), history.is_none()),
        (&None, true, true)
    );
    assert!(!steer);
    assert!(mentions.is_empty());
    let grant = thread_tools.as_deref().expect("a grant");
    assert_eq!(grant.thread, t.id);
    assert_eq!(grant.job, 1);
    assert_eq!(grant.agent, AgentId::new("coder"));
    assert_eq!(grant.caller, Caller::Ask(1));
    assert_eq!(grant.depth, 1);
    assert!(grant.is_consistent());

    // the row ends delivered, with the task on it and on the ledger
    let ended = row_ends(&w, &row).await;
    assert_eq!(ended.status, OutboxStatus::Delivered);
    assert_eq!(ended.task_id.as_deref(), Some("task-1"));
    assert!(ended.sent_at.is_some());
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.asks[0].task_id.as_deref(), Some("task-1"));
    assert_eq!(thread.job.asks[0].outcome, Some(AskOutcome::Completed));

    // the asked agent's task is not the thread's: the binding is untouched, the thread still
    // works, and nothing it said is in the log but the end of the ask
    assert_eq!(thread.state, ThreadState::Working);
    assert_eq!(w.store.get_binding(t.id).await.unwrap().unwrap(), binding);
    let after = events(&app, &alice(), t.id).await;
    assert_eq!(
        after[before.len()..]
            .iter()
            .map(|e| e.kind())
            .collect::<Vec<_>>(),
        [EventKind::AskFinished]
    );
    run.shutdown().await;
}

#[tokio::test]
async fn the_words_an_agent_states_are_its_answer() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "stream tell me", "k1"),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(
        done.text.as_deref(),
        Some(orch_ports::memory::stream_text().as_str())
    );
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Delivered);
    run.shutdown().await;
}

/// Rows are unordered (ADR 0026): the thread's own delegation, in flight, does not hold an ask,
/// and an ask does not hold it.
#[tokio::test]
async fn the_threads_own_delegation_and_an_ask_do_not_wait_for_each_other() {
    let w = World::new();
    let app = app3(&w);
    // the thread's delegation is real this time: `plain` waits at the gate
    let id = ThreadId(uuid::Uuid::now_v7());
    let text = "gate @coder help";
    let Creation::Created { .. } = app
        .create_thread_as(
            &alice(),
            id,
            NewThread {
                title: None,
                target: target("plain"),
                text: text.to_owned(),
            },
            Inbound {
                mentions: vec![Mention {
                    agent_id: AgentId::new("coder"),
                    label: "@coder".to_owned(),
                    start: 5,
                    end: 11,
                    card_url: None,
                }],
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("a new thread");
    };
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), id, ThreadState::Working).await;
    run.shutdown().await;
    let row = put(&*app, &w.store, id, ask_of("coder", "echo meanwhile", "k1")).await;
    let run = spawn_dispatcher(&app, fast(), "d2");

    let (done, _) = finished(&app, id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Delivered);
    assert_eq!(
        app.get_thread(&alice(), id).await.unwrap().state,
        ThreadState::Working,
        "the thread's task is still at the gate"
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), id, ThreadState::Done).await;
    run.shutdown().await;
}

/// The asked agent's `branch` and `checks` are its own: the gate of the job reads those of the
/// agent the thread runs on (owner decision 6).
#[tokio::test]
async fn what_an_asked_agent_pushes_never_reaches_the_gate() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    put(&*app, &w.store, t.id, ask_of("coder", "pushed a fix", "k1")).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(
        done.artifacts
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["branch", "checks"],
        "it is told what the asked agent handed back"
    );
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        thread.state,
        ThreadState::Working,
        "its completion is not the job's"
    );
    assert!(thread.job.pushed.is_none());
    assert!(thread.job.results.is_empty());
    assert!(thread.job.branch_problem.is_none());
    // the asked agent's artifacts and checks are in `ask_finished` and nowhere else
    let log = events(&app, &alice(), t.id).await;
    assert!(
        log.iter()
            .all(|e| !matches!(e.kind(), EventKind::Artifact | EventKind::CheckResult)),
        "{:?}",
        log.iter().map(|e| (e.seq, e.kind())).collect::<Vec<_>>()
    );
    run.shutdown().await;
}

// ---- input required, continued -------------------------------------------------------------

#[tokio::test]
async fn a_question_back_ends_the_ask_and_the_next_ask_to_the_agent_continues_its_task() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;

    // the first ask: the asked agent wants to know something
    let first = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "ask which branch", "k1"),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::InputRequired);
    assert_eq!(done.question.as_deref(), Some("Which branch?"));
    assert_eq!(done.text, None);
    assert_eq!(row_ends(&w, &first).await.status, OutboxStatus::Delivered);
    // its task is on the ledger, waiting
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.asks[0].task_id.as_deref(), Some("task-1"));
    run.shutdown().await;

    // the second ask answers it: the same task, in the same context
    let second = put(&*app, &w.store, t.id, ask_of("coder", "ask main", "k2")).await;
    assert_eq!(
        second.payload,
        orch_ports::OutboxPayload::Ask {
            job: 1,
            ask: 2,
            agent: AgentId::new("coder"),
            depth: 1,
            text: "ask main".to_owned(),
            continue_task: Some("task-1".to_owned()),
            reference_task_ids: Vec::new(),
        }
    );
    let run = spawn_dispatcher(&app, fast(), "d2");
    let (done, _) = finished(&app, t.id, 2).await;
    assert_eq!(done.state, AskOutcome::Completed, "{done:?}");
    assert_eq!(done.text.as_deref(), Some("answered: ask main"));
    assert_eq!(row_ends(&w, &second).await.status, OutboxStatus::Delivered);
    run.shutdown().await;

    // the third ask is a new task that says which one it follows
    let third = put(&*app, &w.store, t.id, ask_of("coder", "echo again", "k3")).await;
    let run = spawn_dispatcher(&app, fast(), "d3");
    let (done, _) = finished(&app, t.id, 3).await;
    assert_eq!(done.state, AskOutcome::Completed);
    row_ends(&w, &third).await;
    run.shutdown().await;

    let sent = sends_to(&w, "coder");
    let shape: Vec<_> = sent
        .iter()
        .map(|c| {
            let Call::Send {
                context_id,
                task_id,
                reference_task_ids,
                ..
            } = c
            else {
                unreachable!()
            };
            (
                context_id.clone(),
                task_id.clone(),
                reference_task_ids.clone(),
            )
        })
        .collect();
    let context = ask_context(t.id, &AgentId::new("coder"));
    assert_eq!(
        shape,
        [
            (context.clone(), None, vec![]),
            (context.clone(), Some("task-1".to_owned()), vec![]),
            // the continued task is over: the next one refers to what the agent did before
            (context, None, vec!["task-1".to_owned()]),
        ]
    );
}

/// A real agent's stream for a message that continues a task begins with the task as it stands:
/// still waiting. The ask that continues it must not end with that question.
#[tokio::test]
async fn the_snapshot_of_the_waiting_task_is_not_taken_for_the_answer_that_continues_it() {
    let w = World::new();
    w.agent.set_snapshot_on_continue(true);
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let first = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "ask which branch", "k1"),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::InputRequired);
    row_ends(&w, &first).await;
    run.shutdown().await;

    let second = put(&*app, &w.store, t.id, ask_of("coder", "ask main", "k2")).await;
    let run = spawn_dispatcher(&app, fast(), "d2");
    let (done, _) = finished(&app, t.id, 2).await;
    assert_eq!(done.state, AskOutcome::Completed, "{done:?}");
    assert_eq!(done.text.as_deref(), Some("answered: ask main"));
    assert_eq!(row_ends(&w, &second).await.status, OutboxStatus::Delivered);
    run.shutdown().await;
}

// ---- failed --------------------------------------------------------------------------------

#[tokio::test]
async fn a_task_that_fails_ends_the_ask_failed_and_the_job_goes_on() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "failed to do it", "k1"),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Failed);
    assert_eq!(done.error.as_deref(), Some("scripted failure"));
    assert_eq!(
        actor,
        Actor::agent(&AgentId::new("coder"), Some("rev-1".to_owned()))
    );
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Delivered);
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        thread.state,
        ThreadState::Working,
        "the asker decides what to do about it"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_request_the_agent_refuses_fails_the_ask_at_once() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "fail please", "k1")).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Failed);
    assert!(done.error.is_some());
    assert_eq!(
        actor,
        Actor::system(),
        "the orchestrator could not get an answer"
    );
    let ended = row_ends(&w, &row).await;
    assert_eq!(ended.status, OutboxStatus::Dead);
    assert_eq!(sends_to(&w, "coder").len(), 1, "a refusal is not retried");
    run.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_cannot_be_reached_is_tried_again_and_then_the_ask_fails() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "down for the count", "k1"),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!((done.state, actor), (AskOutcome::Failed, Actor::system()));
    let ended = row_ends(&w, &row).await;
    assert_eq!(ended.status, OutboxStatus::Dead);
    assert_eq!(
        sends_to(&w, "coder").len(),
        fast().max_attempts as usize,
        "the budget, and no more"
    );
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.state, ThreadState::Working);
    assert_eq!(finished_count(&app, t.id).await, 1);
    run.shutdown().await;
}

/// A row an earlier build parked was claimed without being sent: those claims are not failed
/// sends, so the first real one does not take the row to its last attempt.
#[tokio::test]
async fn the_claims_of_a_parked_row_are_not_failed_sends() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "down again", "k1")).await;
    // an earlier build claimed it four times and put it back each time
    for _ in 0..4 {
        let claimed = w
            .store
            .claim_outbox("old", SystemClock.now(), Duration::from_secs(60), 10)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        w.store
            .retry_outbox(
                &Lease {
                    id: row.id,
                    owner: "old".into(),
                    attempt: claimed[0].attempts,
                },
                SystemClock.now(),
                "asks are not sent yet".to_owned(),
            )
            .await
            .unwrap();
    }
    assert_eq!(row_of(&w, &row).await.attempts, 4);
    // a long backoff: the first failed send is the only one this test sees
    let run = spawn_dispatcher(
        &app,
        DispatcherConfig {
            backoff_base: Duration::from_secs(30),
            backoff_max: Duration::from_secs(60),
            ..fast()
        },
        "d1",
    );
    eventually("the first real send fails and is retried", || async {
        let now = row_of(&w, &row).await;
        (now.attempts == 5
            && now.status == OutboxStatus::Pending
            && now.last_error != Some("asks are not sent yet".to_owned()))
        .then_some(())
    })
    .await;
    assert_eq!(sends_to(&w, "coder").len(), 1);
    assert_eq!(finished_count(&app, t.id).await, 0, "the ask still runs");
    run.shutdown().await;
}

// ---- the registry --------------------------------------------------------------------------

type RegistryPorts = PortSet<
    orch_ports::memory::MemoryStore,
    orch_ports::memory::MemoryWakeup,
    orch_ports::memory::ScriptedAgent,
    SystemClock,
    orch_ports::memory::SeqIds,
    orch_ports::memory::ScriptedModel,
    CompositeRegistry<FixedRegistry, MemoryRegistry>,
>;

/// `plain` and `coder` are static; `scout` is the platform's, in a registry the test changes.
fn app_over(w: &World, registry: &MemoryRegistry) -> Arc<App<RegistryPorts>> {
    let static_agents = AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")]);
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: CompositeRegistry::new(static_agents.fixed_registry(), registry.clone()),
            },
            static_agents,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn platform_scout() -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("scout"),
            "https://scout.agents.example.com/.well-known/agent-card.json",
            None,
        ),
        name: "Scout".to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
    }
}

fn spawn_over(app: &Arc<App<RegistryPorts>>) -> (tokio::task::JoinHandle<()>, CancellationToken) {
    let token = CancellationToken::new();
    let dispatcher = Dispatcher::new(Arc::clone(app), fast(), "d1");
    (tokio::spawn(dispatcher.run(token.clone())), token)
}

#[tokio::test]
async fn an_agent_the_registry_no_longer_lists_fails_the_ask_and_a_registry_that_is_down_waits() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(platform_scout());
    let app = app_over(&w, &registry);
    let t = working_thread(&*app, &w.store).await;

    // the registry is down: the row waits for it, sends nothing, and the ask is not failed by it
    let waits = put(&*app, &w.store, t.id, ask_of("scout", "echo hello", "k1")).await;
    registry.set_down(true);
    let (handle, token) = spawn_over(&app);
    eventually("the row is tried again", || async {
        (row_of(&w, &waits).await.attempts >= 3).then_some(())
    })
    .await;
    assert!(sends_to(&w, "scout").is_empty());
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().job.asks[0].outcome,
        None
    );
    // it comes back: the ask is sent and answered
    registry.set_down(false);
    eventually("the ask is answered", || async {
        let thread = app.get_thread(&alice(), t.id).await.unwrap();
        (thread.job.asks[0].outcome == Some(AskOutcome::Completed)).then_some(())
    })
    .await;
    assert_eq!(sends_to(&w, "scout").len(), 1);
    token.cancel();
    handle.await.unwrap();

    // the platform removes the agent: the next ask to it fails, and says so
    registry.remove(&AgentId::new("scout"));
    let gone = put(
        &*app,
        &w.store,
        t.id,
        ask_of("scout", "echo hello again", "k2"),
    )
    .await;
    let (handle, token) = spawn_over(&app);
    eventually("the ask fails", || async {
        let thread = app.get_thread(&alice(), t.id).await.unwrap();
        (thread.job.asks[1].outcome == Some(AskOutcome::Failed)).then_some(())
    })
    .await;
    let log = app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    let ended = log
        .iter()
        .find_map(|e| match &e.body {
            EventBody::AskFinished(d) if d.ask == 2 => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        ended.error.as_deref(),
        Some("agent 'scout' is no longer listed")
    );
    assert_eq!(row_ends(&w, &gone).await.status, OutboxStatus::Dead);
    assert_eq!(
        sends_to(&w, "scout").len(),
        1,
        "nothing was sent to it again"
    );
    token.cancel();
    handle.await.unwrap();
}

// ---- cancelled -----------------------------------------------------------------------------

async fn tasks_known(w: &World, rows: &[&OutboxItem]) {
    for row in rows {
        eventually("the asked agent has the task", || async {
            row_of(w, row).await.task_id.map(|_| ())
        })
        .await;
    }
}

#[tokio::test]
async fn the_persons_stop_ends_the_asks_and_cancels_the_tasks_of_every_one() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let first = put(&*app, &w.store, t.id, ask_of("coder", "slow work", "k1")).await;
    // the asked agent asks one in turn
    let nested = put(
        &*app,
        &w.store,
        t.id,
        ask_by(
            Caller::Ask(1),
            "scout",
            "slow more",
            "k2",
            AskLimits::default(),
        ),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    tasks_known(&w, &[&first, &nested]).await;
    assert!(cancels(&w).is_empty());

    app.cancel(&alice(), t.id).await.unwrap();

    // the core ended both asks, `canceled`, the moment the person stopped the job
    for (n, why) in [
        (1, "the person stopped the job"),
        (2, "the asking task ended"),
    ] {
        let (done, actor) = finished(&app, t.id, n).await;
        assert_eq!(done.state, AskOutcome::Canceled, "ask {n}");
        assert_eq!(done.error.as_deref(), Some(why), "ask {n}");
        assert_eq!(actor, Actor::system());
    }
    // and the rows noticed: both asked tasks are told to stop, and the rows are over
    for row in [&first, &nested] {
        assert_eq!(row_ends(&w, row).await.status, OutboxStatus::Skipped);
    }
    let mut stopped = cancels(&w);
    stopped.sort();
    assert_eq!(stopped, ["task-1", "task-2"]);
    wait_state(&app, &alice(), t.id, ThreadState::Cancelled).await;
    assert_eq!(finished_count(&app, t.id).await, 2, "each ask ended once");
    run.shutdown().await;
}

#[tokio::test]
async fn the_end_of_the_asking_task_ends_the_asks_it_made() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "slow work", "k1")).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    tasks_known(&w, &[&row]).await;
    // the job's agent finishes without waiting for the answer
    app.apply(
        t.id,
        plain_says(status(AgentTaskState::Completed)),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Canceled);
    assert_eq!(done.error.as_deref(), Some("the asking task ended"));
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Skipped);
    assert_eq!(cancels(&w), ["task-1"]);
    run.shutdown().await;
}

// ---- the deadline --------------------------------------------------------------------------

fn fast_inbox() -> InboxConfig {
    InboxConfig {
        poll_interval: Duration::from_millis(50),
        ..InboxConfig::default()
    }
}

#[tokio::test]
async fn the_deadline_ends_the_ask_and_the_asked_agent_is_told_to_stop() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let short = AskLimits::default().with_timeout(Duration::from_millis(400));
    let first = put(
        &*app,
        &w.store,
        t.id,
        ask_by(Caller::Main, "coder", "slow work", "k1", short),
    )
    .await;
    // it has asked one in turn, with a long deadline of its own
    let nested = put(
        &*app,
        &w.store,
        t.id,
        ask_by(
            Caller::Ask(1),
            "scout",
            "slow more",
            "k2",
            AskLimits::default(),
        ),
    )
    .await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    tasks_known(&w, &[&first, &nested]).await;
    let token = CancellationToken::new();
    let inbox =
        tokio::spawn(InboxWorker::new(Arc::clone(&app), fast_inbox(), "w1").run(token.clone()));

    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::TimedOut);
    assert_eq!(actor, Actor::system());
    // the ask above it is over, so is the one it asked
    let (child, _) = finished(&app, t.id, 2).await;
    assert_eq!(child.state, AskOutcome::Canceled);
    assert_eq!(child.error.as_deref(), Some("the asking task ended"));
    for row in [&first, &nested] {
        assert_eq!(row_ends(&w, row).await.status, OutboxStatus::Skipped);
    }
    let mut stopped = cancels(&w);
    stopped.sort();
    assert_eq!(stopped, ["task-1", "task-2"]);
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(
        thread.state,
        ThreadState::Working,
        "the job is not the ask's to end"
    );
    assert_eq!(finished_count(&app, t.id).await, 2);
    token.cancel();
    inbox.await.unwrap();
    run.shutdown().await;
}

// ---- crashes -------------------------------------------------------------------------------

/// Plays a first claimant that sent the message and died before recording anything: the message
/// reached the asked agent (`task-1`), the row knows nothing.
async fn sent_and_forgotten(
    w: &World,
    thread: ThreadId,
    row: &OutboxItem,
    agent: &str,
    text: &str,
) {
    let claimed = w
        .store
        .claim_outbox("d1", SystemClock.now(), Duration::from_millis(50), 10)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    let stream = AgentClient::send_stream(
        &w.agent,
        SendRequest {
            endpoint: AgentEndpoint::a2a(AgentId::new(agent), "https://x.example.com/card", None),
            message_id: row.id.to_string(),
            context_id: ask_context(thread, &AgentId::new(agent)),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text(text.to_owned()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
        },
    )
    .await
    .unwrap();
    drop(stream);
    assert_eq!(row_of(w, row).await.task_id, None);
}

/// A crash between sending and recording: the row has no task, and the next claimant looks the
/// message up by its id instead of sending it again.
#[tokio::test]
async fn a_message_that_reached_the_agent_before_the_crash_is_found_not_resent() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "gate find it", "k1")).await;
    sent_and_forgotten(&w, t.id, &row, "coder", "gate find it").await;

    let run = spawn_dispatcher(&app, fast(), "d2");
    eventually("the message is found", || async {
        (finds(&w) > 0).then_some(())
    })
    .await;
    eventually("the task is on the row", || async {
        row_of(&w, &row).await.task_id.map(|_| ())
    })
    .await;
    w.agent.release_gate();
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(sends_to(&w, "coder").len(), 1, "never sent a second time");
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.asks[0].task_id.as_deref(), Some("task-1"));
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Delivered);
    run.shutdown().await;
}

/// A lookup that fails does not say the agent was not asked: it is tried again, and the message is
/// not sent a second time because of it.
#[tokio::test]
async fn a_lookup_that_fails_is_tried_again_and_does_not_cost_a_second_message() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "gate find it", "k1")).await;
    sent_and_forgotten(&w, t.id, &row, "coder", "gate find it").await;
    w.agent.fail_next_finds(2);

    let run = spawn_dispatcher(&app, fast(), "d2");
    eventually("the lookup got through", || async {
        row_of(&w, &row).await.task_id.map(|_| ())
    })
    .await;
    assert_eq!(finds(&w), 3, "two failures, then the answer");
    w.agent.release_gate();
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(sends_to(&w, "coder").len(), 1);
    run.shutdown().await;
}

/// A lookup that keeps failing is not "no such task": the message is never sent again, the row is
/// retried, and in the end the ask fails.
#[tokio::test]
async fn a_lookup_that_never_answers_fails_the_ask_instead_of_asking_twice() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "echo find it", "k1")).await;
    sent_and_forgotten(&w, t.id, &row, "coder", "echo find it").await;
    w.agent.fail_next_finds(usize::MAX);

    let run = spawn_dispatcher(&app, fast(), "d2");
    let (done, actor) = finished(&app, t.id, 1).await;
    assert_eq!((done.state, actor), (AskOutcome::Failed, Actor::system()));
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Dead);
    assert!(finds(&w) > 3, "the lookup was retried, then the row was");
    assert_eq!(
        sends_to(&w, "coder").len(),
        1,
        "the only message is the one the crashed claimant sent"
    );
    run.shutdown().await;
}

/// The process that sent the message dies. The next claimant finds the task on the row and
/// follows it: the agent is asked once, and the ask ends once.
#[tokio::test]
async fn a_reclaimed_row_follows_the_asked_agents_task_instead_of_asking_again() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "gate find it", "k1")).await;
    let first = spawn_dispatcher(&app, fast(), "d1");
    eventually("the message is sent", || async {
        row_of(&w, &row).await.task_id.map(|_| ())
    })
    .await;
    first.kill();
    let second = spawn_dispatcher(&app, fast(), "d2");
    eventually("the second worker re-attaches", || async {
        w.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Resubscribe { .. }))
            .then_some(())
    })
    .await;
    w.agent.release_gate();
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(sends_to(&w, "coder").len(), 1, "asked once");
    assert_eq!(finished_count(&app, t.id).await, 1);
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Delivered);
    second.shutdown().await;
}

/// The claimant that died after recording the task on the row and before telling the core: the
/// next one tells it, so the next ask of the agent can continue the task.
#[tokio::test]
async fn a_task_recorded_on_the_row_but_not_on_the_ledger_is_recorded_by_the_next_claimant() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "ask which branch", "k1"),
    )
    .await;
    sent_and_forgotten(&w, t.id, &row, "coder", "ask which branch").await;
    // the first claimant got as far as the row
    let lease = Lease {
        id: row.id,
        owner: "d1".into(),
        attempt: 1,
    };
    assert!(
        w.store
            .mark_verify_sent(&lease, "task-1".to_owned(), SystemClock.now())
            .await
            .unwrap()
    );
    assert_eq!(
        app.get_thread(&alice(), t.id).await.unwrap().job.asks[0].task_id,
        None
    );

    let run = spawn_dispatcher(&app, fast(), "d2");
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::InputRequired);
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.asks[0].task_id.as_deref(), Some("task-1"));
    assert_eq!(sends_to(&w, "coder").len(), 1);
    run.shutdown().await;

    // and the ask that follows it continues it
    let next = put(&*app, &w.store, t.id, ask_of("coder", "ask main", "k2")).await;
    assert!(matches!(
        next.payload,
        orch_ports::OutboxPayload::Ask { continue_task: Some(ref t), .. } if t == "task-1"
    ));
}

/// A task a crashed claimant started and nobody followed is cancelled when its ask is over.
#[tokio::test]
async fn a_task_nobody_follows_is_cancelled_when_its_ask_is_over() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "slow work", "k1")).await;
    sent_and_forgotten(&w, t.id, &row, "coder", "slow work").await;
    // the person stops the job while nobody holds the row
    app.cancel(&alice(), t.id).await.unwrap();
    let (done, _) = finished(&app, t.id, 1).await;
    assert_eq!(done.state, AskOutcome::Canceled);
    assert!(cancels(&w).is_empty());

    let run = spawn_dispatcher(&app, fast(), "d2");
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Skipped);
    assert_eq!(
        cancels(&w),
        ["task-1"],
        "found by its message id, and stopped"
    );
    assert_eq!(sends_to(&w, "coder").len(), 1, "and never sent again");
    run.shutdown().await;
}

/// A row whose ask is over before it is claimed is dropped without asking anybody.
#[tokio::test]
async fn an_ask_that_is_over_before_it_is_sent_is_never_sent() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(
        &*app,
        &w.store,
        t.id,
        ask_of("coder", "echo too late", "k1"),
    )
    .await;
    app.cancel(&alice(), t.id).await.unwrap();
    let run = spawn_dispatcher(&app, fast(), "d1");
    assert_eq!(row_ends(&w, &row).await.status, OutboxStatus::Skipped);
    assert!(sends_to(&w, "coder").is_empty());
    assert!(cancels(&w).is_empty());
    run.shutdown().await;
}

/// The worker that lost its claim writes nothing: its answer is refused by the store, and the
/// worker that holds the row decides.
#[tokio::test]
async fn a_worker_that_lost_its_claim_writes_nothing() {
    let w = World::new();
    let app = app3(&w);
    let t = working_thread(&*app, &w.store).await;
    let row = put(&*app, &w.store, t.id, ask_of("coder", "gate find it", "k1")).await;
    let run = spawn_dispatcher(&app, no_heartbeat(), "d1");
    eventually("the message is sent", || async {
        row_of(&w, &row).await.task_id.map(|_| ())
    })
    .await;
    let before = events(&app, &alice(), t.id).await.len();
    let claimed = w
        .store
        .claim_outbox(
            "thief",
            SystemClock.now() + SignedDuration::from_secs(1),
            Duration::from_secs(3600),
            10,
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1, "the lapsed ask row");
    w.agent.release_gate();
    eventually("the old worker's write is refused", || async {
        (w.store.fenced_commits() > 0).then_some(())
    })
    .await;
    assert_eq!(events(&app, &alice(), t.id).await.len(), before);
    assert_eq!(finished_count(&app, t.id).await, 0);
    let held = row_of(&w, &row).await;
    assert_eq!(
        (held.status, held.lease_owner.as_deref()),
        (OutboxStatus::Inflight, Some("thief"))
    );
    run.kill();
}

#[allow(dead_code)]
fn unused(_: AgentError) {}
