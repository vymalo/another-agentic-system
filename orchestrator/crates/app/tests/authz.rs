//! Roles and permissions on threads, agents and files (ADR 0033): what the application lets a
//! person do with what is theirs and what is not, and what it says when it does not.
//!
//! The rules under test: a thread that is not the person's is `NotFound` (it does not exist for
//! them), whatever their roles, the administrator's included (ADR 0039); a permission their roles
//! lack is `Forbidden` whatever is asked for; an unknown role grants nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_app::{
    Access, AgentScope, App, AppConfig, AppError, FirstMessage, ForkAt, ForkRequest, NewThread,
    Permission, Policy, RoleGrant, Scope,
};
use orch_core::{
    AgentId, AgentTarget, Classify, ErrorClass, EventKind, Input, Origin, ThreadId, ThreadRecord,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ArtifactKey, ArtifactMeta, ArtifactStore, FixedRegistry, PortSet, Principal, Role, SystemClock,
};
use support::*;

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

fn user(email: &str) -> Principal {
    principal(email, &["user"])
}

fn admin(email: &str) -> Principal {
    principal(email, &["admin"])
}

fn role(permissions: &[Permission], agents: &[&str]) -> RoleGrant {
    RoleGrant {
        permissions: permissions.iter().copied().collect::<BTreeSet<_>>(),
        agents: AgentScope::from_patterns(agents),
    }
}

fn policy(roles: &[(&str, RoleGrant)], default_role: Option<&str>) -> Policy {
    Policy::new(
        roles
            .iter()
            .map(|(name, grant)| (Role::new(*name), grant.clone()))
            .collect::<BTreeMap<_, _>>(),
        default_role.map(Role::new),
    )
    .unwrap()
}

