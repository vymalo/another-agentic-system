//! The dispatcher's side of ADR 0084: a run is bound when its stream first names the task id, with
//! the endpoint the live card maps the person's pick to (the status note of 2026-11-24), and
//! unbound at every end state of its thread (`done`, `failed`, `cancelled`), idempotently. A broker
//! that cannot be reached is logged and the run goes on (S9 parks it); a later message that picks
//! another alias is refused (`409`), never re-pointed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use jiff::SignedDuration;
use orch_app::{Creation, Dispatcher, DispatcherConfig, Inbound, NewThread};
use orch_core::{AgentId, ModelPick, ModelPickOffer, ThreadId, ThreadState};
use orch_ports::memory::MemoryRunLeases;
use orch_ports::{
    AgentError, Clock as _, RunBinding, RunBindingError, RunBindings, RunCaller, RunId,
    RunLeaseError, RunLeases, SystemClock, ThreadStore as _, ToolSecret,
};
use serde_json::json;
use support::*;
use tokio_util::sync::CancellationToken;

/// The broker, as the test sees it: the `RunBindings` port the dispatcher holds, over the broker's
/// own in-memory `RunLeases` (the other half of the same wire), so that a test can both read what
/// was sent and ask the injector's lookup what it would answer.
#[derive(Clone, Default)]
struct Broker {
    leases: Arc<MemoryRunLeases>,
    binds: Arc<Mutex<Vec<RunBinding>>>,
    /// What the broker answered each bind, in order: `Err(Conflict)` is the `409`.
    outcomes: Arc<Mutex<Vec<Result<(), RunBindingError>>>>,
    unbinds: Arc<Mutex<Vec<RunId>>>,
    /// The store's view of the thread at the moment of the bind: how far its log had come.
    bind_seqs: Arc<Mutex<Vec<i64>>>,
    /// Where to look the thread up when a bind happens (the ordering test).
    watched: Arc<Mutex<Option<(orch_ports::memory::MemoryStore, ThreadId)>>>,
    /// Every bind is refused as unreachable.
    unreachable: Arc<AtomicBool>,
}

impl Broker {
    /// A broker whose deployment holds a credential for the endpoint `frontier`.
    fn new() -> Self {
        Broker::with_endpoints([("frontier", "key-frontier")])
    }

    /// A broker whose deployment holds a credential per model endpoint label.
    fn with_endpoints<const N: usize>(tokens: [(&str, &str); N]) -> Self {
        Broker {
            leases: Arc::new(MemoryRunLeases::new(
                tokens
                    .into_iter()
                    .map(|(model, token)| (model.to_owned(), ToolSecret::new(token)))
                    .collect(),
            )),
            ..Broker::default()
        }
    }

