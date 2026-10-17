//! Who may use a platform agent (ADR 0045, decision 1): at every check of an agent, a role must
//! grant the permission over the agent **and** the agent's audience must let the person in, and
//! an agent the audience shuts a person out of looks to them like an agent that does not exist.
//!
//! One test per place the ADR names (decision 1.7): the list, one agent, the default agent,
//! starting a thread, sending, forking, a mention, an asked agent (in `ask_call.rs`, which has
//! the harness for it) and attaching a tool server. The predicate itself is tested in
//! `orch_app::authz` and the registry's side in `orch-ports` and `orch-registry-platform`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentScope, App, AppConfig, AppError, Creation, Dispatcher, ForkAt, ForkRequest, Inbound,
    NewThread, Permission, Policy, RoleGrant, ToolServerInfo,
};
use orch_core::{
    AgentId, AgentSource, AgentTarget, Classify, ErrorClass, Input, Mention, Origin, ThreadId,
    ThreadState, UiActionData, UiVersion,
};
use orch_ports::memory::{
    MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    AgentAudience, AgentEndpoint, CompositeRegistry, FixedRegistry, PortSet, Principal,
    RegistryEntry, Role, SystemClock, ThreadListing, ThreadStore,
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
fn app_over(w: &World, registry: &MemoryRegistry, cfg: AppConfig) -> Arc<RegistryApp> {
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
                ..cfg
            },
        )
        .expect("a valid gate"),
    )
}

fn entry(id: &str, audience: AgentAudience) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
        audience,
    }
}

/// `agent.use:<name>`, the convention for an agent's use permission.
fn use_permission(name: &str) -> String {
    format!("agent.use:{name}")
}

fn for_those_holding(name: &str) -> AgentAudience {
    AgentAudience::from_values([use_permission(name)])
}

