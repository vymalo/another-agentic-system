//! The title worker's model path, as the deployment runs it (card ANR-670): the injector in front
//! of the model endpoint, and the broker the worker binds a run with.
//!
//! A title is asked outside any run, by the outbox worker, so the worker binds the call's run
//! itself — the thread's agent, the thread's **owner** as the person, and the title task's endpoint
//! as the model — and sends that run id with the request. The injector resolves the lease by that
//! id and stamps `x-account-id` with the lease's account, which is the person's `UserId` (ADR 0065,
//! ADR 0084): the account of a title is the thread's owner, never a constant and never the person
//! who asked for the title.
//!
//! [`Injector`] is the injector as the worker meets it: it reads the run id's lease from the
//! broker's own in-memory leases and records the account it would stamp; a call with no run id is
//! the injector's `403`, a person with nothing left to spend is its `402 model_locked`, and in both
//! cases the model behind it is never asked — its journal (`ScriptedModel::calls`) is the proof.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use orch_app::{App, AppConfig, Dispatcher, NewThread, TaskSettings};
use orch_core::{TaskKind, ThreadId, ThreadRecord, ThreadState, UserId};
use orch_ports::memory::{
    MemoryRunLeases, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ChatModel, ChatRequest, ModelError, PortSet, RunBinding, RunBindingError, RunBindings,
    RunCaller, RunId, RunLeases, SystemClock, ThreadStore,
};
use tokio_util::sync::CancellationToken;

use super::{World, directory, eventually, fast, target};

/// The deployment's model endpoint of the title task, by the name the lease is looked up by.
pub const ENDPOINT: &str = "default";
/// The token the broker lends for that endpoint (the provider key the injector adds upstream).
pub const LEASE_TOKEN: &str = "key-from-the-broker";

/// The injector and the model behind it, as the title worker meets them.
///
/// `complete` is the injector: the caller's run id names the lease (a request without one, or for
/// a run nobody bound, is refused `403` before the gateway); the lease's account is the person the
/// call is charged to; a person in `at_zero` is refused `402 model_locked`. Only a call that
/// passes all of that reaches the model — the journal of `calls` is the gateway's.
#[derive(Clone)]
pub struct Injector {
    inner: ScriptedModel,
    leases: Arc<MemoryRunLeases>,
    at_zero: BTreeSet<UserId>,
    /// The account the injector stamped for each call it let through, in order.
    stamped: Arc<Mutex<Vec<UserId>>>,
    /// The key the lease handed out for each call, in order: what `Authorization` carried.
    keys: Arc<Mutex<Vec<String>>>,
    /// Every question the worker sent to the injector, refused or not.
    asked: Arc<AtomicUsize>,
}

impl Injector {
    pub fn new(inner: ScriptedModel, leases: Arc<MemoryRunLeases>) -> Self {
        Injector {
            inner,
            leases,
            at_zero: BTreeSet::new(),
            stamped: Arc::default(),
            keys: Arc::default(),
            asked: Arc::default(),
        }
    }

    pub fn with_zero(mut self, who: UserId) -> Self {
        self.at_zero.insert(who);
        self
    }

    pub fn stamped(&self) -> Vec<UserId> {
        self.stamped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }

    pub fn keys(&self) -> Vec<String> {
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl ChatModel for Injector {
    async fn complete(&self, request: &ChatRequest) -> Result<String, ModelError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let Some(run) = &request.run else {
            // The authorizer refuses a request with no X-Run-Id before the broker is asked.
            return Err(ModelError::Unauthenticated);
        };
        // The injector's own lookup: it may read a lease, an agent may not (ADR 0084).
        let Ok(lease) = self.leases.lease(run, &RunCaller::Injector).await else {
            // `404 run_not_bound` / `410 lease_expired`: a refusal, never an empty account id.
            return Err(ModelError::Unauthenticated);
        };
        let account = lease.account().clone();
        self.stamped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(account.clone());
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(lease.token().expose().to_owned());
        if self.at_zero.contains(&account) {
            // The Envoy filter's answer for a spent balance, before the model endpoint.
            return Err(ModelError::Rejected("402: model_locked".to_owned()));
        }
        self.inner.complete(request).await
    }
}

/// The broker, as the worker sees it: the run bindings it sends, and the unbinds.
#[derive(Clone)]
pub struct Broker {
    leases: Arc<MemoryRunLeases>,
    binds: Arc<Mutex<Vec<RunBinding>>>,
    unbinds: Arc<Mutex<Vec<RunId>>>,
}

impl Broker {
    pub fn new(leases: Arc<MemoryRunLeases>) -> Self {
        Broker {
            leases,
            binds: Arc::default(),
            unbinds: Arc::default(),
        }
    }

    pub fn binds(&self) -> Vec<RunBinding> {
        self.binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn unbinds(&self) -> Vec<RunId> {
        self.unbinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl RunBindings for Broker {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunBindingError> {
        let outcome = self
            .leases
            .bind(binding.clone())
            .await
            .map_err(|_| RunBindingError::Conflict);
        self.binds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(binding);
        outcome
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunBindingError> {
        let outcome = self.leases.unbind(run).await;
        self.unbinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(run.clone());
        outcome.map_err(|_| RunBindingError::Unreachable)
    }
}

/// The in-memory stack with the injector as the deployment's model path.
pub type BillingPorts =
    PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds, Injector>;
pub type TestApp = App<BillingPorts>;

/// The one model path of these tests: a `title` task at [`ENDPOINT`], timed out fast.
pub fn app_with(w: &World, model: Injector) -> Arc<TestApp> {
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model,
                auth: orch_ports::RefuseAll,
                registry: directory().fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory(),
            AppConfig {
                stream_poll: Duration::from_millis(100),
                tasks: BTreeMap::from([(
                    TaskKind::Title,
                    TaskSettings::new(TaskKind::Title, ENDPOINT, "mock-title")
                        .with_timeout(Duration::from_millis(300)),
                )]),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

pub struct Running {
    pub handle: tokio::task::JoinHandle<()>,
    pub token: CancellationToken,
}

impl Running {
    pub async fn shutdown(self) {
        self.token.cancel();
        self.handle.await.unwrap();
    }
}

pub fn spawn(app: &Arc<TestApp>, broker: Broker) -> Running {
    let token = CancellationToken::new();
    let d = Dispatcher::with_run_bindings(Arc::clone(app), fast(), "d1", broker);
    Running {
        handle: tokio::spawn(d.run(token.clone())),
        token,
    }
}

/// Waits until no outbox row of the thread is left to work.
pub async fn quiet(w: &World, id: ThreadId) {
    eventually("the outbox to be worked", || async {
        w.store
            .list_open_outbox(id)
            .await
            .unwrap()
            .is_empty()
            .then_some(())
    })
    .await;
}

/// Creates a thread of `agent` for `user`: the application's own entry point, not the support
/// module's, whose app is the scripted model's.
pub async fn create(app: &TestApp, user: &UserId, agent: &str, text: &str) -> ThreadRecord {
    app.create_thread(
        user,
        NewThread {
            title: None,
            target: target(agent),
            text: text.to_owned(),
        },
    )
    .await
    .unwrap()
}

/// Polls the thread until it is in `want`.
pub async fn wait_state(
    app: &TestApp,
    user: &UserId,
    id: ThreadId,
    want: ThreadState,
) -> ThreadRecord {
    eventually("a thread state", || async {
        let t = app.get_thread(user, id).await.unwrap();
        (t.state == want).then_some(t)
    })
    .await
}
