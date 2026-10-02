//! The agent registry through the thread service and the dispatcher (ADR 0022): the agents a
//! person can pick are read live, a registry that cannot be read leaves the static agents and
//! says so, and a delegation waits for it rather than dying of it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{App, AppConfig, AppError, Dispatcher, NewThread};
use orch_core::{AgentId, AgentSource, AgentTarget, Classify, ErrorClass, EventBody, ThreadState};
use orch_ports::memory::{
    MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    AgentEndpoint, CompositeRegistry, FixedRegistry, OutboxStatus, PortSet, RegistryEntry,
    SystemClock, ThreadStore,
};
use support::*;
use tokio_util::sync::CancellationToken;

type RegistryPorts = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    ScriptedModel,
    CompositeRegistry<FixedRegistry, MemoryRegistry>,
>;
type RegistryApp = App<RegistryPorts>;

/// The deployment's static agents (`coder`, `plain`) in front of a registry the test changes.
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

/// An agent the platform lists: `<id>` at a card URL of its own, with tags.
fn platform_agent(id: &str, name: &str, tags: &[&str]) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        origin: AgentSource::Registry,
    }
}

fn spawn(app: &Arc<RegistryApp>) -> (tokio::task::JoinHandle<()>, CancellationToken) {
    let token = CancellationToken::new();
    let dispatcher = Dispatcher::new(Arc::clone(app), fast(), "d1");
    (tokio::spawn(dispatcher.run(token.clone())), token)
}

fn new_thread(agent: &str, text: &str) -> NewThread {
    NewThread {
        title: None,
        target: AgentTarget {
            agent_id: AgentId::new(agent),
            release: None,
        },
        text: text.to_owned(),
    }
}

async fn state(w: &World, id: orch_core::ThreadId) -> ThreadState {
    w.store.get_thread(None, id).await.unwrap().unwrap().state
}

