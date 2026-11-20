//! A person's start and close of a helper session when the platform's registry stops listing the
//! agent (ADR 0072, S3): the row is no delegation, so it does not fail the job. The start ends
//! `failed` with a word and the close is refused with a word; both are the person's to see.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{App, AppConfig, Dispatcher, NewThread, StartChildSession};
use orch_core::{
    AgentId, AgentSource, AgentTarget, ChildSessionsOffer, EventBody, EventKind, KnownExtension,
    SessionState, StartOutcomeKind, ThreadId, ThreadState,
};
use orch_ports::memory::{
    MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    AgentAudience, AgentEndpoint, CompositeRegistry, FixedRegistry, PortSet, Principal,
    RegistryEntry, SystemClock, ThreadStore,
};
use serde_json::json;
use support::*;
use tokio_util::sync::CancellationToken;

type RegistryApp = App<
    PortSet<
        MemoryStore,
        MemoryWakeup,
        ScriptedAgent,
        SystemClock,
        SeqIds,
        ScriptedModel,
        CompositeRegistry<FixedRegistry, MemoryRegistry>,
    >,
>;

fn app_over(w: &World, registry: &MemoryRegistry) -> Arc<RegistryApp> {
    let directory = directory();
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
                registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn listed(id: &str) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
        audience: AgentAudience::Everyone,
    }
}

fn person() -> Principal {
    Principal::of(alice())
}

fn start(request: &str, keep_open: bool) -> StartChildSession {
    StartChildSession {
        request: request.into(),
        agent: "planner".into(),
        description: Some("Find the writers".into()),
        text: "echo x".into(),
        keep_open,
        tier: None,
    }
}

struct Rig {
    w: World,
    registry: MemoryRegistry,
    app: Arc<RegistryApp>,
}

fn rig() -> Rig {
    let w = World::new();
    w.agent.set_child_sessions(
        "helper",
        Some(
            ChildSessionsOffer::from_params(&json!({
                "agents": [{"name": "planner", "description": "Plans"}],
                "maxOpen": 16
            }))
            .unwrap(),
        ),
    );
    w.agent.set_extensions("helper", &[KnownExtension::Steer]);
    let registry = MemoryRegistry::new();
    registry.add(listed("helper"));
    let app = app_over(&w, &registry);
    Rig { w, registry, app }
}

impl Rig {
    fn run(&self, owner: &str) -> (CancellationToken, tokio::task::JoinHandle<()>) {
        let token = CancellationToken::new();
        let handle =
            tokio::spawn(Dispatcher::new(Arc::clone(&self.app), fast(), owner).run(token.clone()));
        (token, handle)
    }

    async fn state(&self, id: ThreadId) -> ThreadState {
        self.w
            .store
            .get_thread(None, id)
            .await
            .unwrap()
            .unwrap()
            .state
    }

    async fn done(&self, id: ThreadId) {
        eventually("done", || async {
            (self.state(id).await == ThreadState::Done).then_some(())
        })
        .await;
    }

    async fn parent(&self) -> ThreadId {
        let t = self
            .app
            .create_thread(
                &person(),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("helper"),
                        release: None,
                    },
                    text: "echo hi".to_owned(),
                },
            )
            .await
            .unwrap();
        let (token, handle) = self.run("d0");
        self.done(t.id).await;
        token.cancel();
        handle.await.unwrap();
        t.id
    }
}

#[tokio::test]
async fn a_start_for_an_agent_the_registry_dropped_ends_failed_and_the_job_is_not() {
    let r = rig();
    let parent = r.parent().await;
    r.app
        .start_child_session(&person(), parent, start("req-1", false))
        .await
        .unwrap();
    r.registry.remove(&AgentId::new("helper"));
    let (token, handle) = r.run("d1");
    let ev = eventually("the start's end", || async {
        let ev = r.app.list_events(&person(), parent, 0, 500).await.unwrap();
        ev.iter()
            .any(|e| e.kind() == EventKind::ChildSessionStartEnded)
            .then_some(ev)
    })
    .await;
    token.cancel();
    handle.await.unwrap();
    let ended = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ChildSessionStartEnded(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(ended.outcome, StartOutcomeKind::Failed);
    assert_eq!(
        ended.reason.as_deref(),
        Some("agent 'helper' is no longer listed")
    );
    // the job is not failed: the thread is as it was, the pending start drained, the row is dead
    assert_eq!(r.state(parent).await, ThreadState::Done);
    assert!(
        !ev.iter().any(|e| e.kind() == EventKind::Error),
        "the job did not fail"
    );
    let record = r.app.get_thread(&person(), parent).await.unwrap();
    assert!(record.job.session_starts.is_empty());
    let rows = r.w.store.list_open_outbox(parent).await.unwrap();
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn a_close_for_an_agent_the_registry_dropped_is_refused_with_a_word_and_may_be_asked_again() {
    let r = rig();
    let parent = r.parent().await;
    r.app
        .start_child_session(&person(), parent, start("req-1", true))
        .await
        .unwrap();
    let (token, handle) = r.run("d1");
    let kid = eventually("the helper", || async {
        r.w.store
            .child_threads(&alice(), parent)
            .await
            .unwrap()
            .into_iter()
            .next()
    })
    .await;
    r.done(kid.id).await;
    token.cancel();
    handle.await.unwrap();

    r.app
        .close_child_session(&person(), kid.id, false)
        .await
        .unwrap();
    r.registry.remove(&AgentId::new("helper"));
    let (token, handle) = r.run("d2");
    let ev = eventually("the refusal", || async {
        let ev = r.app.list_events(&person(), kid.id, 0, 500).await.unwrap();
        ev.iter()
            .any(|e| e.kind() == EventKind::Error)
            .then_some(ev)
    })
    .await;
    token.cancel();
    handle.await.unwrap();
    let error = ev
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(error.message, "agent 'helper' is no longer listed");
    // the session is as it was: not closed, no close asked, and the thread did not fail
    let record = r.app.get_thread(&person(), kid.id).await.unwrap();
    let child = record.job.child.unwrap();
    assert_ne!(child.state, SessionState::Closed);
    assert!(!child.close_asked, "the ask was cleared");
    assert_eq!(record.state, ThreadState::Done);
    let rows = r.w.store.list_open_outbox(kid.id).await.unwrap();
    assert!(rows.is_empty(), "{rows:?}");
}

#[tokio::test]
async fn a_start_for_an_agent_the_registry_no_longer_lists_is_not_found_and_writes_nothing() {
    let r = rig();
    let parent = r.parent().await;
    let before = r
        .app
        .list_events(&person(), parent, 0, 500)
        .await
        .unwrap()
        .len();
    r.registry.remove(&AgentId::new("helper"));
    let e = r
        .app
        .start_child_session(&person(), parent, start("req-1", false))
        .await
        .unwrap_err();
    assert!(matches!(e, orch_app::AppError::NotFound), "{e:?}");
    assert_eq!(
        r.app
            .list_events(&person(), parent, 0, 500)
            .await
            .unwrap()
            .len(),
        before
    );
    assert!(r.w.store.list_open_outbox(parent).await.unwrap().is_empty());
}