    fn binds(&self) -> Vec<RunBinding> {
        self.binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn outcomes(&self) -> Vec<Result<(), RunBindingError>> {
        self.outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn unbinds(&self) -> Vec<RunId> {
        self.unbinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn bind_seqs(&self) -> Vec<i64> {
        self.bind_seqs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records a bind the dispatcher asked for.
    fn record(&self, binding: &RunBinding) {
        self.binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(binding.clone());
    }
}

impl RunBindings for Broker {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunBindingError> {
        // The lock is not held across the await: the guard is cloned out and dropped first.
        let watched = {
            let guard = self.watched.lock().unwrap_or_else(PoisonError::into_inner);
            guard.clone()
        };
        if let Some((store, thread)) = watched
            && let Ok(Some(record)) = store.get_thread(None, thread).await
        {
            self.bind_seqs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(record.last_seq);
        }
        let outcome = if self.unreachable.load(Ordering::SeqCst) {
            Err(RunBindingError::Unreachable)
        } else {
            // Recorded once the broker has answered, so that a test which sees the call in `binds`
            // also finds the binding where a lookup would: the two cannot disagree.
            self.leases
                .bind(binding.clone())
                .await
                .map_err(|_| RunBindingError::Conflict)
        };
        self.record(&binding);
        self.outcomes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(outcome);
        outcome
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunBindingError> {
        // The broker first, the record second: a test that sees the call also sees it gone.
        let outcome = self.leases.unbind(run).await;
        self.unbinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(run.clone());
        outcome.map_err(|_| RunBindingError::Unreachable)
    }
}

fn broker_deadline_is_a_day(binding: &RunBinding) {
    let now = SystemClock.now();
    // Within a day, and not much sooner: the broker's cap, less a minute of slack.
    let cap = now + SignedDuration::from_secs(24 * 60 * 60);
    let almost = now + SignedDuration::from_secs(24 * 60 * 60 - 60);
    assert!(
        binding.expires_at > almost && binding.expires_at <= cap,
        "the binding ends at the broker's cap, not {:?}",
        binding.expires_at
    );
}

/// The `model-pick/v1` of a mock card that maps each alias to a deployment endpoint label.
fn offering() -> Option<ModelPickOffer> {
    ModelPickOffer::from_params(&json!({
        "choices": [
            {"id": "fast", "label": "Fast", "endpoint": "frontier"},
            {"id": "deep", "label": "Deep", "endpoint": "deep-endpoint"}
        ],
        "default": "fast"
    }))
}

/// A thread of `coder` whose first message carries `pick` (a card is read to check it).
async fn start_picked(w: &World, text: &str, pick: Option<ModelPick>) -> ThreadId {
    let app = w.app();
    let thread = ThreadId(uuid::Uuid::now_v7());
    match app
        .create_thread_as(
            &alice(),
            thread,
            NewThread {
                title: None,
                target: target("coder"),
                text: text.to_owned(),
            },
            Inbound {
                pick,
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    {
        Creation::Created { .. } => thread,
        Creation::Exists => panic!("a fresh id"),
    }
}

/// A message of `alice` that picks `pick`.
fn message(text: &str, pick: Option<ModelPick>) -> orch_core::Input {
    orch_core::Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: orch_core::Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick,
    }
}

/// Spawns the dispatcher with `broker` (no shutdown token: a test stops it by dropping the handle).
fn spawn(app: &Arc<TestApp>, cfg: DispatcherConfig, broker: Broker) -> tokio::task::JoinHandle<()> {
    let token = CancellationToken::new();
    let d = Dispatcher::with_run_bindings(Arc::clone(app), cfg, "d1", broker);
    tokio::spawn(d.run(token))
}

#[tokio::test]
async fn a_run_is_bound_when_its_task_id_is_first_seen_and_before_its_events() {
    let w = World::new();
    let app = w.app();
    let broker = Broker::new();
    let t = create(&app, &alice(), "coder", "bind me").await;
    *broker
        .watched
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some((w.store.clone(), t.id));
    let run = spawn(&app, fast(), broker.clone());
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;

    let binds = broker.binds();
    assert_eq!(binds.len(), 1, "one run, one binding: {binds:?}");
    let binding = &binds[0];
    assert_eq!(binding.run.as_str(), "task-1");
    assert_eq!(binding.agent, AgentId::new("coder"));
    assert_eq!(binding.person, alice());
    assert_eq!(
        binding.model, None,
        "the scripted card lists no endpoint, so the binding names none (ADR 0084, fail closed)"
    );
    broker_deadline_is_a_day(binding);

    // Bound before any of the delegation's events was applied: the log had no agent event when
    // the bind happened.
    let at = broker.bind_seqs();
    assert_eq!(at.len(), 1, "{at:?}");
    let seen = shape(&events(&app, &alice(), t.id).await);
    let before: Vec<&String> = seen.iter().take(usize::try_from(at[0]).unwrap()).collect();
    assert!(
        before.iter().all(|k| !k.starts_with("agent_")),
        "the bind came after an agent event: {before:?}"
    );
    assert!(
        seen.iter().any(|k| k.starts_with("agent_")),
        "the run never reported anything: {seen:?}"
    );
    run.abort();
}

#[tokio::test]
async fn a_call_that_beats_the_bind_parks_and_then_succeeds() {
    let w = World::new();
    // The card maps its aliases to the deployment's endpoints, so the binding names one and the
    // lease can be lent (the status note of 2026-11-24).
    w.agent.set_pick("coder", offering(), None);
    let app = w.app();
    let broker = Broker::new();
    // The `slow` turn stays open until its task is terminal, so the run is still bound when the
    // lookup below is made.
    let _thread = create(&app, &alice(), "coder", "slow job").await;
    let run_id = RunId::new("task-1");
    let caller = RunCaller::Injector;
    // The agent's first model call beats the orchestrator's first frame: the injector's lookup is
    // `run_not_bound`, and adam parks the run and retries (ADR 0067 decision 9).
    assert!(matches!(
        broker.leases.lease(&run_id, &caller).await,
        Err(RunLeaseError::NotBound)
    ));
    let run = spawn(&app, fast(), broker.clone());
    // The first frame has come: the run is bound, and the retry of the call that beat it is
    // answered — the broker knows whose run it is and lends a token for at most one lease.
    eventually("the run is bound", || async {
        broker.leases.binding(&run_id).is_some().then_some(())
    })
    .await;
    let lease = broker
        .leases
        .lease(&run_id, &caller)
        .await
        .expect("the run is bound after its first frame");
    assert_eq!(lease.token().expose(), "key-frontier");
    assert!(lease.expires_at() > SystemClock.now(), "the lease is spent");
    assert_eq!(
        lease.account(),
        &alice(),
        "the lease names the run's person, the ledger's account"
    );
    let binding = broker.leases.binding(&run_id).unwrap();
    assert_eq!(binding.agent, AgentId::new("coder"));
    assert_eq!(binding.person, alice());
    assert_eq!(
        binding.model.as_deref(),
        Some("frontier"),
        "no pick: the card's default choice names the endpoint"
    );
    // The account reaches the broker's lease and not the agent: every A2A message the run sent
    // holds no account id (ADR 0084, amended 2026-11-24).
    let messages: Vec<String> = w
        .agent
        .sends()
        .into_iter()
        .map(|call| format!("{call:?}"))
        .collect();
    assert!(
        !messages.is_empty(),
        "the run sent the agent nothing: there is no message to grep"
    );
    for message in &messages {
        assert!(
            !message.contains(alice().as_str()),
            "the account id is in the A2A message: {message}"
        );
    }
    run.abort();
}

#[tokio::test]
async fn the_picked_alias_binds_the_endpoint_the_live_card_maps_it_to() {
    let w = World::new();
    w.agent.set_pick("coder", offering(), None);
    let app = w.app();
    let broker =
        Broker::with_endpoints([("frontier", "key-frontier"), ("deep-endpoint", "key-deep")]);
    // `slow` keeps the run bound for the lookup below.
    let _thread = start_picked(&w, "slow job", Some(ModelPick::model("deep").unwrap())).await;
    let run = spawn(&app, fast(), broker.clone());

    let binding = eventually("the run is bound", || async {
        broker.leases.binding(&RunId::new("task-1"))
    })
    .await;
    assert_eq!(
        binding.model.as_deref(),
        Some("deep-endpoint"),
        "the picked alias's endpoint, not the card's default one"
    );
    let lease = broker
        .leases
        .lease(&RunId::new("task-1"), &RunCaller::Injector)
        .await
        .expect("the deployment holds a credential for that endpoint");
    assert_eq!(lease.token().expose(), "key-deep");
    assert_eq!(broker.binds()[0].model.as_deref(), Some("deep-endpoint"));
    run.abort();
}

#[tokio::test]
async fn a_message_that_picks_nothing_binds_the_cards_default_choice() {
    let w = World::new();
    w.agent.set_pick("coder", offering(), None);
    let app = w.app();
    let broker = Broker::new();
    let _thread = create(&app, &alice(), "coder", "slow job").await;
    let run = spawn(&app, fast(), broker.clone());

    eventually("the run is bound", || async {
        broker.leases.binding(&RunId::new("task-1"))
    })
    .await;
    assert_eq!(
        broker.binds()[0].model.as_deref(),
        Some("frontier"),
        "no pick anywhere: the card's own default choice"
    );
    run.abort();
}

#[tokio::test]
async fn a_card_without_an_endpoint_binds_no_model_and_the_lookup_is_unavailable() {
    let w = World::new();
    // A choice, and no endpoint on it: the card of today.
    w.agent.set_pick(
        "coder",
        ModelPickOffer::from_params(&json!({"choices": [{"id": "fast"}], "default": "fast"})),
        None,
    );
    let app = w.app();
    let broker = Broker::new();
    let _thread = start_picked(&w, "slow job", Some(ModelPick::model("fast").unwrap())).await;
    let run = spawn(&app, fast(), broker.clone());

    let binding = eventually("the run is bound", || async {
        broker.leases.binding(&RunId::new("task-1"))
    })
    .await;
    assert_eq!(binding.model, None, "no endpoint is invented for the card");
    assert!(
        matches!(
            broker
                .leases
                .lease(&RunId::new("task-1"), &RunCaller::Injector,)
                .await,
            Err(RunLeaseError::Unavailable(_))
        ),
        "a binding that names no endpoint lends nothing (fail closed)"
    );
    run.abort();
}

#[tokio::test]
async fn a_later_message_that_picks_another_alias_is_refused_and_the_run_keeps_its_endpoint() {
    let w = World::new();
    w.agent.set_pick("coder", offering(), None);
    let app = w.app();
    let broker =
        Broker::with_endpoints([("frontier", "key-frontier"), ("deep-endpoint", "key-deep")]);
    // `ask` opens a task and waits for the person, so the follow-up continues the same run.
    let thread = start_picked(
        &w,
        "ask which branch",
        Some(ModelPick::model("fast").unwrap()),
    )
    .await;
    let run = spawn(&app, fast(), broker.clone());
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    let bound = broker
        .leases
        .binding(&RunId::new("task-1"))
        .expect("the first message bound the run");
    assert_eq!(bound.model.as_deref(), Some("frontier"));

    // The same run, another alias: the bind is refused (`409`), never re-pointed.
    app.submit(
        &alice(),
        thread,
        message("main", Some(ModelPick::model("deep").unwrap())),
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    let binds = broker.binds();
    assert_eq!(
        binds.len(),
        2,
        "the follow-up asked to bind the same run again"
    );
    assert_eq!(binds[1].model.as_deref(), Some("deep-endpoint"));
    assert_eq!(
        broker.outcomes(),
        [Ok(()), Err(RunBindingError::Conflict)],
        "the broker refused to re-point the live run"
    );
    run.abort();
}

#[tokio::test]
async fn the_same_pick_on_a_later_message_is_idempotent() {
    let w = World::new();
    w.agent.set_pick("coder", offering(), None);
    let app = w.app();
    let broker = Broker::new();
    let thread = start_picked(
        &w,
        "ask which branch",
        Some(ModelPick::model("fast").unwrap()),
    )
    .await;
    let run = spawn(&app, fast(), broker.clone());
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;

    app.submit(
        &alice(),
        thread,
        message("main", Some(ModelPick::model("fast").unwrap())),
        None,
    )
    .await
    .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    let binds = broker.binds();
    assert_eq!(binds.len(), 2);
    assert_eq!(
        binds.iter().map(|b| b.model.as_deref()).collect::<Vec<_>>(),
        [Some("frontier"), Some("frontier")]
    );
    assert_eq!(broker.outcomes(), [Ok(()), Ok(())]);
    run.abort();
}

#[tokio::test]
async fn a_run_is_unbound_when_the_thread_is_done() {
    let w = World::new();
    let app = w.app();
    let broker = Broker::new();
    let t = create(&app, &alice(), "coder", "finish it").await;
    let run = spawn(&app, fast(), broker.clone());
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    eventually("the run is unbound", || async {
        broker
            .unbinds()
            .iter()
            .any(|r| r.as_str() == "task-1")
            .then_some(())
    })
    .await;
    assert!(
        broker.leases.binding(&RunId::new("task-1")).is_none(),
        "the broker still holds the run"
    );
    run.abort();
}

#[tokio::test]
async fn a_run_that_never_reached_the_agent_is_never_bound() {
    let w = World::new();
    let app = w.app();
    let broker = Broker::new();
    // The agent refuses the send: the row is dead-lettered and the job fails.
    w.agent
        .fail_next_sends(1, || AgentError::Rejected("scripted failure".to_owned()));
    let t = create(&app, &alice(), "coder", "fail it").await;
    let run = spawn(&app, fast(), broker.clone());
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    // Nothing reached the agent, so no run was ever opened: nothing to unbind, and no error.
    assert!(broker.binds().is_empty(), "the send never reached an agent");
    assert!(broker.unbinds().is_empty());
    run.abort();
}

#[tokio::test]
async fn a_run_is_unbound_when_the_task_fails() {
    let w = World::new();
    let app = w.app();
    let broker = Broker::new();
    // The scripted turn opens a task and reports it failed.
    let t = create(&app, &alice(), "coder", "failed").await;
    let run = spawn(&app, fast(), broker.clone());
    wait_state(&app, &alice(), t.id, ThreadState::Failed).await;
    eventually("the run is unbound", || async {
        broker
            .unbinds()
            .iter()
            .any(|r| r.as_str() == "task-1")
            .then_some(())
    })
    .await;
    run.abort();
}

#[tokio::test]
async fn a_run_is_unbound_when_the_thread_is_cancelled() {
    let w = World::new();
    let app = w.app();
    let broker = Broker::new();
    let run_id = RunId::new("task-1");
    // The `slow` script keeps the turn open until the task is terminal, so the person's stop
    // reaches a running task (as `cancel_a_running_task_reaches_the_agent` does).
    let t = create(&app, &alice(), "coder", "slow job").await;
    let run = spawn(&app, fast(), broker.clone());
    // A stop reaches a *running* task, as in `cancel_a_running_task_reaches_the_agent`: wait for
    // the turn to be working (the bind has happened by then, on the first frame) before stopping.
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    eventually("the run is bound", || async {
        broker.leases.binding(&run_id).is_some().then_some(())
    })
    .await;
    app.cancel(&alice(), t.id).await.unwrap();
    wait_state(&app, &alice(), t.id, ThreadState::Cancelled).await;
    eventually("the run is unbound", || async {
        broker
            .unbinds()
            .iter()
            .any(|r| r.as_str() == "task-1")
            .then_some(())
    })
    .await;
    run.abort();
}

#[tokio::test]
async fn an_unbind_is_idempotent() {
    let broker = Broker::new();
    let run = RunId::new("task-1");
    broker
        .leases
        .bind(RunBinding {
            run: run.clone(),
            agent: AgentId::new("coder"),
            person: alice(),
            model: None,
            expires_at: SystemClock.now() + SignedDuration::from_secs(60),
        })
        .await
        .unwrap();
    assert_eq!(broker.unbind(&run).await, Ok(()));
    // The end of a run the dispatcher has already unbound, or one it never bound, is no error.
    assert_eq!(broker.unbind(&run).await, Ok(()));
    assert_eq!(broker.unbind(&RunId::new("task-never-bound")).await, Ok(()));
}

#[tokio::test]
async fn an_unreachable_broker_does_not_park_the_run_today() {
    let w = World::new();
    let app = w.app();
    let broker = Broker {
        unreachable: Arc::new(AtomicBool::new(true)),
        ..Broker::new()
    };
    let t = create(&app, &alice(), "coder", "the broker is down").await;
    let run = spawn(&app, fast(), broker.clone());
    w.agent.release_gate();
    // The bind failed, and the run went on: S9 turns this into a parking run.
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    assert_eq!(broker.binds().len(), 1, "the bind was attempted");
    assert!(broker.leases.binding(&RunId::new("task-1")).is_none());
    run.abort();
}
