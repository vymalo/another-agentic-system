//! Attaching a person's own MCP server to a thread, and what a fork keeps of it (ADR 0046, decision
//! 4, slice 5a): the thread **owner** must own the server (`u-` ids), it must be `ready`, the limit
//! of 16 counts both kinds, and a fork carries what its new owner owns.
//!
//! The definitions are in the in-memory store and the broker is one that does nothing: attaching asks
//! the broker for nothing (a lease refreshes a token, and an attach is not a call), so a test that
//! passes with a broker that refuses everything has shown it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, AppError, Creation, Dispatcher, ForkAt,
    ForkRequest, Inbound, NewThread, OwnReason, Permission, Policy, RoleGrant, ToolServerInfo,
    UserToolRefusal,
};
use orch_core::{
    AgentId, AgentTarget, EventBody, EventKind, ThreadId, ThreadRecord, ThreadState, ToolsData,
    UserId,
};
use orch_ports::memory::{
    MemoryPendingAuthorizations, MemoryStore, MemoryUserToolServers, MemoryWakeup, ScriptedAgent,
    SeqIds,
};
use orch_ports::{
    AgentEndpoint, CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy,
    FixedRegistry, HostPattern, KeepsCredentials, NoArtifacts, NoBroker, NoModel, OAuthStart,
    OAuthState, PortSet, Principal, RedirectUri, RefuseAll, Role, SystemClock, ToolSecret,
    UserAuth, UserToolServer, UserToolServerId, UserToolServerState, UserToolServerStore,
    UserTools,
};

// ------------------------------------------------------------------------------ the doubles

/// A broker that does nothing: every call is refused. Attaching must not need it.
#[derive(Clone, Default)]
struct IdleBroker;

impl From<NoBroker> for IdleBroker {
    fn from(_: NoBroker) -> Self {
        IdleBroker
    }
}

impl KeepsCredentials for IdleBroker {}

fn idle() -> CredentialError {
    CredentialError::unavailable("the test broker does nothing")
}

impl CredentialBroker for IdleBroker {
    async fn store(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        Err(idle())
    }

    async fn begin_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        Err(idle())
    }

    async fn complete_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _state: &OAuthState,
        _code: &ToolSecret,
        _iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        Err(idle())
    }

    async fn lease(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        Err(idle())
    }

    async fn revoke(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        Err(idle())
    }
}

type Bundle = UserTools<MemoryUserToolServers, IdleBroker, MemoryPendingAuthorizations>;
type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    RefuseAll,
    NoArtifacts,
    Bundle,
>;

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// Alice and Bob may use servers of their own (`tools.user.add`); Carol may not.
fn alice() -> Principal {
    principal("alice@example.com", &["user", "maker"])
}
fn bob() -> Principal {
    principal("bob@example.com", &["user", "maker"])
}
fn carol() -> Principal {
    principal("carol@example.com", &["user"])
}
/// Alice after her operator took `tools.user.add` from her roles: the same person, the same threads.
fn alice_revoked() -> Principal {
    principal("alice@example.com", &["user"])
}

fn policy() -> Policy {
    Policy::new(
        BTreeMap::from([
            (Role::new("user"), RoleGrant::user()),
            (
                Role::new("maker"),
                RoleGrant {
                    permissions: [Permission::ToolsUserAdd].into_iter().collect(),
                    ..RoleGrant::none()
                },
            ),
        ]),
        Some(Role::new("user")),
    )
    .unwrap()
}

fn directory() -> AgentDirectory {
    let entry = |id: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    };
    AgentDirectory::new(vec![entry("plain"), entry("coder")])
}

/// The deployment's servers: nine, `docs-1` to `docs-9`, so that a thread can reach its limit of 16
/// with the eight a person may hold.
fn deployment() -> Vec<ToolServerInfo> {
    (1..=9)
        .map(|n| ToolServerInfo::new(format!("docs-{n}"), format!("Docs {n}")))
        .collect()
}