#[tokio::test]
async fn an_agent_the_platform_adds_is_listed_and_can_be_picked_without_a_restart() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    let app = app_over(&w, &registry);
    let before = app.list_agents(&alice()).await.unwrap();
    assert_eq!(
        before
            .agents
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["coder", "plain"]
    );
    assert!(
        app.describe_agent(&alice(), &AgentId::new("helper"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        app.create_thread(&alice(), new_thread("helper", "echo hi")).await,
        Err(AppError::Invalid(m)) if m == "unknown agent 'helper'"
    ));

    registry.add(platform_agent("helper", "Helper", &["writing", "docs"]));

    let after = app.list_agents(&alice()).await.unwrap();
    assert_eq!(
        after
            .agents
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["coder", "plain", "helper"],
        "the static agents come first: the default does not change"
    );
    let helper = &after.agents[2];
    assert_eq!(helper.name, "Helper");
    assert_eq!(helper.source, AgentSource::Registry);
    assert_eq!(helper.tags, ["writing", "docs"]);
    assert_eq!(
        helper.card_url.as_deref(),
        Some("https://helper.agents.example.com/.well-known/agent-card.json")
    );
    assert_eq!(after.agents[0].source, AgentSource::Static);
    assert!(after.sources.iter().all(|s| s.available));
    assert_eq!(after.sources.len(), 2, "the static list and the registry");
    assert_eq!(
        app.default_agent(&alice()).await,
        Some(AgentId::new("coder"))
    );
    assert!(
        app.describe_agent(&alice(), &AgentId::new("helper"))
            .await
            .unwrap()
            .is_some()
    );

    // And it can be targeted, and served by the dispatcher.
    let (handle, token) = spawn(&app);
    let t = app
        .create_thread(&alice(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    eventually("the thread is done", || async {
        (state(&w, t.id).await == ThreadState::Done).then_some(())
    })
    .await;
    token.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_registry_that_is_down_leaves_the_static_agents_and_says_so() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(platform_agent("helper", "Helper", &[]));
    let app = app_over(&w, &registry);
    assert_eq!(app.list_agents(&alice()).await.unwrap().agents.len(), 3);

    registry.set_down(true);
    let list = app.list_agents(&alice()).await.unwrap();
    assert_eq!(
        list.agents
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["coder", "plain"],
        "none of the registry's agents, however recently they were listed"
    );
    let down: Vec<_> = list.sources.iter().filter(|s| !s.available).collect();
    assert_eq!(down.len(), 1, "{:?}", list.sources);
    assert_eq!(app.registry_sources(&alice()).await.unwrap(), list.sources);
    // The static agents are still found and targeted; the registry's cannot be said to exist.
    assert!(
        app.resolve_agent(&AgentId::new("coder"))
            .await
            .unwrap()
            .is_some()
    );
    app.create_thread(&alice(), new_thread("plain", "echo hi"))
        .await
        .unwrap();
    for who in ["helper", "nobody"] {
        let err = app.resolve_agent(&AgentId::new(who)).await.unwrap_err();
        assert!(
            matches!(err, AppError::RegistryUnavailable { .. }),
            "{who}: {err:?}"
        );
        assert_eq!(err.class(), ErrorClass::Transient);
        let err = app
            .create_thread(&alice(), new_thread(who, "echo hi"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::RegistryUnavailable { .. }),
            "{who}: a registry that cannot say is not \"unknown agent\": {err:?}"
        );
        assert!(matches!(
            app.describe_agent(&alice(), &AgentId::new(who)).await,
            Err(AppError::RegistryUnavailable { .. })
        ));
    }

    registry.set_down(false);
    assert_eq!(app.list_agents(&alice()).await.unwrap().agents.len(), 3);
    assert!(
        app.list_agents(&alice())
            .await
            .unwrap()
            .sources
            .iter()
            .all(|s| s.available)
    );
}

#[tokio::test]
async fn a_delegation_waits_while_the_registry_is_down_and_goes_through_when_it_is_back() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(platform_agent("helper", "Helper", &[]));
    let app = app_over(&w, &registry);
    let t = app
        .create_thread(&alice(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    let row = w.store.list_open_outbox(t.id).await.unwrap()[0].id;

    // The registry goes down after the message was accepted, before it was delivered.
    registry.set_down(true);
    let (handle, token) = spawn(&app);
    let waiting = eventually("the row has been tried again", || async {
        let r = w.store.get_outbox(row).await.unwrap().unwrap();
        (r.attempts >= 3).then_some(r)
    })
    .await;
    assert_ne!(
        waiting.status,
        OutboxStatus::Dead,
        "a registry that is down is not an agent that is gone"
    );
    let operator = waiting.last_error.unwrap_or_default();
    assert!(
        operator.contains("the agent registry is unreachable"),
        "{operator}"
    );
    assert_eq!(
        state(&w, t.id).await,
        ThreadState::Queued,
        "the thread waits"
    );
    assert!(w.agent.sends().is_empty(), "nothing reached an agent");

    registry.set_down(false);
    eventually("the thread is done", || async {
        (state(&w, t.id).await == ThreadState::Done).then_some(())
    })
    .await;
    assert_eq!(w.agent.sends().len(), 1);
    token.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn a_delegation_to_an_agent_the_registry_no_longer_lists_is_dead_lettered() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(platform_agent("helper", "Helper", &[]));
    let app = app_over(&w, &registry);
    let t = app
        .create_thread(&alice(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    let row = w.store.list_open_outbox(t.id).await.unwrap()[0].id;

    // The platform removes the agent; the registry answers, and says it is not there.
    registry.remove(&AgentId::new("helper"));
    let (handle, token) = spawn(&app);
    eventually("the thread failed", || async {
        (state(&w, t.id).await == ThreadState::Failed).then_some(())
    })
    .await;
    let events = app.list_events(&alice(), t.id, 0, 100).await.unwrap();
    let message = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.message.clone()),
            _ => None,
        })
        .expect("the thread says why");
    assert_eq!(message, "agent 'helper' is no longer listed");
    let dead = w.store.get_outbox(row).await.unwrap().unwrap();
    assert_eq!(dead.status, OutboxStatus::Dead);
    assert!(w.agent.sends().is_empty());
    token.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn the_static_entry_wins_over_a_registry_agent_with_the_same_id() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(platform_agent("coder", "Platform coder", &["coding"]));
    let app = app_over(&w, &registry);
    let list = app.list_agents(&alice()).await.unwrap();
    assert_eq!(
        list.agents
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>(),
        ["coder", "plain"]
    );
    assert_eq!(list.agents[0].name, "Coder");
    assert_eq!(list.agents[0].source, AgentSource::Static);
    assert!(list.agents[0].tags.is_empty());
    let entry = app
        .resolve_agent(&AgentId::new("coder"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.origin, AgentSource::Static);
}
