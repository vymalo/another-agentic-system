//! Agent aliases (ADR 0049): an agent answers to other names, is listed and creates threads under
//! its own id only, and a thread created under an alias before the rename keeps working.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, AppError, Creation, Inbound, NewThread,
};
use orch_core::{AgentId, AgentTarget, Mention, ThreadId};
use orch_ports::memory::ScriptedModel;
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use support::*;

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

/// `adam`, also called `coder`, and `plain`.
fn aliased() -> AgentDirectory {
    AgentDirectory::new(vec![entry("adam", "Adam"), entry("plain", "Plain")])
        .with_aliases([(AgentId::new("coder"), AgentId::new("adam"))])
}

fn app(w: &World) -> Arc<TestApp> {
    let directory = aliased();
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
                registry: directory.fixed_registry(),
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

#[test]
fn the_directory_resolves_an_alias_and_names_the_aliases_of_an_agent() {
    let directory = aliased();
    let coder = AgentId::new("coder");
    let adam = AgentId::new("adam");
    assert_eq!(directory.canonical(&coder), adam);
    assert_eq!(directory.canonical(&adam), adam);
    assert_eq!(
        directory.canonical(&AgentId::new("ghost")),
        AgentId::new("ghost")
    );
    assert_eq!(directory.get(&coder).unwrap().name, "Adam");
    assert_eq!(directory.aliases_of(&adam), [coder]);
    assert!(directory.aliases_of(&AgentId::new("plain")).is_empty());
    // an alias of an agent that is not configured is dropped
    let none = AgentDirectory::new(Vec::new())
        .with_aliases([(AgentId::new("coder"), AgentId::new("adam"))]);
    assert!(none.aliases().is_empty());
}

#[tokio::test]
async fn the_listing_has_only_the_canonical_id_and_says_its_aliases() {
    let w = World::new();
    let app = app(&w);
    let listing = app.list_agents(&alice()).await.unwrap();
    let ids: Vec<_> = listing.agents.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["adam", "plain"]);
    assert_eq!(listing.agents[0].aliases, [AgentId::new("coder")]);
    assert!(listing.agents[1].aliases.is_empty());
    // a lookup by the alias is the agent
    let described = app
        .describe_agent(&alice(), &AgentId::new("coder"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(described.id.as_str(), "adam");
    assert_eq!(
        app.resolve_agent(&AgentId::new("coder"))
            .await
            .unwrap()
            .unwrap()
            .id()
            .as_str(),
        "adam"
    );
}

#[tokio::test]
async fn a_thread_started_on_an_alias_is_created_with_the_canonical_id() {
    let w = World::new();
    let app = app(&w);
    let thread = app
        .create_thread(&alice(), new_thread("coder", "hello"))
        .await
        .unwrap();
    assert_eq!(thread.target.agent_id.as_str(), "adam");
}

#[tokio::test]
async fn a_mention_of_an_alias_is_written_as_the_agent() {
    let w = World::new();
    let app = app(&w);
    let id = ThreadId(uuid::Uuid::now_v7());
    let text = "ask @coder please";
    let Creation::Created { thread, .. } = app
        .create_thread_as(
            &alice(),
            id,
            new_thread("plain", text),
            Inbound {
                mentions: vec![Mention {
                    agent_id: AgentId::new("coder"),
                    label: "@coder".to_owned(),
                    start: 4,
                    end: 10,
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
    assert_eq!(thread.target.agent_id.as_str(), "plain");
    let events = app.list_events(&alice(), id, 0, 50).await.unwrap();
    let mentioned: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.body {
            orch_core::EventBody::UserMessage(m) => Some(m.mentions.clone()),
            _ => None,
        })
        .flatten()
        .map(|m| (m.agent_id.to_string(), m.label))
        .collect();
    assert_eq!(mentioned, [("adam".to_owned(), "@coder".to_owned())]);
}

#[tokio::test]
async fn a_thread_created_before_the_rename_keeps_working() {
    let w = World::new();
    // the deployment from before the rename: the agent is `coder`, and its log will say so
    let old_directory = AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")]);
    let old_app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: ScriptedModel::default(),
                auth: orch_ports::RefuseAll,
                registry: old_directory.fixed_registry(),
            },
            old_directory,
            AppConfig::default(),
        )
        .unwrap(),
    );
    let old = old_app
        .create_thread(&alice(), new_thread("coder", "hello"))
        .await
        .unwrap();
    assert_eq!(old.target.agent_id.as_str(), "coder");

    // the same store, served by the renamed deployment
    let app = app(&w);
    let thread = app.get_thread(&alice(), old.id).await.unwrap();
    assert_eq!(
        thread.target.agent_id.as_str(),
        "coder",
        "a thread keeps its id"
    );
    // the person may still write to it: `agent.invoke` is asked about the agent the alias names,
    // and the registry finds the agent
    let sent = app.post_message(&alice(), old.id, "again".to_owned()).await;
    assert!(
        !matches!(sent, Err(AppError::Forbidden { .. } | AppError::NotFound)),
        "{sent:?}"
    );
    assert!(
        app.resolve_agent(&thread.target.agent_id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        app.create_thread(&alice(), new_thread("ghost", "x")).await,
        Err(AppError::Invalid(_))
    ));
}

#[tokio::test]
async fn a_role_and_a_tool_server_that_name_an_alias_are_about_the_agent() {
    use orch_app::{AgentScope, Permission, Policy, RoleGrant, ToolServerInfo};
    use orch_ports::Role;
    use std::collections::{BTreeMap, BTreeSet};

    let w = World::new();
    let directory = aliased();
    let mut web = ToolServerInfo::new("web", "Web");
    web.agents = Some(vec![AgentId::new("coder")]);
    let grant = RoleGrant {
        permissions: BTreeSet::from([Permission::AgentInvoke]),
        agents: AgentScope::from_patterns(["coder"]),
    };
    let policy = Policy::new(BTreeMap::from([(Role::new("clerk"), grant)]), None).unwrap();
    let app = App::new(
        PortSet {
            artifacts: orch_ports::NoArtifacts,
            store: w.store.clone(),
            wakeup: w.wakeup.clone(),
            agents: w.agent.clone(),
            clock: SystemClock,
            ids: w.ids.clone(),
            model: w.model.clone(),
            auth: orch_ports::RefuseAll,
            registry: directory.fixed_registry(),
        },
        directory,
        AppConfig {
            policy,
            tool_servers: vec![web],
            ..AppConfig::default()
        },
    )
    .unwrap();
    // the role is about `adam`; a request asks with the canonical id
    assert!(app.policy().any_role_may_invoke(&AgentId::new("adam")));
    assert!(!app.policy().any_role_may_invoke(&AgentId::new("plain")));
    // the server is offered for the agent under each of its names: a thread keeps the id it
    // was created with
    let offered = |id: &str| {
        app.attached_for(&AgentId::new(id), &["web".to_owned()])
            .len()
    };
    assert_eq!(
        (offered("adam"), offered("coder"), offered("plain")),
        (1, 1, 0)
    );
}