struct Rig {
    app: Arc<App<Stack>>,
    servers: MemoryUserToolServers,
    stop: tokio_util::sync::CancellationToken,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Rig {
    fn full() -> Rig {
        Self::with(EgressPolicy::new(), true)
    }

    fn with(egress: EgressPolicy, keeps_credentials: bool) -> Rig {
        let servers = MemoryUserToolServers::new();
        let bundle = if keeps_credentials {
            Bundle::full(
                egress,
                servers.clone(),
                IdleBroker,
                MemoryPendingAuthorizations::new(),
            )
        } else {
            Bundle::without_credentials(egress, servers.clone())
        };
        let app = Arc::new(
            App::new(
                PortSet {
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth: RefuseAll,
                    registry: directory().fixed_registry(),
                    artifacts: NoArtifacts,
                    user_tools: bundle,
                },
                directory(),
                AppConfig {
                    policy: policy(),
                    tool_servers: deployment(),
                    stream_poll: Duration::from_millis(100),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        // a dispatcher, so that a thread runs to its end (`echo`) and can be forked
        let stop = tokio_util::sync::CancellationToken::new();
        tokio::spawn(
            Dispatcher::new(Arc::clone(&app), support::fast(), "attach").run(stop.clone()),
        );
        Rig { app, servers, stop }
    }

    /// A server of `owner`'s in `state` (a `bearer` one has a credential when it is connected).
    async fn server(&self, owner: &Principal, state: UserToolServerState) -> UserToolServerId {
        self.server_of(owner, state, UserAuth::Bearer).await
    }

    async fn server_of(
        &self,
        owner: &Principal,
        state: UserToolServerState,
        auth: UserAuth,
    ) -> UserToolServerId {
        let id = UserToolServerId::generate();
        let target = EgressPolicy::new()
            .check_url("https://mcp.example.com/mcp")
            .unwrap();
        let mut server = UserToolServer::new(id.clone(), "Tracker", &target, auth)
            .unwrap()
            .with_state(state);
        if state == UserToolServerState::Connected && server.auth().needs_credential() {
            server = server.with_credential(CredentialRef::new("cred-1").unwrap());
        }
        self.servers.insert(&owner.user, &server).await.unwrap();
        id
    }

    async fn thread(&self, owner: &Principal) -> ThreadRecord {
        self.app
            .create_thread(
                owner,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "echo hello".to_owned(),
                },
            )
            .await
            .unwrap()
    }

    /// Waits until the thread's turn is over.
    async fn done(&self, owner: &Principal, thread: ThreadId) -> ThreadRecord {
        support::eventually("the turn to end", || async {
            let record = self.app.get_thread(&owner.user, thread).await.unwrap();
            (record.state == ThreadState::Done).then_some(record)
        })
        .await
    }

    async fn attach(
        &self,
        owner: &Principal,
        thread: ThreadId,
        ids: &[&str],
    ) -> Result<ThreadRecord, AppError> {
        self.app
            .set_tools(owner, thread, ids.iter().map(|s| (*s).to_owned()).collect())
            .await
    }

    async fn events(&self, owner: &Principal, thread: ThreadId) -> Vec<orch_core::Event> {
        self.app
            .list_events(&owner.user, thread, 0, 500)
            .await
            .unwrap()
    }
}

fn unprocessable(result: Result<ThreadRecord, AppError>) -> String {
    match result {
        Err(AppError::Unprocessable(detail)) => detail,
        other => panic!("expected Unprocessable, got {other:?}"),
    }
}

fn not_ready(result: Result<ThreadRecord, AppError>) -> (String, OwnReason) {
    match result {
        Err(AppError::UserTool(UserToolRefusal::NotReady { id, reason })) => (id, reason),
        other => panic!("expected NotReady, got {other:?}"),
    }
}

fn pattern(text: &str) -> HostPattern {
    text.parse().unwrap()
}

// ------------------------------------------------------------------------------------ attach

#[tokio::test]
async fn the_owner_attaches_a_ready_server_of_their_own_and_the_log_keeps_its_id_only() {
    let rig = Rig::full();
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;

    let after = rig
        .attach(&alice(), thread.id, &[id.as_str()])
        .await
        .unwrap();
    assert_eq!(after.job.tools, [id.to_string()]);

    let log = rig.events(&alice(), thread.id).await;
    let attached: Vec<_> = log
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ToolsAttached(data) => Some(data.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        attached,
        [ToolsData {
            servers: vec![id.to_string()]
        }]
    );
    // nothing of the definition is in the log: not its name, nor its URL
    let text = serde_json::to_string(&log).unwrap();
    assert!(
        !text.contains("Tracker") && !text.contains("mcp.example.com"),
        "{text}"
    );

    // the same set again writes nothing
    let again = rig
        .attach(&alice(), thread.id, &[id.as_str()])
        .await
        .unwrap();
    assert_eq!(again.last_seq, after.last_seq);
}

#[tokio::test]
async fn a_server_of_a_kind_that_needs_no_credential_is_ready_when_connected() {
    let rig = Rig::with(EgressPolicy::new(), false);
    let id = rig
        .server_of(&alice(), UserToolServerState::Connected, UserAuth::None)
        .await;
    let thread = rig.thread(&alice()).await;
    assert!(
        rig.attach(&alice(), thread.id, &[id.as_str()])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn another_persons_server_reads_like_an_id_nobody_has() {
    let rig = Rig::full();
    let bobs = rig.server(&bob(), UserToolServerState::Connected).await;
    let nobodys = UserToolServerId::generate();
    let thread = rig.thread(&alice()).await;

    let theirs = unprocessable(rig.attach(&alice(), thread.id, &[bobs.as_str()]).await);
    let none = unprocessable(rig.attach(&alice(), thread.id, &[nobodys.as_str()]).await);
    // the sentence is the same up to the id Alice sent: no oracle for the ids of others
    assert_eq!(
        theirs.replace(bobs.as_str(), "ID"),
        none.replace(nobodys.as_str(), "ID")
    );
    assert!(theirs.contains("is offered for attaching"), "{theirs}");
    // nothing was attached
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert!(after.job.tools.is_empty());
}

#[tokio::test]
async fn a_malformed_own_id_is_unknown_and_not_a_server_of_someone() {
    let rig = Rig::full();
    let thread = rig.thread(&alice()).await;
    // the shape of any server id, with the prefix of a person's, but not an id one is made with
    for id in ["u-x", "u-abc", "u-0123456789abc", "u-ABCDEFGHIJ"] {
        let result = rig.attach(&alice(), thread.id, &[id]).await;
        match result {
            Err(AppError::Unprocessable(_) | AppError::Invalid(_)) => {}
            other => panic!("{id}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_server_that_is_not_ready_is_refused_with_why() {
    let rig = Rig::full();
    let thread = rig.thread(&alice()).await;
    for (state, reason) in [
        (UserToolServerState::Added, OwnReason::NotConnected),
        (UserToolServerState::NeedsReauth, OwnReason::Reauthorize),
        (UserToolServerState::Revoked, OwnReason::Disconnected),
    ] {
        let id = rig.server(&alice(), state).await;
        let (named, why) = not_ready(rig.attach(&alice(), thread.id, &[id.as_str()]).await);
        assert_eq!((named, why), (id.to_string(), reason), "{state:?}");
        assert_eq!(reason.attach_code(), "tool_server_needs_auth");
    }
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert!(
        after.job.tools.is_empty(),
        "a refused attach attached nothing"
    );
}

#[tokio::test]
async fn a_server_is_unavailable_to_a_person_without_the_permission() {
    let rig = Rig::full();
    // Carol holds a definition (her permission was taken away after she added it)
    let id = rig.server(&carol(), UserToolServerState::Connected).await;
    let thread = rig.thread(&carol()).await;
    let (_, reason) = not_ready(rig.attach(&carol(), thread.id, &[id.as_str()]).await);
    assert_eq!(reason, OwnReason::NotPermitted);
    assert_eq!(reason.attach_code(), "tool_server_unavailable");
}

#[tokio::test]
async fn a_server_the_egress_lists_refuse_now_is_unavailable() {
    let rig = Rig::with(
        EgressPolicy::new().with_deny(pattern("mcp.example.com")),
        true,
    );
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;
    let (_, reason) = not_ready(rig.attach(&alice(), thread.id, &[id.as_str()]).await);
    assert_eq!(reason, OwnReason::EgressRefused);
}

#[tokio::test]
async fn a_credential_kind_is_unavailable_where_no_credentials_are_kept() {
    let rig = Rig::with(EgressPolicy::new(), false);
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;
    let (_, reason) = not_ready(rig.attach(&alice(), thread.id, &[id.as_str()]).await);
    assert_eq!(reason, OwnReason::BrokerDown);
}

#[tokio::test]
async fn attaching_asks_the_broker_for_nothing() {
    // the broker of this rig refuses every call, `lease` included: the attach above passed
    let rig = Rig::full();
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;
    assert!(
        rig.attach(&alice(), thread.id, &[id.as_str()])
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn definitions_that_cannot_be_read_fail_the_attach_and_attach_nothing() {
    let rig = Rig::full();
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;
    rig.servers.set_down(true);
    let result = rig.attach(&alice(), thread.id, &[id.as_str()]).await;
    assert!(matches!(result, Err(AppError::Store(_))), "{result:?}");
    rig.servers.set_down(false);
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert!(after.job.tools.is_empty());
}

#[tokio::test]
async fn a_deployment_with_the_feature_off_reads_an_own_id_as_unknown() {
    let w = support::World::new();
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    });
    let thread = support::create(&app, &support::alice(), "plain", "hello").await;
    let id = UserToolServerId::generate();
    let off = app
        .set_tools(&support::alice(), thread.id, vec![id.to_string()])
        .await;
    let off = unprocessable(off);
    let unknown = unprocessable(
        app.set_tools(&support::alice(), thread.id, vec!["nothing".to_owned()])
            .await,
    );
    assert_eq!(
        off.replace(id.as_str(), "ID"),
        unknown.replace("nothing", "ID")
    );
}

#[tokio::test]
async fn a_server_the_deployment_lists_is_its_own_whatever_its_id_looks_like() {
    // with people's servers off, a deployment may list a server called `u-docs`; it is attached
    // from the deployment's list and never read as a person's
    let w = support::World::new();
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tool_servers: vec![ToolServerInfo::new("u-docs", "Docs")],
        ..AppConfig::default()
    });
    let thread = support::create(&app, &support::alice(), "plain", "hello").await;
    let after = app
        .set_tools(&support::alice(), thread.id, vec!["u-docs".to_owned()])
        .await
        .unwrap();
    assert_eq!(after.job.tools, ["u-docs"]);
}

#[tokio::test]
async fn the_limit_of_sixteen_counts_the_deployments_servers_and_the_persons_own() {
    let rig = Rig::full();
    let thread = rig.thread(&alice()).await;
    let mut own = Vec::new();
    for _ in 0..8 {
        own.push(
            rig.server(&alice(), UserToolServerState::Connected)
                .await
                .to_string(),
        );
    }
    let docs: Vec<String> = (1..=8).map(|n| format!("docs-{n}")).collect();
    let sixteen: Vec<&str> = own.iter().chain(&docs).map(String::as_str).collect();
    let after = rig.attach(&alice(), thread.id, &sixteen).await.unwrap();
    assert_eq!(after.job.tools.len(), 16);

    let seventeen: Vec<&str> = sixteen.iter().copied().chain(["docs-9"]).collect();
    let detail = unprocessable(rig.attach(&alice(), thread.id, &seventeen).await);
    assert!(detail.contains("16"), "{detail}");
    let kept = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert_eq!(kept.job.tools.len(), 16, "the refused set changed nothing");
}

#[tokio::test]
async fn a_thread_keeps_a_server_that_needs_a_new_key_and_a_person_can_detach_it() {
    let rig = Rig::full();
    let id = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = rig.thread(&alice()).await;
    rig.attach(&alice(), thread.id, &[id.as_str()])
        .await
        .unwrap();

    // the server needs a new key now (a 401 marked it)
    let stored = rig.servers.get(&alice().user, &id).await.unwrap().unwrap();
    rig.servers
        .set_state(
            &alice().user,
            &id,
            UserToolServerState::NeedsReauth,
            stored.revision(),
        )
        .await
        .unwrap();

    // an unrelated change of the set does not check the server again
    let both = rig
        .attach(&alice(), thread.id, &[id.as_str(), "docs-1"])
        .await
        .unwrap();
    assert_eq!(both.job.tools.len(), 2);
    // and it is not offered again once it is detached
    let only = rig.attach(&alice(), thread.id, &["docs-1"]).await.unwrap();
    assert_eq!(only.job.tools, ["docs-1"]);
    let (_, reason) = not_ready(rig.attach(&alice(), thread.id, &[id.as_str()]).await);
    assert_eq!(reason, OwnReason::Reauthorize);
}

#[tokio::test]
async fn a_thread_is_created_with_a_server_of_its_owner_and_not_with_one_that_is_not_ready() {
    let rig = Rig::full();
    let ready = rig.server(&alice(), UserToolServerState::Connected).await;
    let waiting = rig.server(&alice(), UserToolServerState::Added).await;
    let new = |tools: &[&UserToolServerId]| {
        let inbound = Inbound {
            tools: tools.iter().map(|t| t.to_string()).collect(),
            ..Inbound::default()
        };
        let new = NewThread {
            title: None,
            target: AgentTarget {
                agent_id: AgentId::new("plain"),
                release: None,
            },
            text: "hello".to_owned(),
        };
        (ThreadId(uuid::Uuid::now_v7()), new, inbound)
    };

    let (id, thread, inbound) = new(&[&ready]);
    let Creation::Created { thread, .. } = rig
        .app
        .create_thread_as(&alice(), id, thread, inbound)
        .await
        .unwrap()
    else {
        panic!("a fresh id exists");
    };
    assert_eq!(thread.job.tools, [ready.to_string()]);

    // a server that is not ready refuses the whole creation: no thread, no message
    let (id, thread, inbound) = new(&[&ready, &waiting]);
    let refused = rig
        .app
        .create_thread_as(&alice(), id, thread, inbound)
        .await;
    assert!(
        matches!(
            refused,
            Err(AppError::UserTool(UserToolRefusal::NotReady { .. }))
        ),
        "{refused:?}"
    );
    assert!(rig.app.get_thread(&alice().user, id).await.is_err());

    // somebody else's server: the same as an id nobody has
    let bobs = rig.server(&bob(), UserToolServerState::Connected).await;
    let (id, thread, inbound) = new(&[&bobs]);
    let refused = rig
        .app
        .create_thread_as(&alice(), id, thread, inbound)
        .await;
    assert!(
        matches!(refused, Err(AppError::Unprocessable(_))),
        "{refused:?}"
    );
}

// -------------------------------------------------------------------------------------- fork

/// A thread of Alice's that has ended, with `ids` attached: a fork can be made from it.
async fn ended_with(rig: &Rig, ids: &[&str]) -> ThreadRecord {
    let thread = rig.thread(&alice()).await;
    rig.done(&alice(), thread.id).await;
    rig.attach(&alice(), thread.id, ids).await.unwrap();
    rig.done(&alice(), thread.id).await
}

fn after_first_turn() -> ForkRequest {
    ForkRequest {
        at: ForkAt::AfterTurn {
            seq: 1,
            first: None,
        },
        target: None,
        id: None,
    }
}

fn detached_in(log: &[orch_core::Event]) -> Vec<Vec<String>> {
    log.iter()
        .filter_map(|e| match &e.body {
            EventBody::ToolsDetached(data) => Some(data.servers.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_fork_carries_the_servers_its_owner_owns() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;

    let fork = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.job.tools.len(), 2);
    assert!(fork.job.tools.contains(&mine.to_string()));
    // nothing was detached: the fork's log has no `tools_detached`
    let log = rig.events(&alice(), fork.id).await;
    assert!(detached_in(&log).is_empty(), "{:?}", detached_in(&log));
}

#[tokio::test]
async fn a_fork_carries_a_server_that_needs_a_new_key_because_it_is_still_theirs() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str()]).await;
    let stored = rig
        .servers
        .get(&alice().user, &mine)
        .await
        .unwrap()
        .unwrap();
    rig.servers
        .set_state(
            &alice().user,
            &mine,
            UserToolServerState::NeedsReauth,
            stored.revision(),
        )
        .await
        .unwrap();
    let fork = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.job.tools, [mine.to_string()]);
}

#[tokio::test]
async fn a_fork_detaches_a_server_its_owner_does_not_own_in_its_first_events() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;

    // the server is deleted; then somebody else adds a definition under the very same id (the owner
    // of a definition is part of its key, so Bob's `u-…` is not Alice's)
    rig.servers.delete(&alice().user, &mine).await.unwrap();
    let target = EgressPolicy::new()
        .check_url("https://mcp.example.com/mcp")
        .unwrap();
    let bobs = UserToolServer::new(mine.clone(), "Bob's", &target, UserAuth::Bearer)
        .unwrap()
        .with_state(UserToolServerState::Connected)
        .with_credential(CredentialRef::new("cred-bob").unwrap());
    rig.servers.insert(&bob().user, &bobs).await.unwrap();

    let fork = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert_eq!(
        fork.job.tools,
        ["docs-1"],
        "the deployment's server is kept"
    );

    // detached in the fork's own log, after the copy and its marker, so the log and the thread agree
    let log = rig.events(&alice(), fork.id).await;
    assert_eq!(detached_in(&log), [vec![mine.to_string()]]);
    let marker = log
        .iter()
        .position(|e| e.kind() == EventKind::ThreadForked)
        .unwrap();
    let detach = log
        .iter()
        .position(|e| e.kind() == EventKind::ToolsDetached)
        .unwrap();
    assert!(detach > marker, "the detach comes after thread_forked");
    // the parent is untouched
    let parent_after = rig.app.get_thread(&alice().user, parent.id).await.unwrap();
    assert_eq!(parent_after.job.tools.len(), 2);
}

#[tokio::test]
async fn a_fork_of_a_thread_whose_server_is_deleted_detaches_it() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str()]).await;
    rig.servers.delete(&alice().user, &mine).await.unwrap();
    let fork = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert!(fork.job.tools.is_empty());
}

