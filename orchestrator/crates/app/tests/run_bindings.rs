//! The dispatcher's side of ADR 0084: a run is bound when its stream first names the task id, and
//! unbound at every end state of its thread (`done`, `failed`, `cancelled`), idempotently. A broker
//! that cannot be reached is logged and the run goes on (S9 parks it).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use jiff::SignedDuration;
use orch_app::{Dispatcher, DispatcherConfig};
use orch_core::{AgentId, ThreadId, ThreadState};
use orch_ports::memory::MemoryRunLeases;
use orch_ports::{
    AgentError, Clock as _, RunBinding, RunBindingError, RunBindings, RunCaller, RunId,
    RunLeaseError, RunLeases, SystemClock, ThreadStore as _, ToolSecret,
};
use support::*;
use tokio_util::sync::CancellationToken;

/// The broker, as the test sees it: the `RunBindings` port the dispatcher holds, over the broker's
/// own in-memory `RunLeases` (the other half of the same wire), so that a test can both read what
/// was sent and ask the injector's lookup what it would answer.
#[derive(Clone, Default)]
struct Broker {
    leases: Arc<MemoryRunLeases>,
    binds: Arc<Mutex<Vec<RunBinding>>>,
    unbinds: Arc<Mutex<Vec<RunId>>>,
    /// The store's view of the thread at the moment of the bind: how far its log had come.
    bind_seqs: Arc<Mutex<Vec<i64>>>,
    /// Where to look the thread up when a bind happens (the ordering test).
    watched: Arc<Mutex<Option<(orch_ports::memory::MemoryStore, ThreadId)>>>,
    /// Every bind is refused as unreachable.
    unreachable: Arc<AtomicBool>,
    /// The model endpoint the deployment names on the binding. Production names none yet (ADR
    /// 0084, open question 4); a test that needs the lease to be lent stands for a deployment
    /// whose composition root knows its endpoint.
    model: Option<String>,
}

impl Broker {
    fn new() -> Self {
        Broker {
            leases: Arc::new(MemoryRunLeases::with_token(
                "default",
                ToolSecret::new("model-key"),
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
        let binding = RunBinding {
            model: self.model.clone().or(binding.model),
            ..binding
        };
        if self.unreachable.load(Ordering::SeqCst) {
            self.record(&binding);
            return Err(RunBindingError::Unreachable);
        }
        // Recorded once the broker has answered, so that a test which sees the call in `binds`
        // also finds the binding where a lookup would: the two cannot disagree.
        let outcome = self.leases.bind(binding.clone()).await;
        self.record(&binding);
        outcome.map_err(|_| RunBindingError::Conflict)
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
        "the orchestrator names no model endpoint of its own (ADR 0084, open question 4)"
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
    let app = w.app();
    // Stands for a deployment that names its model endpoint in the binding (ADR 0084, open
    // question 4): the dispatcher names none yet, so only this half lets the lease be lent.
    let broker = Broker {
        model: Some("default".to_owned()),
        ..Broker::new()
    };
    // The `slow` turn stays open until its task is terminal, so the run is still bound when the
    // lookup below is made.
    let _thread = create(&app, &alice(), "coder", "slow job").await;
    let run_id = RunId::new("task-1");
    let caller = RunCaller::Agent(AgentId::new("coder"));
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
    assert_eq!(lease.token().expose(), "model-key");
    assert!(lease.expires_at() > SystemClock.now(), "the lease is spent");
    let binding = broker.leases.binding(&run_id).unwrap();
    assert_eq!(binding.agent, AgentId::new("coder"));
    assert_eq!(binding.person, alice());
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