/// A person who carries these roles in their credential.
fn person(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

fn user() -> Principal {
    person("alice@example.com", &["user"])
}

fn admin() -> Principal {
    person("root@example.com", &["admin"])
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

fn ids(list: &orch_app::AgentList) -> Vec<&str> {
    list.agents.iter().map(|a| a.id.as_str()).collect()
}

/// What a person is told of an agent that does not exist, for the id `id`.
fn unknown(id: &str) -> String {
    format!("unknown agent '{id}'")
}

fn invalid(error: AppError) -> String {
    match error {
        AppError::Invalid(detail) => detail,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn unprocessable(error: AppError) -> String {
    match error {
        AppError::Unprocessable(detail) => detail,
        other => panic!("expected Unprocessable, got {other:?}"),
    }
}

/// The registry of most tests: one agent for each audience.
fn populated() -> MemoryRegistry {
    let registry = MemoryRegistry::new();
    registry.add(entry("open", AgentAudience::Everyone));
    registry.add(entry("mine", for_those_holding("mine")));
    registry.add(entry("theirs", for_those_holding("theirs")));
    registry.add(entry("fresh", AgentAudience::AdminsOnly));
    registry
}

// ---------------------------------------------------------------- the list

#[tokio::test]
async fn the_list_holds_the_agents_whose_audience_lets_the_person_in() {
    let w = World::new();
    let app = app_over(&w, &populated(), AppConfig::default());

    // a user: the static agents, and the registry's that are for everyone
    let list = app.list_agents(&user()).await.unwrap();
    assert_eq!(ids(&list), ["coder", "plain", "open"]);
    assert!(
        list.sources.iter().all(|s| s.available),
        "{:?}",
        list.sources
    );

    // a use permission in the roles claim, which `auth.roles` knows nothing of
    let holder = person("alice@example.com", &["user", &use_permission("mine")]);
    assert_eq!(
        ids(&app.list_agents(&holder).await.unwrap()),
        ["coder", "plain", "open", "mine"]
    );

    // a person with no roles at all is a user by default, and has no value to match
    let bare = orch_core::UserId::new("bare@example.com");
    assert_eq!(
        ids(&app.list_agents(&bare).await.unwrap()),
        ["coder", "plain", "open"]
    );

    // an administrator is shown every agent, the one with no audience included
    assert_eq!(
        ids(&app.list_agents(&admin()).await.unwrap()),
        ["coder", "plain", "open", "mine", "theirs", "fresh"]
    );
}

#[tokio::test]
async fn a_person_without_the_roles_for_an_agent_is_not_shown_it_whatever_its_audience() {
    // `chatter` reads the agents `open` and `mine` only: their audience is not a grant
    let policy = Policy::new(
        BTreeMap::from([(
            Role::new("chatter"),
            RoleGrant {
                agents: AgentScope::from_patterns(["open", "plain"]),
                ..RoleGrant::user()
            },
        )]),
        None,
    )
    .unwrap();
    let w = World::new();
    let app = app_over(
        &w,
        &populated(),
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    let chatter = person("c@example.com", &["chatter", &use_permission("mine")]);
    assert_eq!(
        ids(&app.list_agents(&chatter).await.unwrap()),
        ["plain", "open"],
        "`mine` is in the claim and in the audience, and no role names it"
    );
}

// ---------------------------------------------------------------- one agent

#[tokio::test]
async fn an_agent_the_audience_hides_is_not_described_and_is_the_answer_for_an_agent_that_is_not_there()
 {
    let w = World::new();
    let app = app_over(&w, &populated(), AppConfig::default());
    let describe = |who: &Principal, id: &str| {
        let app = Arc::clone(&app);
        let who = who.clone();
        let id = AgentId::new(id);
        async move { app.describe_agent(&who, &id).await }
    };

    for hidden in ["theirs", "fresh"] {
        let hidden = describe(&user(), hidden).await.unwrap();
        let nowhere = describe(&user(), "nowhere").await.unwrap();
        assert!(hidden.is_none());
        assert_eq!(hidden.is_none(), nowhere.is_none(), "the same answer");
    }
    assert!(describe(&user(), "open").await.unwrap().is_some());
    assert!(describe(&user(), "coder").await.unwrap().is_some());
    // the person it is for, and an administrator
    let holder = person("a@example.com", &["user", &use_permission("theirs")]);
    assert!(describe(&holder, "theirs").await.unwrap().is_some());
    assert!(describe(&admin(), "fresh").await.unwrap().is_some());
}

#[tokio::test]
async fn what_the_roles_refuse_is_refused_before_the_registry_and_the_audience_does_not_change_it()
{
    let policy = Policy::new(
        BTreeMap::from([(
            Role::new("chatter"),
            RoleGrant {
                agents: AgentScope::from_patterns(["open"]),
                ..RoleGrant::user()
            },
        )]),
        None,
    )
    .unwrap();
    let w = World::new();
    let registry = populated();
    let app = app_over(
        &w,
        &registry,
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    let chatter = person("c@example.com", &["chatter"]);
    // the same 403 for an agent that is hidden, one that is not, and one nobody lists: a role's
    // refusal says nothing about the registry
    for id in ["fresh", "mine", "nowhere"] {
        let err = app
            .describe_agent(&chatter, &AgentId::new(id))
            .await
            .unwrap_err();
        assert!(
            matches!(
                &err,
                AppError::Forbidden {
                    permission: Permission::AgentRead,
                    ..
                }
            ),
            "{id}: {err:?}"
        );
    }
    // and it is asked before the registry, which can be down
    registry.set_down(true);
    let err = app
        .describe_agent(&chatter, &AgentId::new("fresh"))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Forbidden { .. }), "{err:?}");
}

#[tokio::test]
async fn a_registry_that_cannot_say_refuses_as_it_did() {
    let w = World::new();
    let registry = populated();
    let app = app_over(&w, &registry, AppConfig::default());
    registry.set_down(true);
    for id in ["open", "fresh", "nowhere"] {
        assert!(matches!(
            app.describe_agent(&user(), &AgentId::new(id)).await,
            Err(AppError::RegistryUnavailable { .. })
        ));
        assert!(matches!(
            app.create_thread(&user(), new_thread(id, "echo hi")).await,
            Err(AppError::RegistryUnavailable { .. })
        ));
    }
    // a static agent needs no registry
    app.describe_agent(&user(), &AgentId::new("coder"))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_id_both_sources_list_is_the_static_one_and_it_has_no_audience() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    // the platform lists `coder` too, for administrators only: the static entry is the one that counts
    registry.add(entry("coder", AgentAudience::AdminsOnly));
    let app = app_over(&w, &registry, AppConfig::default());
    assert_eq!(
        ids(&app.list_agents(&user()).await.unwrap()),
        ["coder", "plain"]
    );
    assert!(
        app.describe_agent(&user(), &AgentId::new("coder"))
            .await
            .unwrap()
            .is_some()
    );
    app.create_thread(&user(), new_thread("coder", "echo hi"))
        .await
        .unwrap();
}

// ---------------------------------------------------------------- the default agent

#[tokio::test]
async fn the_default_agent_is_the_first_one_the_person_may_invoke() {
    // two roles that name only the platform's agents, so the static ones do not come first
    let both = AgentScope::from_patterns(["fresh", "mine", "open"]);
    let policy = Policy::new(
        BTreeMap::from([
            (
                Role::new("user"),
                RoleGrant {
                    agents: both.clone(),
                    ..RoleGrant::user()
                },
            ),
            (
                Role::new("admin"),
                RoleGrant {
                    agents: both,
                    ..RoleGrant::admin()
                },
            ),
        ]),
        Some(Role::new("user")),
    )
    .unwrap();
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("fresh", AgentAudience::AdminsOnly));
    registry.add(entry("mine", for_those_holding("mine")));
    registry.add(entry("open", AgentAudience::Everyone));
    let app = app_over(
        &w,
        &registry,
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    assert_eq!(
        app.default_agent(&admin()).await,
        Some(AgentId::new("fresh")),
        "an administrator is shown the agent with no audience"
    );
    assert_eq!(
        app.default_agent(&user()).await,
        Some(AgentId::new("open")),
        "the first two are not for a user"
    );
    let holder = person("a@example.com", &["user", &use_permission("mine")]);
    assert_eq!(app.default_agent(&holder).await, Some(AgentId::new("mine")));

    registry.remove(&AgentId::new("open"));
    assert_eq!(
        app.default_agent(&user()).await,
        None,
        "none a user may invoke"
    );
}

// ---------------------------------------------------------------- starting a thread

#[tokio::test]
async fn a_thread_cannot_be_started_on_an_agent_the_audience_hides_and_nothing_is_written() {
    let w = World::new();
    let app = app_over(&w, &populated(), AppConfig::default());
    for hidden in ["theirs", "fresh"] {
        let said = invalid(
            app.create_thread(&user(), new_thread(hidden, "echo hi"))
                .await
                .unwrap_err(),
        );
        assert_eq!(said, unknown(hidden));
    }
    // it is what an agent nobody lists is told, word for word
    let nowhere = invalid(
        app.create_thread(&user(), new_thread("nowhere", "echo hi"))
            .await
            .unwrap_err(),
    );
    assert_eq!(nowhere, unknown("nowhere"));
    let listing = ThreadListing::recent(None, 50, false);
    assert!(
        app.list_threads(&user(), listing).await.unwrap().is_empty(),
        "nothing was written"
    );

    // and the same agents are started by the people they are for
    let holder = person("a@example.com", &["user", &use_permission("theirs")]);
    app.create_thread(&holder, new_thread("theirs", "echo hi"))
        .await
        .unwrap();
    app.create_thread(&admin(), new_thread("fresh", "echo hi"))
        .await
        .unwrap();
    app.create_thread(&user(), new_thread("open", "echo hi"))
        .await
        .unwrap();
}

#[tokio::test]
async fn starting_a_thread_on_an_agent_needs_the_roles_as_well_as_the_audience() {
    let policy = Policy::new(
        BTreeMap::from([
            (
                Role::new("chatter"),
                RoleGrant {
                    agents: AgentScope::from_patterns(["open"]),
                    ..RoleGrant::user()
                },
            ),
            (Role::new("admin"), RoleGrant::admin()),
        ]),
        None,
    )
    .unwrap();
    let w = World::new();
    let app = app_over(
        &w,
        &populated(),
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    let chatter = person("c@example.com", &["chatter", &use_permission("mine")]);
    let err = app
        .create_thread(&chatter, new_thread("mine", "echo hi"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            AppError::Forbidden {
                permission: Permission::AgentInvoke,
                ..
            }
        ),
        "the audience names the person and no role names the agent: {err:?}"
    );
    assert_eq!(err.class(), ErrorClass::Forbidden);
}

// ---------------------------------------------------------------- sending: read-only

#[tokio::test]
async fn a_thread_whose_agent_the_person_may_no_longer_use_is_read_only_not_hidden() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("helper", AgentAudience::Everyone));
    let app = app_over(&w, &registry, AppConfig::default());
    let holder = person("alice@example.com", &["user", &use_permission("helper")]);
    let t = app
        .create_thread(&holder, new_thread("helper", "echo hi"))
        .await
        .unwrap();
    app.post_message(&holder, t.id, "echo more".to_owned())
        .await
        .unwrap();

    // the platform narrows the audience to somebody else
    registry.add(entry("helper", for_those_holding("somebody-else")));

    // it is theirs: they read it, in every way
    let read = app.get_thread(&holder, t.id).await.unwrap();
    assert_eq!(read.id, t.id);
    assert_eq!(
        app.list_events(&holder, t.id, 0, 100).await.unwrap().len(),
        2
    );
    assert!(
        app.list_threads(&holder, ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .iter()
            .any(|listed| listed.id == t.id),
        "the thread is still in their list"
    );
    // and what would start the agent's work is a 403, as when `agent.invoke` stops covering it
    let refused = |error: AppError| {
        assert!(
            matches!(&error, AppError::Forbidden { permission: Permission::AgentInvoke, detail }
                if detail == "your roles do not grant agent.invoke for the agent helper"),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Forbidden);
    };
    refused(
        app.post_message(&holder, t.id, "again".to_owned())
            .await
            .unwrap_err(),
    );
    refused(
        app.stop_and_send(&holder, t.id, "stop".to_owned())
            .await
            .unwrap_err(),
    );
    let input = Input::UserMessage {
        user: holder.user.clone(),
        text: "via submit".to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::default(),
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
    };
    refused(app.submit(&holder, t.id, input, None).await.unwrap_err());
    // "Stop & send" and a click on a card start the agent's work as much as a message does, and
    // reach `App::submit` as the inputs a surface translated
    let input = Input::StopAndSend {
        user: holder.user.clone(),
        text: "stop, via submit".to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::default(),
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
    };
    refused(app.submit(&holder, t.id, input, None).await.unwrap_err());
    let input = Input::UiAction {
        user: holder.user.clone(),
        action: UiActionData {
            tier: None,
            tier_cap: None,
            surface_id: "s1".to_owned(),
            name: "go".to_owned(),
            source_component_id: "go".to_owned(),
            context: serde_json::Map::new(),
            version: UiVersion::V0_9_1,
            run_id: None,
        },
        catalog: None,
        tier: None,
        tier_cap: None,
    };
    refused(app.submit(&holder, t.id, input, None).await.unwrap_err());
    assert_eq!(
        app.list_events(&holder, t.id, 0, 100).await.unwrap().len(),
        2,
        "nothing was written"
    );

    // what is not the agent's work is still theirs to do
    app.rename_thread(&holder, t.id, "Kept").await.unwrap();

    // and when the audience takes them in again, they can write
    registry.add(entry("helper", for_those_holding("helper")));
    app.post_message(&holder, t.id, "welcome back".to_owned())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_agent_the_registry_no_longer_lists_is_not_a_refusal_of_the_audience() {
    // there is no audience to keep a person out of: the dispatcher says it is no longer listed
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("helper", AgentAudience::Everyone));
    let app = app_over(&w, &registry, AppConfig::default());
    let t = app
        .create_thread(&user(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    registry.remove(&AgentId::new("helper"));
    app.post_message(&user(), t.id, "echo more".to_owned())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_registry_that_cannot_say_does_not_let_a_message_through_to_a_registry_agent() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("helper", AgentAudience::Everyone));
    let app = app_over(&w, &registry, AppConfig::default());
    let t = app
        .create_thread(&user(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    let plain = app
        .create_thread(&user(), new_thread("plain", "echo hi"))
        .await
        .unwrap();
    registry.set_down(true);
    let err = app
        .post_message(&user(), t.id, "echo more".to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::RegistryUnavailable { .. }),
        "{err:?}"
    );
    // a static agent is not the registry's
    app.post_message(&user(), plain.id, "echo more".to_owned())
        .await
        .unwrap();
}

// ---------------------------------------------------------------- forking

#[tokio::test]
async fn a_fork_onto_an_agent_the_audience_hides_is_refused_as_unknown_and_writes_nothing() {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("helper", AgentAudience::Everyone));
    registry.add(entry("other", for_those_holding("other")));
    let app = app_over(&w, &registry, AppConfig::default());
    let token = CancellationToken::new();
    let dispatcher =
        tokio::spawn(Dispatcher::new(Arc::clone(&app), fast(), "d1").run(token.clone()));
    let t = app
        .create_thread(&user(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    eventually("the first turn to finish", || async {
        (w.store.get_thread(None, t.id).await.unwrap().unwrap().state == ThreadState::Done)
            .then_some(())
    })
    .await;
    token.cancel();
    dispatcher.await.unwrap();

    let fork = |target: Option<&str>| ForkRequest {
        at: ForkAt::AfterTurn {
            seq: 1,
            first: None,
        },
        target: target.map(|agent| AgentTarget {
            agent_id: AgentId::new(agent),
            release: None,
        }),
        id: None,
    };
    let before = app
        .list_threads(&user(), ThreadListing::recent(None, 50, false))
        .await
        .unwrap()
        .len();

    // onto another agent the person may not use
    let said = invalid(
        app.fork_thread(&user(), t.id, fork(Some("other")))
            .await
            .unwrap_err(),
    );
    assert_eq!(said, unknown("other"));
    let nowhere = invalid(
        app.fork_thread(&user(), t.id, fork(Some("nowhere")))
            .await
            .unwrap_err(),
    );
    assert_eq!(nowhere, unknown("nowhere"));

    // and onto the parent's own agent, once the platform took the person out of its audience
    registry.add(entry("helper", for_those_holding("somebody-else")));
    let said = invalid(
        app.fork_thread(&user(), t.id, fork(None))
            .await
            .unwrap_err(),
    );
    assert_eq!(said, unknown("helper"));
    assert_eq!(
        app.list_threads(&user(), ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .len(),
        before,
        "nothing was written"
    );

    // while the audience lets them in, a fork is made
    registry.add(entry("helper", AgentAudience::Everyone));
    let forked = app.fork_thread(&user(), t.id, fork(None)).await.unwrap();
    assert!(forked.created);
}

// ---------------------------------------------------------------- mentions

fn mention_of(agent: &str) -> (String, Vec<Mention>) {
    let label = format!("@{agent}");
    let text = format!("ask {label} please");
    (
        text,
        vec![Mention {
            agent_id: AgentId::new(agent),
            label: label.clone(),
            start: 4,
            end: 4 + u32::try_from(label.len()).unwrap(),
            card_url: None,
            tier_cap: None,
        }],
    )
}

async fn start_with_mention(
    app: &RegistryApp,
    who: &Principal,
    mentioned: &str,
) -> Result<Creation, AppError> {
    let (text, mentions) = mention_of(mentioned);
    app.create_thread_as(
        who,
        ThreadId(uuid::Uuid::now_v7()),
        new_thread("plain", &text),
        Inbound {
            mentions,
            ..Inbound::default()
        },
    )
    .await
}

#[tokio::test]
async fn a_mention_of_an_agent_the_audience_hides_is_the_error_of_an_unknown_agent() {
    let w = World::new();
    let app = app_over(&w, &populated(), AppConfig::default());
    for hidden in ["theirs", "fresh"] {
        let said = unprocessable(start_with_mention(&app, &user(), hidden).await.unwrap_err());
        assert_eq!(said, format!("unknown agent '{hidden}' in mentions"));
    }
    let nowhere = unprocessable(
        start_with_mention(&app, &user(), "nowhere")
            .await
            .unwrap_err(),
    );
    assert_eq!(nowhere, "unknown agent 'nowhere' in mentions");

    // the people the agents are for mention them, and so does anybody for an open one
    let holder = person("a@example.com", &["user", &use_permission("theirs")]);
    start_with_mention(&app, &holder, "theirs").await.unwrap();
    start_with_mention(&app, &admin(), "fresh").await.unwrap();
    start_with_mention(&app, &user(), "open").await.unwrap();
}

#[tokio::test]
async fn a_mention_in_a_message_to_an_existing_thread_is_judged_the_same_way() {
    let w = World::new();
    let app = app_over(&w, &populated(), AppConfig::default());
    let t = app
        .create_thread(&user(), new_thread("plain", "echo hi"))
        .await
        .unwrap();
    let (text, mentions) = mention_of("fresh");
    let input = Input::UserMessage {
        user: user().user.clone(),
        text,
        message_id: None,
        run_id: None,
        origin: Origin::default(),
        catalog: None,
        mentions,
        tier: None,
        tier_cap: None,
    };
    let said = unprocessable(app.submit(&user(), t.id, input, None).await.unwrap_err());
    assert_eq!(said, "unknown agent 'fresh' in mentions");
}

// ---------------------------------------------------------------- attaching a tool server

fn with_a_server(policy: Policy) -> AppConfig {
    AppConfig {
        policy,
        tool_servers: vec![ToolServerInfo::new("docs", "Documentation")],
        ..AppConfig::default()
    }
}

#[tokio::test]
async fn a_server_is_not_attached_for_an_agent_the_person_may_no_longer_use_but_it_can_be_taken_off()
 {
    let w = World::new();
    let registry = MemoryRegistry::new();
    registry.add(entry("helper", AgentAudience::Everyone));
    let app = app_over(&w, &registry, with_a_server(Policy::default()));
    let t = app
        .create_thread(&user(), new_thread("helper", "echo hi"))
        .await
        .unwrap();
    let attached = app
        .set_tools(&user(), t.id, vec!["docs".to_owned()])
        .await
        .unwrap();
    assert_eq!(attached.job.tools, ["docs"]);

    registry.add(entry("helper", for_those_holding("somebody-else")));

    // it is attached for an agent: not for this one, now
    let again = app.set_tools(&user(), t.id, Vec::new()).await.unwrap();
    assert!(
        again.job.tools.is_empty(),
        "taking servers off is still theirs"
    );
    let err = app
        .set_tools(&user(), t.id, vec!["docs".to_owned()])
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            AppError::Forbidden {
                permission: Permission::AgentInvoke,
                ..
            }
        ),
        "{err:?}"
    );
    let unchanged = app.get_thread(&user(), t.id).await.unwrap();
    assert!(unchanged.job.tools.is_empty());

    // for the people the agent is for, and for an agent that does not mind
    registry.add(entry("helper", AgentAudience::Everyone));
    app.set_tools(&user(), t.id, vec!["docs".to_owned()])
        .await
        .unwrap();
}

#[tokio::test]
async fn a_thread_is_started_with_servers_only_on_an_agent_the_person_may_use() {
    let w = World::new();
    let app = app_over(&w, &populated(), with_a_server(Policy::default()));
    let id = ThreadId(uuid::Uuid::now_v7());
    let err = app
        .create_thread_as(
            &user(),
            id,
            new_thread("fresh", "echo hi"),
            Inbound {
                tools: vec!["docs".to_owned()],
                ..Inbound::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid(err), unknown("fresh"));
    assert!(
        app.list_threads(&user(), ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .is_empty()
    );
}

// ---------------------------------------------------------------- the surfaces ask what the person is

#[tokio::test]
async fn find_agent_asks_the_audience_and_not_the_roles() {
    let policy = Policy::new(
        BTreeMap::from([
            (
                Role::new("chatter"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::AgentInvoke, Permission::ThreadWrite]),
                    agents: AgentScope::from_patterns(["open"]),
                    max_tier: None,
                },
            ),
            (Role::new("admin"), RoleGrant::admin()),
        ]),
        None,
    )
    .unwrap();
    let w = World::new();
    let app = app_over(
        &w,
        &populated(),
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    let chatter = person("c@example.com", &["chatter"]);
    let found = |who: &Principal, id: &str| {
        let app = Arc::clone(&app);
        let (who, id) = (who.clone(), AgentId::new(id));
        async move { app.find_agent(&who, &id).await.unwrap().is_some() }
    };
    assert!(found(&chatter, "open").await);
    // the roles do not name `coder`, and that is the act's to refuse, as a 403 and not as "no such agent"
    assert!(found(&chatter, "coder").await);
    // the audience does not name the person
    assert!(!found(&chatter, "fresh").await);
    assert!(!found(&chatter, "nowhere").await);
    assert!(found(&admin(), "fresh").await);
}