#[tokio::test]
async fn a_fork_made_while_the_definitions_cannot_be_read_fails_and_is_made_again() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str()]).await;
    rig.servers.set_down(true);
    let result = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await;
    assert!(matches!(result, Err(AppError::Store(_))), "{result:?}");
    rig.servers.set_down(false);
    let fork = rig
        .app
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.job.tools, [mine.to_string()]);
}

#[tokio::test]
async fn a_fork_in_a_deployment_that_turned_the_feature_off_detaches_a_persons_server() {
    // the thread was made when the feature was on; the fork is made by a process where it is off
    let on = Rig::full();
    let mine = on.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&on, &[mine.as_str()]).await;
    assert_eq!(parent.job.tools.len(), 1);
    // the same thread store under an application whose bundle is off
    let off = App::new(
        PortSet {
            store: on.app.ports().store.clone(),
            wakeup: MemoryWakeup::new(),
            agents: ScriptedAgent::new(),
            clock: SystemClock,
            // the same ids, so that the fork's id is not one the other application made
            ids: on.app.ports().ids.clone(),
            model: NoModel,
            auth: RefuseAll,
            registry: directory().fixed_registry(),
            artifacts: NoArtifacts,
            user_tools: orch_ports::NoUserTools::off(),
        },
        directory(),
        AppConfig {
            policy: policy(),
            ..AppConfig::default()
        },
    )
    .unwrap();
    let fork = off
        .fork_thread(&alice(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert!(fork.job.tools.is_empty());
}

// ------------------------------------------------------- a revoked permission (slice 5a review)

/// The `tools_detached` and `user_message` events of the thread in log order, as `(kind, ids)`.
fn tools_and_messages(log: &[orch_core::Event]) -> Vec<(&'static str, Vec<String>)> {
    log.iter()
        .filter_map(|e| match &e.body {
            EventBody::ToolsDetached(data) => Some(("detached", data.servers.clone())),
            EventBody::ToolsAttached(data) => Some(("attached", data.servers.clone())),
            EventBody::UserMessage(_) => Some(("message", Vec::new())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_owner_who_holds_the_permission_keeps_their_servers_across_a_message() {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;
    rig.app
        .post_message(&alice(), thread.id, "again".to_owned())
        .await
        .unwrap();
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert!(
        after.job.tools.contains(&mine.to_string()),
        "{:?}",
        after.job.tools
    );
    let log = rig.events(&alice(), thread.id).await;
    assert!(detached_in(&log).is_empty(), "{:?}", detached_in(&log));
}

#[tokio::test]
async fn the_next_message_of_an_owner_who_lost_the_permission_detaches_their_servers_in_its_own_commit()
 {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;

    // the permission is taken away; the next message of the owner starts a job
    rig.app
        .post_message(&alice_revoked(), thread.id, "again".to_owned())
        .await
        .unwrap();
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    // the deployment's server stays; the person's own is gone before the job reads its set
    assert_eq!(after.job.tools, ["docs-1".to_owned()]);
    // in the log: detached, then the message, in one commit (no event between them)
    let log = rig.events(&alice(), thread.id).await;
    let order = tools_and_messages(&log);
    let detached = order
        .iter()
        .position(|(kind, ids)| *kind == "detached" && ids == &[mine.to_string()])
        .unwrap_or_else(|| panic!("no tools_detached for the server: {order:?}"));
    let again = order
        .iter()
        .rposition(|(kind, _)| *kind == "message")
        .unwrap();
    assert!(detached < again, "{order:?}");
    let by_seq = |position: usize| {
        log.iter()
            .filter(|e| {
                matches!(
                    e.body,
                    EventBody::ToolsDetached(_)
                        | EventBody::ToolsAttached(_)
                        | EventBody::UserMessage(_)
                )
            })
            .nth(position)
            .unwrap()
            .seq
    };
    assert_eq!(
        by_seq(again),
        by_seq(detached) + 1,
        "one commit: nothing between"
    );
    // and the picker says why: the server is `not_permitted`
    assert!(matches!(
        rig.attach(&alice_revoked(), thread.id, &[mine.as_str()])
            .await,
        Err(AppError::UserTool(UserToolRefusal::NotReady {
            reason: OwnReason::NotPermitted,
            ..
        }))
    ));
}

#[tokio::test]
async fn every_entry_that_starts_a_job_detaches_what_the_owner_may_no_longer_use() {
    // stop & send, a message submitted by a surface, a fork with its first message
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let mine = mine.to_string();

    let thread = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;
    rig.app
        .stop_and_send(&alice_revoked(), thread.id, "stop".to_owned())
        .await
        .unwrap();
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert_eq!(after.job.tools, ["docs-1".to_owned()], "stop_and_send");

    let thread = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;
    rig.app
        .submit(
            &alice_revoked(),
            thread.id,
            orch_core::Input::UserMessage {
                user: alice().user,
                text: "again".to_owned(),
                message_id: None,
                run_id: None,
                origin: orch_core::Origin::default(),
                catalog: None,
                mentions: Vec::new(),
                tier: None,
            },
            None,
        )
        .await
        .unwrap();
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert_eq!(after.job.tools, ["docs-1".to_owned()], "submit");

    // a fork carries what its owner may use, with its first message or without one
    let parent = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;
    let fork = rig
        .app
        .fork_thread(&alice_revoked(), parent.id, after_first_turn())
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.job.tools, ["docs-1".to_owned()], "fork_thread");
    let log = rig.events(&alice(), fork.id).await;
    assert_eq!(
        detached_in(&log),
        [vec![mine.clone()]],
        "in the fork's own log"
    );
    // the parent is untouched
    let parent = rig.app.get_thread(&alice().user, parent.id).await.unwrap();
    assert!(parent.job.tools.contains(&mine));
}

#[tokio::test]
async fn a_message_of_someone_with_no_servers_of_their_own_attached_writes_no_extra_event() {
    let rig = Rig::full();
    let thread = ended_with(&rig, &["docs-1"]).await;
    rig.app
        .post_message(&alice_revoked(), thread.id, "again".to_owned())
        .await
        .unwrap();
    let log = rig.events(&alice(), thread.id).await;
    assert!(detached_in(&log).is_empty());
    // a deployment server whose id looks like a person's is its own, and stays
    let after = rig.app.get_thread(&alice().user, thread.id).await.unwrap();
    assert_eq!(after.job.tools, ["docs-1".to_owned()]);
}

#[tokio::test]
async fn the_person_who_starts_a_job_is_always_the_owner_of_the_thread() {
    // why the requester's roles are the owner's: nobody acts on another's thread, whatever their roles
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let thread = ended_with(&rig, &[mine.as_str()]).await;
    let admin = principal("admin@example.com", &["user", "admin", "maker"]);
    let refused = rig
        .app
        .post_message(&admin, thread.id, "hello".to_owned())
        .await;
    assert!(matches!(refused, Err(AppError::NotFound)), "{refused:?}");
}

#[tokio::test]
async fn a_fork_made_with_its_first_message_by_an_owner_who_lost_the_permission_carries_no_server_of_theirs()
 {
    let rig = Rig::full();
    let mine = rig.server(&alice(), UserToolServerState::Connected).await;
    let parent = ended_with(&rig, &[mine.as_str(), "docs-1"]).await;
    let forked = rig
        .app
        .fork_and_send(
            &alice_revoked(),
            parent.id,
            ThreadId(uuid::Uuid::now_v7()),
            1,
            None,
            orch_app::FirstMessage {
                text: "from here".to_owned(),
                message_id: None,
                run_id: None,
                origin: orch_core::Origin::default(),
                ui_catalog: None,
                mentions: Vec::new(),
                tier: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(forked.thread.job.tools, ["docs-1".to_owned()]);
    let log = rig.events(&alice(), forked.thread.id).await;
    assert_eq!(detached_in(&log), [vec![mine.to_string()]]);
}

#[tokio::test]
async fn a_message_of_a_person_without_the_permission_keeps_a_deployment_server_that_looks_like_a_persons()
 {
    // `u-docs` is the deployment's: the revocation takes a person's own servers off a thread and
    // nothing else, whatever the ids look like
    let w = support::World::new();
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tool_servers: vec![ToolServerInfo::new("u-docs", "Docs")],
        ..AppConfig::default()
    });
    let who = support::alice();
    let thread = support::create(&app, &who, "plain", "hello").await;
    app.set_tools(&who, thread.id, vec!["u-docs".to_owned()])
        .await
        .unwrap();
    app.post_message(&who, thread.id, "again".to_owned())
        .await
        .unwrap();
    let after = app.get_thread(&who, thread.id).await.unwrap();
    assert_eq!(after.job.tools, ["u-docs"]);
    let log = app.list_events(&who, thread.id, 0, 100).await.unwrap();
    assert!(detached_in(&log).is_empty(), "{:?}", detached_in(&log));
}