fn app_under(w: &World, policy: Policy) -> Arc<TestApp> {
    w.app_with(AppConfig {
        policy,
        stream_poll: std::time::Duration::from_millis(100),
        ..AppConfig::default()
    })
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

async fn started(app: &TestApp, who: &Principal, agent: &str) -> ThreadRecord {
    app.create_thread(who, new_thread(agent, "hello"))
        .await
        .unwrap()
}

fn class(result: Result<impl std::fmt::Debug, AppError>) -> ErrorClass {
    result.unwrap_err().class()
}

/// A refusal that is `Forbidden`, with the permission it names.
fn forbidden<T: std::fmt::Debug>(result: Result<T, AppError>) -> Permission {
    match result.unwrap_err() {
        AppError::Forbidden { permission, .. } => permission,
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

fn is_not_found<T: std::fmt::Debug>(result: Result<T, AppError>) {
    assert!(
        matches!(result, Err(AppError::NotFound)),
        "expected NotFound, got {result:?}"
    );
}

#[tokio::test]
async fn a_user_sees_and_changes_only_their_own_threads() {
    let w = World::new();
    let app = w.app();
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice, "plain").await;
    let id = t.id;

    // Alice may do it all.
    assert_eq!(app.get_thread(&alice, id).await.unwrap().id, id);
    assert!(app.export_thread(&alice, id).await.is_ok());
    assert_eq!(
        app.list_threads(&alice, None, 50, false)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(app.rename_thread(&alice, id, "Mine").await.is_ok());

    // For Bob it does not exist: every read, every act, the same answer as for an id nobody has.
    let nobody = ThreadId(uuid::Uuid::from_u128(0x1234));
    for thread in [id, nobody] {
        is_not_found(app.get_thread(&bob, thread).await);
        is_not_found(app.export_thread(&bob, thread).await);
        is_not_found(app.list_events(&bob, thread, 0, 10).await);
        is_not_found(
            app.latest_events(&bob, thread, EventKind::UserMessage, 1)
                .await,
        );
        is_not_found(app.branches(&bob, thread).await);
        is_not_found(app.event_stream(&bob, thread, 0).await.map(|_| ()));
        is_not_found(app.thread_feed(&bob, thread, 0).await.map(|_| ()));
        is_not_found(app.post_message(&bob, thread, "hi".into()).await);
        is_not_found(app.cancel(&bob, thread).await);
        is_not_found(app.rename_thread(&bob, thread, "Theirs").await);
        is_not_found(app.describe_thread(&bob, thread, "About").await);
        is_not_found(
            app.fork_thread(
                &bob,
                thread,
                ForkRequest {
                    at: ForkAt::AfterTurn {
                        seq: 1,
                        first: None,
                    },
                    target: None,
                    id: None,
                },
            )
            .await,
        );
        is_not_found(
            app.submit(
                &bob,
                thread,
                Input::Cancel {
                    user: bob.user.clone(),
                },
                None,
            )
            .await,
        );
        is_not_found(
            app.open_artifact(&bob, thread, &"0".repeat(64))
                .await
                .map(|_| ()),
        );
    }
    assert!(
        app.list_threads(&bob, None, 50, false)
            .await
            .unwrap()
            .is_empty()
    );
    // And nothing of Bob's attempts changed it.
    assert_eq!(app.get_thread(&alice, id).await.unwrap().title, "Mine");
}

#[tokio::test]
async fn an_administrator_reads_and_changes_only_their_own_threads() {
    // ADR 0039: no role reaches another person's thread, the administrator's included. Alice's
    // thread does not exist for Root, for any read or any act, exactly as for Bob.
    let w = World::new();
    let app = w.app();
    let (alice, root) = (user("alice@example.com"), admin("root@example.com"));
    let t = started(&app, &alice, "plain").await;
    let id = t.id;
    assert!(app.access(&root).has(Permission::Admin));

    let nobody = ThreadId(uuid::Uuid::from_u128(7));
    for thread in [id, nobody] {
        is_not_found(app.get_thread(&root, thread).await);
        is_not_found(app.export_thread(&root, thread).await);
        is_not_found(app.list_events(&root, thread, 0, 10).await);
        is_not_found(
            app.latest_events(&root, thread, EventKind::UserMessage, 1)
                .await,
        );
        is_not_found(app.branches(&root, thread).await);
        is_not_found(app.event_stream(&root, thread, 0).await.map(|_| ()));
        is_not_found(app.thread_feed(&root, thread, 0).await.map(|_| ()));
        is_not_found(
            app.open_artifact(&root, thread, &"0".repeat(64))
                .await
                .map(|_| ()),
        );
        is_not_found(app.post_message(&root, thread, "hi".into()).await);
        is_not_found(app.cancel(&root, thread).await);
        is_not_found(app.rename_thread(&root, thread, "Root's").await);
        is_not_found(app.describe_thread(&root, thread, "About").await);
        is_not_found(
            app.fork_thread(
                &root,
                thread,
                ForkRequest {
                    at: ForkAt::AfterTurn {
                        seq: 1,
                        first: None,
                    },
                    target: None,
                    id: None,
                },
            )
            .await,
        );
        is_not_found(
            app.submit(
                &root,
                thread,
                Input::UserMessage {
                    user: root.user.clone(),
                    text: "hi".into(),
                    message_id: None,
                    run_id: None,
                    origin: Origin::default(),
                    catalog: None,
                    mentions: Vec::new(),
                },
                None,
            )
            .await
            .map(|_| ()),
        );
    }
    // `find_thread` is for surfaces that let the consumer choose ids: a free id is `None`, and
    // another person's thread is `NotFound`, the administrator's included.
    is_not_found(app.find_thread(&root, id).await.map(|_| ()));
    assert!(app.find_thread(&root, nobody).await.unwrap().is_none());
    // The administrator's listing is their own, and Alice's thread is not in it.
    assert!(
        app.list_threads(&root, None, 50, false)
            .await
            .unwrap()
            .is_empty()
    );
    // Nothing of it changed Alice's thread.
    let after = app.get_thread(&alice, id).await.unwrap();
    assert_eq!((after.title, after.last_seq), (t.title.clone(), 1));

    // Their own are theirs, to read and to act on, as a user's are.
    let mine = started(&app, &root, "plain").await;
    assert_eq!(app.get_thread(&root, mine.id).await.unwrap().id, mine.id);
    assert!(
        app.rename_thread(&root, mine.id, "Root's own")
            .await
            .is_ok()
    );
    assert!(app.cancel(&root, mine.id).await.is_ok());
    assert!(
        app.post_message(&root, mine.id, "again".into())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_listing_is_the_callers_own_and_only_theirs() {
    // There is no way to list another person's threads, or everyone's (ADR 0039): `list_threads`
    // takes no owner, for the administrator as for anyone.
    let w = World::new();
    let app = w.app();
    let (alice, bob, root) = (
        user("alice@example.com"),
        user("bob@example.com"),
        admin("root@example.com"),
    );
    let a = started(&app, &alice, "plain").await;
    let b = started(&app, &bob, "plain").await;
    let r = started(&app, &root, "plain").await;
    let r2 = started(&app, &root, "plain").await;
    let ids = |list: Vec<ThreadRecord>| list.into_iter().map(|t| t.id).collect::<Vec<_>>();

    assert_eq!(
        ids(app.list_threads(&alice, None, 50, false).await.unwrap()),
        [a.id]
    );
    assert_eq!(
        ids(app.list_threads(&bob, None, 50, false).await.unwrap()),
        [b.id]
    );
    // Newest first, with a cursor and a limit, over the administrator's own.
    assert_eq!(
        ids(app.list_threads(&root, None, 50, false).await.unwrap()),
        [r2.id, r.id]
    );
    assert_eq!(
        ids(app
            .list_threads(&root, Some(r2.id), 50, false)
            .await
            .unwrap()),
        [r.id]
    );
    assert_eq!(
        ids(app.list_threads(&root, None, 1, false).await.unwrap()),
        [r2.id]
    );
    // A cursor that is another person's thread does not open their list.
    assert_eq!(
        ids(app
            .list_threads(&root, Some(b.id), 50, false)
            .await
            .unwrap()),
        Vec::<ThreadId>::new()
    );
}

#[tokio::test]
async fn no_set_of_permissions_reaches_another_persons_thread() {
    // A role holding every permission, `admin` among them, still has only its own threads: the
    // reach of a permission over threads is not configurable (ADR 0039).
    let w = World::new();
    let app = app_under(
        &w,
        policy(
            &[(
                "ops",
                role(
                    &[
                        Permission::AgentRead,
                        Permission::AgentInvoke,
                        Permission::ThreadRead,
                        Permission::ThreadWrite,
                        Permission::ArtifactRead,
                        Permission::Admin,
                    ],
                    &["*"],
                ),
            )],
            Some("ops"),
        ),
    );
    let (alice, ops) = (
        principal("alice@example.com", &["ops"]),
        principal("ops@example.com", &["ops"]),
    );
    let t = started(&app, &alice, "plain").await;
    is_not_found(app.get_thread(&ops, t.id).await);
    is_not_found(app.export_thread(&ops, t.id).await);
    is_not_found(app.rename_thread(&ops, t.id, "x").await);
    is_not_found(
        app.open_artifact(&ops, t.id, &"0".repeat(64))
            .await
            .map(|_| ()),
    );
    assert!(
        app.list_threads(&ops, None, 50, false)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_role_without_a_permission_is_refused_whatever_is_asked_for() {
    let w = World::new();
    let reader = role(&[Permission::AgentRead, Permission::ThreadRead], &["*"]);
    let nothing = RoleGrant::none();
    let app = app_under(
        &w,
        policy(&[("reader", reader), ("nothing", nothing)], None),
    );
    // Someone with a full role made a thread, for the reader to be the owner of.
    let full = app_under(&w, Policy::default());
    let alice = user("alice@example.com");
    let t = started(&full, &alice, "plain").await;

    let reader = principal("alice@example.com", &["reader"]);
    let nobody = principal("alice@example.com", &["nothing"]);
    // The reader reads their own and cannot act, nor start anything.
    assert!(app.get_thread(&reader, t.id).await.is_ok());
    assert_eq!(
        forbidden(app.create_thread(&reader, new_thread("plain", "x")).await),
        Permission::ThreadWrite
    );
    // The same refusal for an id that exists and one that does not: it says nothing of threads.
    for id in [t.id, ThreadId(uuid::Uuid::from_u128(9))] {
        assert_eq!(
            forbidden(app.post_message(&reader, id, "x".into()).await),
            Permission::ThreadWrite
        );
        assert_eq!(
            forbidden(app.cancel(&reader, id).await),
            Permission::ThreadWrite
        );
    }
    assert_eq!(
        forbidden(
            app.open_artifact(&reader, t.id, &"0".repeat(64))
                .await
                .map(|_| ())
        ),
        Permission::ArtifactRead
    );
    // The role that holds nothing is refused even a read, for every id alike.
    for id in [t.id, ThreadId(uuid::Uuid::from_u128(9))] {
        assert_eq!(
            forbidden(app.get_thread(&nobody, id).await),
            Permission::ThreadRead
        );
    }
    assert_eq!(
        forbidden(app.list_threads(&nobody, None, 50, false).await),
        Permission::ThreadRead
    );
    assert_eq!(
        forbidden(app.list_agents(&nobody).await),
        Permission::AgentRead
    );
    assert_eq!(
        forbidden(app.registry_sources(&nobody).await),
        Permission::AgentRead
    );
    // A person with no known role and no default role has nothing either.
    let stranger = principal("alice@example.com", &["wizard"]);
    assert!(app.access(&stranger).is_empty());
    assert_eq!(
        forbidden(app.get_thread(&stranger, t.id).await),
        Permission::ThreadRead
    );
    // The class a surface maps.
    assert_eq!(
        class(app.get_thread(&stranger, t.id).await),
        ErrorClass::Forbidden
    );
}

#[tokio::test]
async fn an_unknown_role_grants_nothing_and_the_default_role_is_what_is_left() {
    let w = World::new();
    let app = w.app();
    let alice = user("alice@example.com");
    let t = started(&app, &alice, "plain").await;
    // Roles no one defined (and spelled differently from the ones that are) are ignored: the
    // person is a user, not an administrator.
    for roles in [
        &["wizard"][..],
        &["Admin"],
        &["admin "],
        &["ADMIN", "root"],
        &[],
    ] {
        let who = principal("mallory@example.com", roles);
        is_not_found(app.get_thread(&who, t.id).await);
        assert!(!app.access(&who).has(Permission::Admin), "{roles:?}");
        assert!(started(&app, &who, "plain").await.owner == who.user);
    }
    // A bare user id is a person with no roles: the default role.
    assert!(app.get_thread(&alice.user, t.id).await.is_ok());
    is_not_found(
        app.get_thread(&orch_core::UserId::new("bob@example.com"), t.id)
            .await,
    );
}

#[tokio::test]
async fn roles_are_unioned() {
    let w = World::new();
    let reads = role(&[Permission::ThreadRead, Permission::ArtifactRead], &["*"]);
    let writes = role(
        &[
            Permission::ThreadWrite,
            Permission::AgentInvoke,
            Permission::AgentRead,
        ],
        &["*"],
    );
    let app = app_under(&w, policy(&[("reads", reads), ("writes", writes)], None));
    let alice = principal("alice@example.com", &["writes"]);
    let t = started(&app, &alice, "plain").await;
    let both = principal("alice@example.com", &["reads", "writes"]);
    let only_reads = principal("alice@example.com", &["reads"]);
    // The union reads the person's own thread (from `reads`) and writes in it (from `writes`).
    assert!(app.get_thread(&both, t.id).await.is_ok());
    assert!(app.post_message(&both, t.id, "x".into()).await.is_ok());
    assert!(started(&app, &both, "plain").await.owner == both.user);
    // Reading alone does not write, nor start anything.
    assert!(app.get_thread(&only_reads, t.id).await.is_ok());
    assert_eq!(
        forbidden(app.post_message(&only_reads, t.id, "x".into()).await),
        Permission::ThreadWrite
    );
    assert_eq!(
        forbidden(
            app.create_thread(&only_reads, new_thread("plain", "x"))
                .await
        ),
        Permission::ThreadWrite
    );
    // No union of roles reaches another person's thread.
    let bob = principal("bob@example.com", &["reads", "writes"]);
    is_not_found(app.get_thread(&bob, t.id).await);
    is_not_found(app.post_message(&bob, t.id, "x".into()).await);
}

#[tokio::test]
async fn agents_are_listed_described_and_invoked_by_the_roles_that_name_them() {
    let w = World::new();
    let chat_only = role(
        &[
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ],
        &["plain"],
    );
    let app = app_under(
        &w,
        policy(
            &[("chat", chat_only), ("user", RoleGrant::user())],
            Some("user"),
        ),
    );
    let full = user("alice@example.com");
    let chat = principal("alice@example.com", &["chat"]);

    // Listed: the agents the role names, in the registry's order; the full role sees both.
    let ids = |list: orch_app::AgentList| {
        list.agents
            .into_iter()
            .map(|a| a.id.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(app.list_agents(&full).await.unwrap()),
        ["coder", "plain"]
    );
    assert_eq!(ids(app.list_agents(&chat).await.unwrap()), ["plain"]);
    // The sources are the registry's own, whoever asks.
    assert_eq!(
        app.list_agents(&chat).await.unwrap().sources,
        app.list_agents(&full).await.unwrap().sources
    );
    // Described: an agent the role does not name is 403, as is invoking it.
    assert!(
        app.describe_agent(&chat, &AgentId::new("plain"))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        forbidden(app.describe_agent(&chat, &AgentId::new("coder")).await),
        Permission::AgentRead
    );
    assert_eq!(
        forbidden(app.create_thread(&chat, new_thread("coder", "x")).await),
        Permission::AgentInvoke
    );
    assert!(
        app.create_thread(&chat, new_thread("plain", "x"))
            .await
            .is_ok()
    );
    // The default agent is the first the person may invoke.
    assert_eq!(app.default_agent(&full).await, Some(AgentId::new("coder")));
    assert_eq!(app.default_agent(&chat).await, Some(AgentId::new("plain")));
    // A thread on an agent the role does not name takes no message from it: the agent is
    // invoked by the message. What does not invoke the agent still works.
    let on_coder = started(&app, &full, "coder").await;
    assert_eq!(
        forbidden(app.post_message(&chat, on_coder.id, "x".into()).await),
        Permission::AgentInvoke
    );
    assert_eq!(
        forbidden(
            app.submit(
                &chat,
                on_coder.id,
                Input::UserMessage {
                    user: chat.user.clone(),
                    text: "x".into(),
                    message_id: None,
                    run_id: None,
                    origin: Origin::default(),
                    catalog: None,
                    mentions: Vec::new(),
                },
                None,
            )
            .await
            .map(|_| ())
        ),
        Permission::AgentInvoke
    );
    assert!(
        app.rename_thread(&chat, on_coder.id, "Renamed")
            .await
            .is_ok()
    );
    assert!(app.cancel(&chat, on_coder.id).await.is_ok());
    // A fork may not move a thread to an agent the role does not name.
    let done = started(&app, &chat, "plain").await;
    assert_eq!(
        forbidden(
            app.fork_thread(
                &chat,
                done.id,
                ForkRequest {
                    at: ForkAt::Replace {
                        seq: 1,
                        text: "again".into(),
                        message_id: None,
                    },
                    target: Some(AgentTarget {
                        agent_id: AgentId::new("coder"),
                        release: None,
                    }),
                    id: None,
                },
            )
            .await
            .map(|_| ())
        ),
        Permission::AgentInvoke
    );
}

#[tokio::test]
async fn a_fork_made_with_its_first_message_takes_write_on_the_parent_and_invoke_on_the_agent() {
    let w = World::new();
    let chat_only = role(
        &[
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ],
        &["plain"],
    );
    let reads = role(
        &[
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
        ],
        &["*"],
    );
    let app = app_under(
        &w,
        policy(
            &[
                ("chat", chat_only),
                ("reads", reads),
                ("user", RoleGrant::user()),
            ],
            Some("user"),
        ),
    );
    let first = || FirstMessage {
        text: "again".into(),
        message_id: None,
        run_id: None,
        origin: Origin::default(),
        ui_catalog: None,
        mentions: Vec::new(),
    };
    let id = ThreadId(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0001,
    ));
    let chat = principal("alice@example.com", &["chat"]);
    let parent = started(&app, &chat, "plain").await;
    // an agent the role does not name is not the target of a fork either
    assert_eq!(
        forbidden(
            app.fork_and_send(
                &chat,
                parent.id,
                id,
                1,
                Some(AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                }),
                first(),
            )
            .await
        ),
        Permission::AgentInvoke
    );
    // a role that may read the thread and not write to it cannot fork it with a message
    let reader = principal("alice@example.com", &["reads"]);
    assert_eq!(
        forbidden(
            app.fork_and_send(&reader, parent.id, id, 1, None, first())
                .await
        ),
        Permission::ThreadWrite
    );
    // nothing was made for either
    assert!(app.get_thread(&chat, id).await.is_err());
    assert_eq!(
        app.list_threads(&chat, None, 10, true).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_role_that_names_an_agent_nobody_has_lists_the_ones_that_exist() {
    let w = World::new();
    let app = app_under(
        &w,
        policy(
            &[(
                "scoped",
                role(&[Permission::AgentRead], &["coder", "ghost"]),
            )],
            Some("scoped"),
        ),
    );
    let who = user("a@example.com");
    // A name no agent has matches nothing; `ghost` is neither listed nor an error.
    let listed = app.list_agents(&who).await.unwrap();
    assert_eq!(
        listed
            .agents
            .iter()
            .map(|a| a.id.to_string())
            .collect::<Vec<_>>(),
        ["coder"]
    );
}

#[tokio::test]
async fn files_are_read_by_artifact_read_over_the_thread() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    type FilePorts = PortSet<
        MemoryStore,
        MemoryWakeup,
        ScriptedAgent,
        SystemClock,
        SeqIds,
        ScriptedModel,
        FixedRegistry,
        orch_ports::RefuseAll,
        MemoryArtifacts,
    >;
    let build = |policy: Policy| {
        Arc::new(
            App::<FilePorts>::new(
                PortSet {
                    artifacts: store.clone(),
                    store: w.store.clone(),
                    wakeup: w.wakeup.clone(),
                    agents: w.agent.clone(),
                    clock: SystemClock,
                    ids: w.ids.clone(),
                    model: w.model.clone(),
                    auth: orch_ports::RefuseAll,
                    registry: directory().fixed_registry(),
                },
                directory(),
                AppConfig {
                    policy,
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        )
    };
    let app = build(Policy::default());
    let (alice, bob, root) = (
        user("alice@example.com"),
        user("bob@example.com"),
        admin("root@example.com"),
    );
    let t = app
        .create_thread(&alice, new_thread("plain", "hi"))
        .await
        .unwrap();

    let bytes = Bytes::from_static(b"hello, file");
    let meta = ArtifactMeta::of("text/plain", Some("hello.txt".to_owned()), &bytes);
    let key = ArtifactKey::new(t.id, meta.sha256);
    store.put(&key, bytes, &meta).await.unwrap();
    let hash = key.sha256_hex();

    let read = |app: Arc<App<FilePorts>>, who: Principal| {
        let hash = hash.clone();
        async move {
            let (meta, mut stream) = app.open_artifact(&who, t.id, &hash).await?;
            let mut body = Vec::new();
            while let Some(piece) = stream.next().await {
                body.extend_from_slice(&piece.unwrap());
            }
            Ok::<_, AppError>((meta.size, body))
        }
    };
    // The owner reads it; another user and the administrator do not know it exists (ADR 0039).
    assert_eq!(
        read(Arc::clone(&app), alice.clone()).await.unwrap().1,
        b"hello, file"
    );
    is_not_found(read(Arc::clone(&app), root.clone()).await);
    is_not_found(read(Arc::clone(&app), bob.clone()).await);
    // A thread that does not exist, and a file that is not there, are the same.
    is_not_found(
        app.open_artifact(&alice, ThreadId(uuid::Uuid::from_u128(5)), &hash)
            .await
            .map(|_| ()),
    );
    is_not_found(
        app.open_artifact(&alice, t.id, &"f".repeat(64))
            .await
            .map(|_| ()),
    );

    // `artifact.read` is its own permission: a role that reads threads and not files.
    let narrow = build(policy(
        &[(
            "narrow",
            RoleGrant {
                permissions: BTreeSet::from([Permission::ThreadRead, Permission::AgentRead]),
                ..RoleGrant::user()
            },
        )],
        Some("narrow"),
    ));
    assert_eq!(
        forbidden(read(Arc::clone(&narrow), alice.clone()).await),
        Permission::ArtifactRead,
        "thread.read is not artifact.read"
    );
    let files_only = build(policy(
        &[("files", role(&[Permission::ArtifactRead], &["*"]))],
        Some("files"),
    ));
    assert_eq!(
        read(Arc::clone(&files_only), alice.clone())
            .await
            .unwrap()
            .0,
        11
    );
    // Nor does artifact.read read the thread.
    assert_eq!(
        forbidden(files_only.get_thread(&alice, t.id).await),
        Permission::ThreadRead
    );
}

#[tokio::test]
async fn an_access_says_what_the_person_may_do() {
    let w = World::new();
    let app = w.app();
    let root = admin("root@example.com");
    let access: Access<'_> = app.access(&root);
    assert_eq!(
        access.roles().map(Role::as_str).collect::<Vec<_>>(),
        ["admin"]
    );
    // Every scoped permission reaches the person's own threads, and nothing else (ADR 0039).
    for permission in [
        Permission::ThreadRead,
        Permission::ThreadWrite,
        Permission::ArtifactRead,
    ] {
        assert_eq!(access.scope(permission), Some(Scope::Own), "{permission}");
    }
    assert_eq!(access.scope(Permission::Admin), None);
    assert!(access.has(Permission::Admin));
    assert_eq!(app.policy().default_role().map(Role::as_str), Some("user"));
}
