//! People's own MCP servers in the application (ADR 0046, slice 4b): who may add, edit and delete,
//! the order of the writes around the broker, what is checked before a write, what the listing says
//! of each server without asking the broker, and that a secret is in no error and no `Debug`.
//!
//! The broker here is a fake that follows the port's docs and records what it was asked and what the
//! definition looked like at that moment, so the order of the writes is asserted and not assumed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use orch_app::{
    AddUserToolServer, AgentDirectory, App, AppConfig, AppError, EditUserToolServer, OwnReason,
    OwnServers, OwnToolServer, Permission, Policy, RoleGrant, UserToolRefusal,
};
use orch_core::{Classify, ErrorClass, UserId, report};
use orch_ports::memory::{
    MemoryPendingAuthorizations, MemoryStore, MemoryUserToolServers, MemoryWakeup, ScriptedAgent,
    SeqIds,
};
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy, FixedRegistry,
    KeepsCredentials, NoArtifacts, NoBroker, NoModel, NoUserToolServers, OAuthStart, OAuthState,
    PortSet, Principal, RedirectUri, RefuseAll, Role, ServerRevision, StoredServer, SystemClock,
    ToolSecret, ToolServerStatus, UnavailableReason, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserTools,
};

// ------------------------------------------------------------------------ the doubles

/// A secret the tests plant: it must not be found anywhere but in the broker's sealed copy.
const SENTINEL: &str = "SENTINEL-4f1c9e-secret";
const URL: &str = "https://mcp.example.com/mcp";
const OTHER_URL: &str = "https://other.example.com/mcp";

/// What the next call of a method of the definitions' store does instead of going straight through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Goes through.
    Pass,
    /// Fails as `Unavailable` before it reaches the store: nothing is written.
    Fail,
    /// Another writer got in first: the row's revision moves (a write that changes nothing else), and
    /// then the call goes through with the revision it was given.
    Race,
    /// Somebody deleted the row first.
    Gone,
}

/// What the next calls of `get` and `replace` do, one entry per call; a call with no entry passes.
#[derive(Default)]
struct Script {
    gets: VecDeque<Step>,
    replaces: VecDeque<Step>,
}

/// The definitions' store with the switches a test throws: a row that does not read back, a store
/// that refuses everything (the shape of the store of a deployment where the feature is off), and a
/// script for `get` and `replace` (a write that fails, a writer that gets in between a read and a write).
#[derive(Clone, Default)]
struct Faulty {
    inner: MemoryUserToolServers,
    corrupt: Arc<AtomicBool>,
    refuse: Arc<AtomicBool>,
    script: Arc<Mutex<Script>>,
}

impl From<NoUserToolServers> for Faulty {
    fn from(_: NoUserToolServers) -> Self {
        let faulty = Faulty::default();
        faulty.refuse.store(true, Ordering::SeqCst);
        faulty
    }
}

impl Faulty {
    fn up(&self) -> Result<(), UserToolServerStoreError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(UserToolServerStoreError::unavailable(
                "people's own MCP servers are off in this deployment",
            ));
        }
        Ok(())
    }

    fn rows_read_back(&self) -> Result<(), UserToolServerStoreError> {
        if self.corrupt.load(Ordering::SeqCst) {
            return Err(UserToolServerStoreError::Corrupt(
                "a stored definition does not read back".to_owned(),
            ));
        }
        Ok(())
    }

    /// The next calls of `get` do these things, in order.
    fn script_gets(&self, steps: impl IntoIterator<Item = Step>) {
        self.script.lock().unwrap().gets.extend(steps);
    }

    /// The next calls of `replace` do these things, in order.
    fn script_replaces(&self, steps: impl IntoIterator<Item = Step>) {
        self.script.lock().unwrap().replaces.extend(steps);
    }

    /// Another writer: the row's revision moves and nothing else changes.
    async fn another_writer(&self, owner: &UserId, id: &UserToolServerId) {
        if let Some(now) = self.inner.get(owner, id).await.unwrap() {
            self.inner
                .set_state(owner, id, now.server().state(), now.revision())
                .await
                .unwrap();
        }
    }
}

impl UserToolServerStore for Faulty {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        self.inner.insert(owner, server).await
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        self.up()?;
        self.rows_read_back()?;
        let step = self.script.lock().unwrap().gets.pop_front();
        match step.unwrap_or(Step::Pass) {
            Step::Pass => {}
            Step::Fail => return Err(UserToolServerStoreError::unavailable("scripted")),
            Step::Race => self.another_writer(owner, id).await,
            Step::Gone => self.inner.delete(owner, id).await.unwrap(),
        }
        self.inner.get(owner, id).await
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        self.up()?;
        self.rows_read_back()?;
        self.inner.list(owner).await
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        let step = self.script.lock().unwrap().replaces.pop_front();
        match step.unwrap_or(Step::Pass) {
            Step::Pass => {}
            Step::Fail => return Err(UserToolServerStoreError::unavailable("scripted")),
            Step::Race => self.another_writer(owner, server.id()).await,
            Step::Gone => self.inner.delete(owner, server.id()).await.unwrap(),
        }
        self.inner.replace(owner, server, expected).await
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        self.inner.set_state(owner, id, state, expected).await
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        self.up()?;
        self.inner.delete(owner, id).await
    }
}

/// What the broker was asked, and what the definition looked like at that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Store {
        id: UserToolServerId,
        /// The URL of the definition the broker read, `None` when there was none.
        url: Option<String>,
        state: Option<UserToolServerState>,
    },
    Revoke {
        id: UserToolServerId,
        url: Option<String>,
    },
}

#[derive(Default)]
struct Brain {
    calls: Vec<Call>,
    sealed: BTreeMap<(UserId, UserToolServerId), String>,
    version: u64,
    fail_store: bool,
    /// A `store` that keeps the secret and then fails (the answer was lost): the broker kept it, the
    /// caller was told it did not.
    keeps_then_fails_store: bool,
    fail_revoke: bool,
    /// A failed `store` also takes the definitions' store down, so that taking the definition back
    /// fails too.
    take_servers_down_on_failed_store: bool,
    /// The next this-many `revoke`s write the definition (a state change that moves its revision), as
    /// the broker does when it learns a grant ended.
    bump_on_revoke: usize,
    /// Refuses every call, as `NoBroker` does.
    refuses: bool,
    /// The next `store` finds the server already moved by another request (a new URL, `added`, no
    /// credential named), as if its compare-and-swap had landed between this edit's read and its
    /// `store`: the broker then keeps the key for the new URL.
    move_before_store: bool,
}

#[derive(Clone)]
struct FakeBroker {
    servers: Faulty,
    brain: Arc<Mutex<Brain>>,
}

impl FakeBroker {
    fn new(servers: &Faulty) -> Self {
        FakeBroker {
            servers: servers.clone(),
            brain: Arc::default(),
        }
    }

    fn brain(&self) -> std::sync::MutexGuard<'_, Brain> {
        self.brain.lock().unwrap()
    }

    fn calls(&self) -> Vec<Call> {
        self.brain().calls.clone()
    }

    fn sealed(&self) -> Vec<String> {
        self.brain().sealed.values().cloned().collect()
    }
}

impl From<NoBroker> for FakeBroker {
    fn from(_: NoBroker) -> Self {
        let broker = FakeBroker::new(&Faulty::default());
        broker.brain().refuses = true;
        broker
    }
}

impl KeepsCredentials for FakeBroker {}

fn refused() -> CredentialError {
    CredentialError::unavailable("no credentials are kept")
}

impl CredentialBroker for FakeBroker {
    async fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        if self.brain().refuses {
            self.brain().calls.push(Call::Store {
                id: server.clone(),
                url: None,
                state: None,
            });
            return Err(refused());
        }
        let mover = std::mem::take(&mut self.brain().move_before_store);
        if mover && let Some(now) = self.servers.inner.get(owner, server).await.unwrap() {
            let moved = UserToolServer::from_stored(
                server.clone(),
                now.server().name(),
                OTHER_URL,
                now.server().auth().clone(),
                UserToolServerState::Added,
                None,
            )
            .unwrap();
            self.servers
                .inner
                .replace(owner, &moved, now.revision())
                .await
                .unwrap();
        }
        // like the real broker: the definition is read from the store, so it must be there
        let stored = self
            .servers
            .inner
            .get(owner, server)
            .await
            .map_err(|_| CredentialError::unavailable("definitions unreadable"))?;
        let (url, state) = stored.as_ref().map_or((None, None), |s| {
            (Some(s.server().url().to_owned()), Some(s.server().state()))
        });
        let mut brain = self.brain();
        brain.calls.push(Call::Store {
            id: server.clone(),
            url,
            state,
        });
        let Some(stored) = stored else {
            return Err(CredentialError::Unknown);
        };
        if !matches!(
            stored.server().auth(),
            UserAuth::Bearer | UserAuth::ApiKey { .. }
        ) {
            return Err(CredentialError::Unknown);
        }
        if brain.fail_store {
            if brain.take_servers_down_on_failed_store {
                self.servers.refuse.store(true, Ordering::SeqCst);
            }
            return Err(CredentialError::unavailable("the sealed store is down"));
        }
        brain.version += 1;
        let version = brain.version;
        brain.sealed.insert(
            (owner.clone(), server.clone()),
            material.expose().to_owned(),
        );
        if brain.keeps_then_fails_store {
            return Err(CredentialError::unavailable("the answer was lost"));
        }
        Ok(CredentialRef::new(&format!("cred-{server}-v{version}")).unwrap())
    }

    async fn begin_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        Err(refused())
    }

    async fn complete_oauth(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _state: &OAuthState,
        _code: &ToolSecret,
        _iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        Err(refused())
    }

    async fn lease(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        Err(CredentialError::Unknown)
    }

    async fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        let stored = self.servers.inner.get(owner, server).await.ok().flatten();
        let bump = {
            let mut brain = self.brain();
            brain.calls.push(Call::Revoke {
                id: server.clone(),
                url: stored.as_ref().map(|s| s.server().url().to_owned()),
            });
            if brain.refuses || brain.fail_revoke {
                return Err(refused());
            }
            let bump = brain.bump_on_revoke > 0;
            brain.bump_on_revoke = brain.bump_on_revoke.saturating_sub(1);
            bump
        };
        if let (true, Some(stored)) = (bump, &stored) {
            self.servers
                .inner
                .set_state(owner, server, stored.server().state(), stored.revision())
                .await
                .unwrap();
        }
        self.brain().sealed.remove(&(owner.clone(), server.clone()));
        Ok(())
    }
}

// ------------------------------------------------------------------------------ the rig

type Bundle = UserTools<Faulty, FakeBroker, MemoryPendingAuthorizations>;
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

struct Rig {
    app: App<Stack>,
    servers: Faulty,
    broker: FakeBroker,
}

/// `user` holds nothing here that matters; `maker` holds `tools.user.add` and nothing else.
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

fn who(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// Alice and Bob may add servers; Carol may not.
fn alice() -> Principal {
    who("alice@example.com", &["user", "maker"])
}
fn bob() -> Principal {
    who("bob@example.com", &["user", "maker"])
}
fn carol() -> Principal {
    who("carol@example.com", &["user"])
}

fn app_over(bundle: Bundle) -> App<Stack> {
    App::new(
        PortSet {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agents: ScriptedAgent::new(),
            clock: SystemClock,
            ids: SeqIds::default(),
            model: NoModel,
            auth: RefuseAll,
            registry: AgentDirectory::new(Vec::new()).fixed_registry(),
            artifacts: NoArtifacts,
            user_tools: bundle,
        },
        AgentDirectory::new(Vec::new()),
        AppConfig {
            policy: policy(),
            ..AppConfig::default()
        },
    )
    .unwrap()
}

impl Rig {
    fn full() -> Rig {
        Self::full_with(EgressPolicy::new())
    }

    fn full_with(egress: EgressPolicy) -> Rig {
        let servers = Faulty::default();
        let broker = FakeBroker::new(&servers);
        let bundle = Bundle::full(
            egress,
            servers.clone(),
            broker.clone(),
            MemoryPendingAuthorizations::new(),
        );
        Rig {
            app: app_over(bundle),
            servers,
            broker,
        }
    }

    fn without_credentials() -> Rig {
        let servers = Faulty::default();
        let bundle = Bundle::without_credentials(EgressPolicy::new(), servers.clone());
        Rig {
            app: app_over(bundle),
            servers,
            broker: FakeBroker::new(&Faulty::default()),
        }
    }

    fn off() -> Rig {
        Rig {
            app: app_over(Bundle::off()),
            servers: Faulty::default(),
            broker: FakeBroker::new(&Faulty::default()),
        }
    }

    /// Another application over the same definitions and the same broker, with `egress`.
    fn with_egress(&self, egress: EgressPolicy) -> App<Stack> {
        app_over(Bundle::full(
            egress,
            self.servers.clone(),
            self.broker.clone(),
            MemoryPendingAuthorizations::new(),
        ))
    }

    async fn add(&self, who: &Principal, request: AddUserToolServer) -> OwnToolServer {
        self.app.add_user_tool_server(who, request).await.unwrap()
    }

    async fn try_add(
        &self,
        who: &Principal,
        request: AddUserToolServer,
    ) -> Result<OwnToolServer, AppError> {
        self.app.add_user_tool_server(who, request).await
    }

    async fn listed(&self, who: &Principal) -> Vec<OwnToolServer> {
        match self.app.list_own_tool_servers(who).await.unwrap() {
            OwnServers::Listed(servers) => servers,
            other => panic!("expected a listing, got {other:?}"),
        }
    }

    async fn stored(&self, owner: &Principal, id: &UserToolServerId) -> Option<StoredServer> {
        self.servers.inner.get(&owner.user, id).await.unwrap()
    }
}

fn bearer(name: &str, url: &str, secret: &str) -> AddUserToolServer {
    AddUserToolServer {
        name: name.to_owned(),
        url: url.to_owned(),
        auth: UserAuth::Bearer,
        secret: Some(ToolSecret::new(secret)),
    }
}

fn api_key(name: &str, url: &str, header: &str, secret: &str) -> AddUserToolServer {
    AddUserToolServer {
        name: name.to_owned(),
        url: url.to_owned(),
        auth: UserAuth::api_key(header).unwrap(),
        secret: Some(ToolSecret::new(secret)),
    }
}

fn none(name: &str, url: &str) -> AddUserToolServer {
    AddUserToolServer {
        name: name.to_owned(),
        url: url.to_owned(),
        auth: UserAuth::None,
        secret: None,
    }
}

fn oauth2(name: &str, url: &str, scopes: &[&str]) -> AddUserToolServer {
    AddUserToolServer {
        name: name.to_owned(),
        url: url.to_owned(),
        auth: UserAuth::oauth2(scopes.iter().map(|s| (*s).to_owned()).collect(), None).unwrap(),
        secret: None,
    }
}

fn is(error: &AppError, refusal: &UserToolRefusal) -> bool {
    matches!(error, AppError::UserTool(r) if r == refusal)
}

fn status_of(server: &OwnToolServer) -> (&'static str, Option<&'static str>) {
    (
        server.status().as_str(),
        server.reason().map(OwnReason::as_str),
    )
}

// ------------------------------------------------------------------------- the feature is off

#[tokio::test]
async fn off_is_a_feature_that_is_absent_for_every_call_and_no_port_is_touched() {
    let rig = Rig::off();
    let mut state = None;
    assert!(matches!(
        rig.app.list_own_tool_servers(&alice()).await.unwrap(),
        OwnServers::Off
    ));
    for error in [
        rig.try_add(&alice(), none("x", URL)).await.unwrap_err(),
        rig.app
            .edit_user_tool_server(
                &alice(),
                "u-abcdefghij",
                EditUserToolServer {
                    name: Some("y".into()),
                    ..EditUserToolServer::default()
                },
            )
            .await
            .unwrap_err(),
        rig.app
            .delete_user_tool_server(&alice(), "u-abcdefghij")
            .await
            .unwrap_err(),
    ] {
        assert!(is(&error, &UserToolRefusal::Off), "{error:?}");
        assert_eq!(error.class(), ErrorClass::NotFound);
        state = Some(error);
    }
    assert!(state.is_some());
    assert!(rig.broker.calls().is_empty());
}

// ------------------------------------------------------------------------------------- add

#[tokio::test]
async fn a_none_server_is_connected_at_once_and_the_broker_is_not_asked() {
    let rig = Rig::full();
    let added = rig.add(&alice(), none("  Docs  ", URL)).await;
    assert_eq!(added.name(), "Docs", "the name is trimmed");
    assert_eq!(added.state(), UserToolServerState::Connected);
    assert_eq!(status_of(&added), ("ready", None));
    assert!(!added.has_credential());
    assert_eq!(added.url(), URL);
    assert!(added.id().as_str().starts_with("u-"));
    assert!(rig.broker.calls().is_empty());
    assert_eq!(rig.listed(&alice()).await.len(), 1);
}

#[tokio::test]
async fn a_bearer_server_is_inserted_then_keyed_then_replaced_as_connected() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("GitHub", URL, SENTINEL)).await;
    assert_eq!(added.state(), UserToolServerState::Connected);
    assert!(added.has_credential());
    assert_eq!(status_of(&added), ("ready", None));
    // the definition was there, `added`, with its URL, when the broker was asked to keep the key
    assert_eq!(
        rig.broker.calls(),
        vec![Call::Store {
            id: added.id().clone(),
            url: Some(URL.to_owned()),
            state: Some(UserToolServerState::Added),
        }]
    );
    // the broker holds the secret; the store holds a reference to it
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert!(
        stored
            .server()
            .credential()
            .unwrap()
            .as_str()
            .starts_with("cred-")
    );
    assert!(!format!("{stored:?}").contains(SENTINEL));
}

#[tokio::test]
async fn an_api_key_server_keeps_its_header_and_is_connected() {
    let rig = Rig::full();
    let added = rig
        .add(&alice(), api_key("Search", URL, "X-Api-Key", SENTINEL))
        .await;
    assert_eq!(added.auth().header_name(), Some("X-Api-Key"));
    assert_eq!(added.state(), UserToolServerState::Connected);
    assert!(added.has_credential());
}

#[tokio::test]
async fn an_oauth2_server_is_added_and_waits_for_the_person_to_authorize() {
    let rig = Rig::full();
    let added = rig
        .add(&alice(), oauth2("Notion", URL, &["read", "write"]))
        .await;
    assert_eq!(added.state(), UserToolServerState::Added);
    assert!(!added.has_credential());
    assert_eq!(status_of(&added), ("needs_auth", Some("not_connected")));
    assert!(rig.broker.calls().is_empty(), "authorizing is slice 4c's");
    assert_eq!(added.auth().scopes(), ["read", "write"]);
}

#[tokio::test]
async fn adding_needs_the_permission_and_writes_nothing_without_it() {
    let rig = Rig::full();
    let error = rig
        .try_add(&carol(), bearer("x", URL, SENTINEL))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::Forbidden { permission, .. } if *permission == Permission::ToolsUserAdd),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Forbidden);
    assert!(rig.listed(&carol()).await.is_empty());
    assert!(rig.broker.calls().is_empty());
    // `admin` does not imply it either: a person with no `maker` role is refused
    let admin = who("root@example.com", &["user"]);
    assert!(rig.try_add(&admin, none("x", URL)).await.is_err());
}

#[tokio::test]
async fn everything_a_person_typed_is_checked_before_any_write() {
    let rig = Rig::full();
    let secret = |s: &str| Some(ToolSecret::new(s));
    let too_long = "k".repeat(orch_app::MAX_USER_SECRET_BYTES + 1);
    let cases: Vec<(&str, AddUserToolServer)> = vec![
        (
            "plain http",
            bearer("x", "http://mcp.example.com/mcp", SENTINEL),
        ),
        (
            "a private address",
            bearer("x", "https://10.0.0.1/mcp", SENTINEL),
        ),
        (
            "the metadata address",
            bearer("x", "https://169.254.169.254/", SENTINEL),
        ),
        ("localhost", bearer("x", "https://localhost/mcp", SENTINEL)),
        (
            "user information",
            bearer("x", "https://u:p@mcp.example.com/mcp", SENTINEL),
        ),
        (
            "a query",
            bearer("x", "https://mcp.example.com/mcp?key=1", SENTINEL),
        ),
        ("not a url", bearer("x", "mcp.example.com", SENTINEL)),
        ("an empty name", bearer("   ", URL, SENTINEL)),
        ("a long name", bearer(&"n".repeat(81), URL, SENTINEL)),
        ("a name with a newline", bearer("a\nb", URL, SENTINEL)),
        (
            "a missing secret",
            AddUserToolServer {
                secret: None,
                ..bearer("x", URL, "")
            },
        ),
        ("an empty secret", bearer("x", URL, "")),
        ("a secret with a newline", bearer("x", URL, "abc\ndef")),
        (
            "a secret with a control character",
            bearer("x", URL, "abc\u{7}def"),
        ),
        (
            "a secret that is not ASCII",
            bearer("x", URL, "clé-secrète"),
        ),
        ("a secret with a space at its end", bearer("x", URL, "abc ")),
        ("a secret that is too long", bearer("x", URL, &too_long)),
        (
            "a secret for a server that wants none",
            AddUserToolServer {
                secret: secret("abc"),
                ..none("x", URL)
            },
        ),
        (
            "a secret for an oauth2 server",
            AddUserToolServer {
                secret: secret("abc"),
                ..oauth2("x", URL, &[])
            },
        ),
        (
            "a header the orchestrator owns",
            AddUserToolServer {
                auth: UserAuth::ApiKey {
                    header: "Authorization".into(),
                },
                ..bearer("x", URL, SENTINEL)
            },
        ),
        (
            "a scope with a quote",
            AddUserToolServer {
                auth: UserAuth::OAuth2 {
                    scopes: vec!["a\"b".into()],
                    client_id: None,
                },
                secret: None,
                ..none("x", URL)
            },
        ),
    ];
    for (label, request) in cases {
        let error = rig.try_add(&alice(), request).await.unwrap_err();
        assert!(
            matches!(error.class(), ErrorClass::Invalid),
            "{label}: {error:?}"
        );
        // the message names no URL and no secret
        let text = format!("{} / {}", error, report(&error));
        for leaked in [SENTINEL, "mcp.example.com", "10.0.0.1", "abc"] {
            assert!(!text.contains(leaked), "{label}: {text}");
        }
    }
    assert!(rig.listed(&alice()).await.is_empty(), "nothing was written");
    assert!(rig.broker.calls().is_empty(), "the broker was not asked");
}

#[tokio::test]
async fn a_url_the_guard_refuses_is_shown_the_public_reason_and_never_the_rule() {
    let egress =
        EgressPolicy::from_lists(Vec::<String>::new(), ["blocked.example.com".to_owned()]).unwrap();
    let rig = Rig::full_with(egress);
    for (url, shown) in [
        // a private address and a name that is always loopback say one thing
        (
            "https://10.0.0.1/mcp",
            "that server cannot be reached from here",
        ),
        (
            "https://localhost/mcp",
            "that server cannot be reached from here",
        ),
        (
            "https://blocked.example.com/mcp",
            "the host is on the deployment's deny list",
        ),
        (
            "http://mcp.example.com/",
            "the URL scheme is not allowed: https only",
        ),
    ] {
        let error = rig.try_add(&alice(), none("x", url)).await.unwrap_err();
        let AppError::UserTool(UserToolRefusal::UrlRefused { reason }) = &error else {
            panic!("{url}: {error:?}");
        };
        assert_eq!(reason, shown, "{url}");
        // the precise rule of the operator's log is not what a person reads
        for rule in ["private-address", "no-address", "deny-list", "allow-list"] {
            assert!(!error.to_string().contains(rule), "{url}: {error}");
        }
        assert_eq!(
            UserToolRefusal::UrlRefused {
                reason: String::new()
            }
            .code(),
            Some("url_refused")
        );
        assert_eq!(error.class(), ErrorClass::Invalid);
    }
}

#[tokio::test]
async fn a_deployment_that_keeps_no_credentials_takes_only_servers_that_want_none() {
    let rig = Rig::without_credentials();
    let added = rig.add(&alice(), none("Docs", URL)).await;
    assert_eq!(added.state(), UserToolServerState::Connected);
    for request in [
        bearer("x", URL, SENTINEL),
        api_key("x", URL, "X-Key", SENTINEL),
        oauth2("x", URL, &[]),
    ] {
        let error = rig.try_add(&alice(), request).await.unwrap_err();
        assert!(is(&error, &UserToolRefusal::NoCredentials), "{error:?}");
        assert_eq!(error.class(), ErrorClass::Rejected);
    }
    assert_eq!(rig.listed(&alice()).await.len(), 1);
    assert!(
        rig.broker.calls().is_empty(),
        "the broker refuses and is not asked"
    );
}

#[tokio::test]
async fn the_ninth_server_is_refused_and_a_failed_add_does_not_cost_a_place() {
    let rig = Rig::full();
    for n in 0..8 {
        rig.add(&alice(), none(&format!("s{n}"), URL)).await;
    }
    let error = rig.try_add(&alice(), none("ninth", URL)).await.unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::TooMany { limit: 8 }),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Rejected);
    assert_eq!(rig.listed(&alice()).await.len(), 8);
    // another person has their own eight
    rig.add(&bob(), none("mine", URL)).await;

    // a failed add: the broker cannot keep the key, and the place is given back
    let rig = Rig::full();
    for n in 0..7 {
        rig.add(&alice(), none(&format!("s{n}"), URL)).await;
    }
    rig.broker.brain().fail_store = true;
    for _ in 0..3 {
        let error = rig
            .try_add(&alice(), bearer("k", URL, SENTINEL))
            .await
            .unwrap_err();
        assert!(
            is(&error, &UserToolRefusal::CredentialsUnavailable),
            "{error:?}"
        );
    }
    assert_eq!(
        rig.listed(&alice()).await.len(),
        7,
        "a failed add leaves nothing"
    );
    rig.broker.brain().fail_store = false;
    rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    assert_eq!(rig.listed(&alice()).await.len(), 8);
}

#[tokio::test]
async fn an_add_whose_last_write_fails_forgets_the_key_it_kept() {
    // the `replace` that would connect the server fails, or another writer got there first: the
    // broker holds a key that no definition names, and the add forgets it (best effort)
    for (step, class) in [
        (Step::Fail, ErrorClass::Transient),
        (Step::Race, ErrorClass::Conflict),
    ] {
        let rig = Rig::full();
        rig.servers.script_replaces([step]);
        let error = rig
            .try_add(&alice(), bearer("k", URL, SENTINEL))
            .await
            .unwrap_err();
        assert_eq!(error.class(), class, "{step:?}: {error:?}");
        if step == Step::Race {
            // the server was changed under the add: busy, and not a thread's contention
            assert!(is(&error, &UserToolRefusal::Busy), "{error:?}");
        }
        let calls = rig.broker.calls();
        assert!(
            matches!(calls.as_slice(), [Call::Store { .. }, Call::Revoke { .. }]),
            "{step:?}: {calls:?}"
        );
        assert!(
            rig.broker.sealed().is_empty(),
            "{step:?}: the key was left behind"
        );
        // the definition is not taken back (the person may have changed it): it is `added`
        let listed = rig.listed(&alice()).await;
        assert_eq!(listed.len(), 1, "{step:?}");
        assert_eq!(status_of(&listed[0]), ("needs_auth", Some("not_connected")));
    }
}

#[tokio::test]
async fn a_store_that_may_have_kept_the_key_is_revoked_before_the_definition_is_taken_back() {
    let rig = Rig::full();
    // the broker kept the key and the answer was lost
    rig.broker.brain().keeps_then_fails_store = true;
    let error = rig
        .try_add(&alice(), bearer("k", URL, SENTINEL))
        .await
        .unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::CredentialsUnavailable),
        "{error:?}"
    );
    assert!(rig.listed(&alice()).await.is_empty(), "taken back");
    assert!(
        rig.broker.sealed().is_empty(),
        "the key the broker kept is forgotten with the definition"
    );
    let calls = rig.broker.calls();
    assert!(
        matches!(calls.as_slice(), [Call::Store { .. }, Call::Revoke { .. }]),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_definition_that_cannot_be_taken_back_stays_added_and_can_be_deleted() {
    let rig = Rig::full();
    {
        let mut brain = rig.broker.brain();
        brain.fail_store = true;
        brain.take_servers_down_on_failed_store = true;
    }
    let error = rig
        .try_add(&alice(), bearer("k", URL, SENTINEL))
        .await
        .unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::CredentialsUnavailable),
        "{error:?}"
    );
    // the store is back: the server is there, `added`, needing its key, and it can go
    rig.servers.refuse.store(false, Ordering::SeqCst);
    rig.broker.brain().fail_store = false;
    let listed = rig.listed(&alice()).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(status_of(&listed[0]), ("needs_auth", Some("not_connected")));
    rig.app
        .delete_user_tool_server(&alice(), listed[0].id().as_str())
        .await
        .unwrap();
    assert!(rig.listed(&alice()).await.is_empty());
}

// ------------------------------------------------------------------------------------ list

#[tokio::test]
async fn nobody_lists_or_touches_another_persons_servers() {
    let rig = Rig::full();
    let mine = rig.add(&alice(), bearer("mine", URL, SENTINEL)).await;
    assert!(rig.listed(&bob()).await.is_empty());
    let id = mine.id().as_str();
    let edit = EditUserToolServer {
        name: Some("stolen".into()),
        ..EditUserToolServer::default()
    };
    let error = rig
        .app
        .edit_user_tool_server(&bob(), id, edit)
        .await
        .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NotFound), "{error:?}");
    let error = rig
        .app
        .delete_user_tool_server(&bob(), id)
        .await
        .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NotFound), "{error:?}");
    assert_eq!(error.class(), ErrorClass::NotFound);
    // the same answer as for an id that was never made, and for one that is not an id
    for id in [
        "u-0000000000",
        "nonsense",
        "",
        "U-ABCDEFGHIJ",
        "u-abcdefghij1",
    ] {
        let error = rig
            .app
            .delete_user_tool_server(&bob(), id)
            .await
            .unwrap_err();
        assert!(is(&error, &UserToolRefusal::NotFound), "{id}: {error:?}");
    }
    // nothing of Alice's changed, and no revoke was asked for her server
    assert_eq!(rig.listed(&alice()).await[0].name(), "mine");
    assert!(
        rig.broker
            .calls()
            .iter()
            .all(|c| !matches!(c, Call::Revoke { id, .. } if id == mine.id())),
    );
}

#[tokio::test]
async fn the_listing_is_sorted_by_name_and_says_what_each_server_is_without_asking_the_broker() {
    let rig = Rig::full();
    let beta = rig.add(&alice(), none("beta", URL)).await;
    let alpha = rig.add(&alice(), bearer("Alpha", URL, SENTINEL)).await;
    let gamma = rig.add(&alice(), oauth2("gamma", URL, &[])).await;
    // the broker learned the grant ended, and the person disconnected another
    let stored = rig.stored(&alice(), alpha.id()).await.unwrap();
    rig.servers
        .inner
        .set_state(
            &alice().user,
            alpha.id(),
            UserToolServerState::NeedsReauth,
            stored.revision(),
        )
        .await
        .unwrap();
    let stored = rig.stored(&alice(), gamma.id()).await.unwrap();
    rig.servers
        .inner
        .set_state(
            &alice().user,
            gamma.id(),
            UserToolServerState::Revoked,
            stored.revision(),
        )
        .await
        .unwrap();
    let calls_before = rig.broker.calls().len();
    let listed = rig.listed(&alice()).await;
    assert_eq!(
        listed.iter().map(|s| s.name()).collect::<Vec<_>>(),
        ["Alpha", "beta", "gamma"]
    );
    assert_eq!(status_of(&listed[0]), ("needs_auth", Some("reauthorize")));
    assert_eq!(status_of(&listed[1]), ("ready", None));
    assert_eq!(status_of(&listed[2]), ("needs_auth", Some("disconnected")));
    assert_eq!(listed[1].id(), beta.id());
    assert_eq!(
        rig.broker.calls().len(),
        calls_before,
        "a listing leases nothing"
    );
}

#[tokio::test]
async fn without_the_permission_the_servers_are_unavailable_not_permitted() {
    let rig = Rig::full();
    rig.add(&alice(), bearer("GitHub", URL, SENTINEL)).await;
    // Alice's roles changed: she is a plain user now
    let demoted = who("alice@example.com", &["user"]);
    let listed = rig.listed(&demoted).await;
    assert_eq!(listed.len(), 1, "she still sees what she added");
    assert_eq!(
        status_of(&listed[0]),
        ("unavailable", Some("not_permitted"))
    );
    assert_eq!(
        listed[0].status(),
        ToolServerStatus::Unavailable(UnavailableReason::NotPermitted)
    );
    // and she cannot add or edit, but she can remove it (decision 4)
    assert!(rig.try_add(&demoted, none("x", URL)).await.is_err());
    let edit = EditUserToolServer {
        name: Some("y".into()),
        ..EditUserToolServer::default()
    };
    let error = rig
        .app
        .edit_user_tool_server(&demoted, listed[0].id().as_str(), edit)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Forbidden { .. }), "{error:?}");
    rig.app
        .delete_user_tool_server(&demoted, listed[0].id().as_str())
        .await
        .unwrap();
    assert!(rig.listed(&alice()).await.is_empty());
    assert!(
        rig.broker
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Revoke { .. })),
        "the credential went with it"
    );
}

#[tokio::test]
async fn a_server_the_lists_refuse_today_is_unavailable_egress_refused_and_can_still_be_deleted() {
    let rig = Rig::full();
    let added = rig.add(&alice(), none("Docs", URL)).await;
    // the operator denied the host since
    let strict = rig.with_egress(
        EgressPolicy::from_lists(Vec::<String>::new(), ["mcp.example.com".to_owned()]).unwrap(),
    );
    let OwnServers::Listed(listed) = strict.list_own_tool_servers(&alice()).await.unwrap() else {
        panic!("listed");
    };
    assert_eq!(
        status_of(&listed[0]),
        ("unavailable", Some("egress_refused"))
    );
    strict
        .delete_user_tool_server(&alice(), added.id().as_str())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_credential_kind_in_a_deployment_that_lost_its_secrets_is_unavailable_broker_down() {
    // a server that holds a credential reference in a deployment that keeps none (the operator took
    // the section away): written straight to the definitions
    let rig = Rig::without_credentials();
    let target = EgressPolicy::new().check_url(URL).unwrap();
    let stranded = UserToolServer::new(
        UserToolServerId::generate(),
        "Stranded",
        &target,
        UserAuth::Bearer,
    )
    .unwrap()
    .with_state(UserToolServerState::Connected)
    .with_credential(CredentialRef::new("cred-old-v1").unwrap());
    rig.servers
        .inner
        .insert(&alice().user, &stranded)
        .await
        .unwrap();
    let listed = rig.listed(&alice()).await;
    assert_eq!(status_of(&listed[0]), ("unavailable", Some("broker_down")));
    // its owner can edit its name, cannot move it, and can delete it without a revoke
    let rename = EditUserToolServer {
        name: Some("Renamed".into()),
        ..EditUserToolServer::default()
    };
    let renamed = rig
        .app
        .edit_user_tool_server(&alice(), stranded.id().as_str(), rename)
        .await
        .unwrap();
    assert_eq!(renamed.name(), "Renamed");
    assert!(renamed.has_credential(), "the reference is kept");
    let moved = EditUserToolServer {
        url: Some(OTHER_URL.into()),
        ..EditUserToolServer::default()
    };
    let error = rig
        .app
        .edit_user_tool_server(&alice(), stranded.id().as_str(), moved)
        .await
        .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NoCredentials), "{error:?}");
    rig.app
        .delete_user_tool_server(&alice(), stranded.id().as_str())
        .await
        .unwrap();
    assert!(rig.listed(&alice()).await.is_empty());
    assert!(
        rig.broker.calls().is_empty(),
        "revoke is not called in a deployment that keeps no credentials (departure 77)"
    );
}

#[tokio::test]
async fn a_row_that_does_not_read_back_is_reported_and_deleted_by_its_id() {
    let rig = Rig::full();
    let keyed = rig.add(&alice(), bearer("GitHub", URL, SENTINEL)).await;
    let other = rig.add(&bob(), none("Bob's", URL)).await;
    rig.servers.corrupt.store(true, Ordering::SeqCst);

    // the whole listing of the person fails in the store; the application says so, it does not fail
    assert!(matches!(
        rig.app.list_own_tool_servers(&alice()).await.unwrap(),
        OwnServers::Unreadable
    ));
    // a read of an edit is a store failure, not a guess
    let edit = EditUserToolServer {
        name: Some("y".into()),
        ..EditUserToolServer::default()
    };
    assert!(
        rig.app
            .edit_user_tool_server(&alice(), keyed.id().as_str(), edit)
            .await
            .is_err()
    );
    // deleting by id needs no read: the credential goes (the kind is not known), then the row
    rig.app
        .delete_user_tool_server(&alice(), keyed.id().as_str())
        .await
        .unwrap();
    assert_eq!(
        rig.broker.calls(),
        vec![
            Call::Store {
                id: keyed.id().clone(),
                url: Some(URL.to_owned()),
                state: Some(UserToolServerState::Added),
            },
            Call::Revoke {
                id: keyed.id().clone(),
                url: Some(URL.to_owned()), // the broker, unlike the listing, does not need the row to read back
            },
        ]
    );
    rig.servers.corrupt.store(false, Ordering::SeqCst);
    assert!(rig.listed(&alice()).await.is_empty());
    assert_eq!(rig.listed(&bob()).await[0].id(), other.id());
}

#[tokio::test]
async fn a_store_that_is_down_is_a_storage_error_and_not_an_empty_list() {
    let rig = Rig::full();
    rig.servers.inner.set_down(true);
    let error = rig.app.list_own_tool_servers(&alice()).await.unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient);
    assert!(matches!(error, AppError::Store(_)), "{error:?}");
    let error = rig.try_add(&alice(), none("x", URL)).await.unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient);
}

// ----------------------------------------------------------------------------------- edit

fn edit() -> EditUserToolServer {
    EditUserToolServer::default()
}

async fn edited(
    rig: &Rig,
    who: &Principal,
    id: &UserToolServerId,
    edit: EditUserToolServer,
) -> Result<OwnToolServer, AppError> {
    rig.app.edit_user_tool_server(who, id.as_str(), edit).await
}

#[tokio::test]
async fn a_new_name_keeps_the_credential_and_asks_the_broker_for_nothing() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("old", URL, SENTINEL)).await;
    let before = rig.broker.calls().len();
    let renamed = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("new".into()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(renamed.name(), "new");
    assert_eq!(renamed.state(), UserToolServerState::Connected);
    assert!(renamed.has_credential());
    assert_ne!(renamed.revision(), added.revision());
    assert_eq!(rig.broker.calls().len(), before);
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn an_edit_that_changes_nothing_writes_nothing() {
    let rig = Rig::full();
    let added = rig
        .add(&alice(), api_key("same", URL, "X-Key", SENTINEL))
        .await;
    let same = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("same".into()),
            url: Some(URL.into()),
            auth: Some(UserAuth::api_key("X-Key").unwrap()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        same.revision(),
        added.revision(),
        "no write, so no new revision"
    );
    // and an edit that names nothing is the sender's mistake
    let error = edited(&rig, &alice(), added.id(), edit())
        .await
        .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Invalid);
}

#[tokio::test]
async fn a_new_header_keeps_the_credential_since_the_header_is_the_definitions() {
    let rig = Rig::full();
    let added = rig
        .add(&alice(), api_key("k", URL, "X-Key", SENTINEL))
        .await;
    let before = rig.broker.calls().len();
    let moved = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(UserAuth::api_key("X-Other-Key").unwrap()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.auth().header_name(), Some("X-Other-Key"));
    assert_eq!(moved.state(), UserToolServerState::Connected);
    assert!(moved.has_credential());
    assert_eq!(rig.broker.calls().len(), before, "nothing revoked");
}

#[tokio::test]
async fn a_new_key_is_stored_then_the_server_is_replaced_as_connected() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "first-secret")).await;
    // the broker learned the grant ended
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    rig.servers
        .inner
        .set_state(
            &alice().user,
            added.id(),
            UserToolServerState::NeedsReauth,
            stored.revision(),
        )
        .await
        .unwrap();
    let rekeyed = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(rekeyed.state(), UserToolServerState::Connected);
    assert_eq!(status_of(&rekeyed), ("ready", None));
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
    // no revoke: one write after the store
    assert!(
        rig.broker
            .calls()
            .iter()
            .all(|c| !matches!(c, Call::Revoke { .. }))
    );
    // a key for a server that wants none is the sender's mistake
    let docs = rig.add(&alice(), none("docs", URL)).await;
    let error = edited(
        &rig,
        &alice(),
        docs.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new("abc")),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Invalid);
}

#[tokio::test]
async fn a_new_url_replaces_the_server_as_added_with_no_credential_and_then_revokes() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    let moved = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.url(), OTHER_URL);
    assert_eq!(moved.state(), UserToolServerState::Added);
    assert!(!moved.has_credential());
    assert_eq!(status_of(&moved), ("needs_auth", Some("not_connected")));
    // the definition was written first (the compare-and-swap), so the revoke found the NEW one: the
    // real broker reads no definition to revoke (departure 21), and the order is what matters
    assert_eq!(
        rig.broker.calls().last().unwrap(),
        &Call::Revoke {
            id: added.id().clone(),
            url: Some(OTHER_URL.to_owned()),
        }
    );
    assert!(rig.broker.sealed().is_empty());
}

#[tokio::test]
async fn a_new_url_with_a_key_replaces_revokes_then_stores_for_the_new_url() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    let moved = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.state(), UserToolServerState::Connected);
    assert!(moved.has_credential());
    let calls = rig.broker.calls();
    assert_eq!(
        &calls[1..],
        &[
            Call::Revoke {
                id: added.id().clone(),
                url: Some(OTHER_URL.to_owned()),
            },
            // the definition had been replaced when the broker read it: the key is for the new URL
            Call::Store {
                id: added.id().clone(),
                url: Some(OTHER_URL.to_owned()),
                state: Some(UserToolServerState::Added),
            },
        ]
    );
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn a_new_kind_revokes_what_the_old_one_held() {
    let rig = Rig::full();
    // bearer to none: connected, nothing to hold
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    let none_now = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(UserAuth::None),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(none_now.state(), UserToolServerState::Connected);
    assert!(!none_now.has_credential());
    assert!(rig.broker.sealed().is_empty());
    // none to oauth2: nothing was held, so nothing is revoked, and it waits for an authorization
    let revokes = rig
        .broker
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Revoke { .. }))
        .count();
    let oauth = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(UserAuth::oauth2(vec!["read".into()], None).unwrap()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(oauth.state(), UserToolServerState::Added);
    assert_eq!(
        rig.broker
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Revoke { .. }))
            .count(),
        revokes
    );
    // oauth2 to bearer with a key
    let bearer_now = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(UserAuth::Bearer),
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(bearer_now.state(), UserToolServerState::Connected);
}

#[tokio::test]
async fn other_scopes_or_another_client_are_a_new_grant_and_the_same_scopes_in_another_order_are_not()
 {
    let rig = Rig::full();
    let added = rig
        .add(&alice(), oauth2("n", URL, &["read", "write"]))
        .await;
    // the person authorized: the broker's work, shown here by the state
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    let connected = stored
        .server()
        .clone()
        .with_state(UserToolServerState::Connected)
        .with_credential(CredentialRef::new("cred-x-v1").unwrap());
    rig.servers
        .inner
        .replace(&alice().user, &connected, stored.revision())
        .await
        .unwrap();
    let revokes = |rig: &Rig| {
        rig.broker
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Revoke { .. }))
            .count()
    };
    let reordered = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(UserAuth::oauth2(vec!["write".into(), "read".into()], None).unwrap()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reordered.state(),
        UserToolServerState::Connected,
        "the grant stands"
    );
    assert_eq!(revokes(&rig), 0);
    let wider = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            auth: Some(
                UserAuth::oauth2(vec!["read".into(), "write".into(), "admin".into()], None)
                    .unwrap(),
            ),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        wider.state(),
        UserToolServerState::Added,
        "new scopes need a new authorization"
    );
    assert_eq!(revokes(&rig), 1);
}

#[tokio::test]
async fn a_failed_revoke_after_the_move_says_so_and_leaves_the_server_added() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    rig.broker.brain().fail_revoke = true;
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            name: Some("moved".into()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::CredentialsUnavailable),
        "{error:?}"
    );
    // the definition was written first and says what the person asked for: `added`, no credential
    // named, so what the broker still holds is named by nothing and is not offered
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), OTHER_URL);
    assert_eq!(stored.server().name(), "moved");
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(stored.server().credential().is_none());
    let listed = rig.listed(&alice()).await;
    assert_eq!(status_of(&listed[0]), ("needs_auth", Some("not_connected")));
    // the delete forgets what is left
    rig.broker.brain().fail_revoke = false;
    rig.app
        .delete_user_tool_server(&alice(), added.id().as_str())
        .await
        .unwrap();
    assert!(rig.broker.sealed().is_empty());
}

#[tokio::test]
async fn a_move_whose_write_fails_has_revoked_nothing_and_the_server_keeps_its_credential() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    // the store fails the compare-and-swap, which is the first write of a move
    rig.servers.script_replaces([Step::Fail]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient, "{error:?}");
    // nothing was revoked: the definition still names a credential, and the broker still has it
    assert!(
        rig.broker
            .calls()
            .iter()
            .all(|c| !matches!(c, Call::Revoke { .. })),
        "{:?}",
        rig.broker.calls()
    );
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), URL);
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert!(stored.server().credential().is_some());
    assert_eq!(stored.revision(), added.revision());
}

#[tokio::test]
async fn a_move_that_loses_the_race_has_revoked_nothing_and_with_a_revision_is_stale() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    // the revision named is the current one when the server is read; another writer moves it
    // before the edit writes
    rig.servers.script_replaces([Step::Race]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    assert!(
        rig.broker
            .calls()
            .iter()
            .all(|c| !matches!(c, Call::Revoke { .. })),
        "a 409 says nothing is written, and a credential is something: {:?}",
        rig.broker.calls()
    );
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), URL);
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert!(stored.server().credential().is_some());
}

#[tokio::test]
async fn a_failed_store_after_a_move_leaves_the_server_added_and_the_old_key_gone() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.broker.brain().fail_store = true;
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::CredentialsUnavailable),
        "{error:?}"
    );
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), OTHER_URL);
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(stored.server().credential().is_none());
    assert!(rig.broker.sealed().is_empty());
}

#[tokio::test]
async fn an_edit_based_on_a_stale_revision_is_refused_and_writes_nothing() {
    let rig = Rig::full();
    let added = rig.add(&alice(), none("a", URL)).await;
    let first = added.revision();
    let renamed = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("b".into()),
            revision: Some(first),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_ne!(renamed.revision(), first);
    // the same revision again: the server changed since it was read
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("c".into()),
            revision: Some(first),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    assert_eq!(rig.listed(&alice()).await[0].name(), "b");
    // the current one is fine
    edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("c".into()),
            revision: Some(renamed.revision()),
            ..edit()
        },
    )
    .await
    .unwrap();
    // a revision that was never one is stale too
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("d".into()),
            revision: Some(ServerRevision::new(987_654_321)),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
}

#[tokio::test]
async fn a_write_in_between_is_read_again_without_a_revision_and_is_stale_with_one() {
    let revokes = |rig: &Rig| {
        rig.broker
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Revoke { .. }))
            .count()
    };
    // another writer moves the revision between the edit's read and its write
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    rig.servers.script_replaces([Step::Race]);
    let moved = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.url(), OTHER_URL);
    assert_eq!(moved.state(), UserToolServerState::Added);
    assert_eq!(
        revokes(&rig),
        1,
        "the first pass lost its write and revoked nothing; the second revoked once"
    );
    assert!(rig.broker.sealed().is_empty());

    // with the revision the person read, the same race is theirs to read again
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    rig.servers.script_replaces([Step::Race]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    assert_eq!(
        rig.stored(&alice(), added.id())
            .await
            .unwrap()
            .server()
            .url(),
        URL
    );
    assert_eq!(revokes(&rig), 0);

    // a writer that never stops is Contended, and nothing was revoked
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    rig.servers.script_replaces([Step::Race; 10]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::Busy), "{error:?}");
    assert_eq!(UserToolRefusal::Busy.code(), None);
    assert_eq!(error.class(), ErrorClass::Conflict);
    assert_eq!(revokes(&rig), 0);
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn once_the_move_is_written_a_later_conflict_does_not_make_it_stale() {
    // the broker writes the definition while the edit revokes (it learned the grant ended): the
    // edit's final write loses, and the person's revision was spent by the edit's own first write,
    // so the edit goes on to the end and does not answer 409 over a move that was made
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.broker.brain().bump_on_revoke = 1;
    let moved = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            secret: Some(ToolSecret::new(SENTINEL)),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(moved.url(), OTHER_URL);
    assert_eq!(moved.state(), UserToolServerState::Connected);
    assert!(moved.has_credential());
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn a_move_with_a_key_whose_last_write_fails_leaves_the_server_added_and_the_key_forgotten() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    // the first write (added, no credential) goes through; the one that connects it fails
    rig.servers.script_replaces([Step::Pass, Step::Fail]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            url: Some(OTHER_URL.into()),
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient, "{error:?}");
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), OTHER_URL);
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(stored.server().credential().is_none());
    // the key that was kept for it is named by nothing: forgotten, best effort
    assert!(rig.broker.sealed().is_empty(), "{:?}", rig.broker.sealed());
}

#[tokio::test]
async fn an_edit_checks_a_new_url_and_a_new_secret_before_it_reads_or_writes() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    let before = rig.broker.calls().len();
    for (label, bad) in [
        (
            "a private url",
            EditUserToolServer {
                url: Some("https://10.0.0.1/".into()),
                ..edit()
            },
        ),
        (
            "an empty name",
            EditUserToolServer {
                name: Some(" ".into()),
                ..edit()
            },
        ),
        (
            "a secret with a newline",
            EditUserToolServer {
                secret: Some(ToolSecret::new("a\nb")),
                ..edit()
            },
        ),
        (
            "a reserved header",
            EditUserToolServer {
                auth: Some(UserAuth::ApiKey {
                    header: "Host".into(),
                }),
                ..edit()
            },
        ),
    ] {
        let error = edited(&rig, &alice(), added.id(), bad).await.unwrap_err();
        assert_eq!(error.class(), ErrorClass::Invalid, "{label}: {error:?}");
    }
    assert_eq!(rig.broker.calls().len(), before, "the broker was not asked");
    assert_eq!(
        rig.stored(&alice(), added.id()).await.unwrap().revision(),
        added.revision()
    );
}

#[tokio::test]
async fn editing_a_server_that_is_gone_is_not_found() {
    let rig = Rig::full();
    let added = rig.add(&alice(), none("a", URL)).await;
    rig.app
        .delete_user_tool_server(&alice(), added.id().as_str())
        .await
        .unwrap();
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            name: Some("b".into()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NotFound), "{error:?}");
}

#[tokio::test]
async fn a_stale_rekey_is_refused_before_the_new_key_is_kept() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    // the server is read at the revision the person holds (the first `get`); another writer moves it
    // before the edit asks the broker to keep the key (the second)
    rig.servers.script_gets([Step::Pass, Step::Race]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    // "nothing is written" is true of the key too: the broker was not asked, and still has the old one
    assert_eq!(
        rig.broker
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Store { .. }))
            .count(),
        1,
        "only the add stored a key: {:?}",
        rig.broker.calls()
    );
    assert_eq!(rig.broker.sealed(), vec!["old-secret".to_owned()]);
}

#[tokio::test]
async fn a_rekey_that_loses_the_race_after_the_key_is_kept_takes_no_credential_away() {
    // the broker overwrites a key in place, so after the store the old one cannot be given back, and
    // revoking the new one would leave a `connected` server with nothing behind it
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.servers.script_replaces([Step::Race]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    assert!(
        rig.broker
            .calls()
            .iter()
            .all(|c| !matches!(c, Call::Revoke { .. })),
        "{:?}",
        rig.broker.calls()
    );
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert!(stored.server().credential().is_some());
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);

    // without a revision the edit reads the server again and keeps the key once more
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.servers.script_replaces([Step::Race]);
    let rekeyed = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(rekeyed.state(), UserToolServerState::Connected);
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn a_rekey_that_loses_to_a_move_forgets_the_key_the_move_left_without_a_name() {
    // another request moved the server (a new URL, `added`, no credential named) after this edit
    // read it and before the broker kept the key: the broker read the NEW definition and kept the
    // key for the new URL, and the definition names no credential. Nothing relies on that key, and
    // the broker would lend it for that URL: it is forgotten, and the edit reads the server again
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.broker.brain().move_before_store = true;
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            revision: Some(added.revision()),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::StaleRevision), "{error:?}");
    let calls = rig.broker.calls();
    assert_eq!(
        &calls[1..],
        &[
            Call::Store {
                id: added.id().clone(),
                url: Some(OTHER_URL.to_owned()),
                state: Some(UserToolServerState::Added),
            },
            Call::Revoke {
                id: added.id().clone(),
                url: Some(OTHER_URL.to_owned()),
            },
        ],
        "{calls:?}"
    );
    assert!(
        rig.broker.sealed().is_empty(),
        "the key kept for a definition that names none was left behind"
    );
    let stored = rig.stored(&alice(), added.id()).await.unwrap();
    assert_eq!(stored.server().url(), OTHER_URL);
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(stored.server().credential().is_none());

    // without a revision the edit reads the server as the move left it and keeps the key again
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    rig.broker.brain().move_before_store = true;
    let rekeyed = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap();
    assert_eq!(rekeyed.url(), OTHER_URL);
    assert_eq!(rekeyed.state(), UserToolServerState::Connected);
    assert!(rekeyed.has_credential());
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
}

#[tokio::test]
async fn a_rekey_of_a_server_deleted_meanwhile_forgets_the_key_it_kept() {
    let rig = Rig::full();
    let added = rig.add(&alice(), bearer("k", URL, "old-secret")).await;
    // the server is deleted after the broker kept the key and before the edit writes
    rig.servers.script_replaces([Step::Gone]);
    let error = edited(
        &rig,
        &alice(),
        added.id(),
        EditUserToolServer {
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        },
    )
    .await
    .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NotFound), "{error:?}");
    assert!(rig.stored(&alice(), added.id()).await.is_none());
    assert!(
        rig.broker.sealed().is_empty(),
        "the key was left behind for a server that is gone"
    );
}

// --------------------------------------------------------------------------------- delete

#[tokio::test]
async fn deleting_revokes_first_when_the_server_holds_a_credential_then_deletes() {
    let rig = Rig::full();
    let keyed = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    let docs = rig.add(&alice(), none("docs", URL)).await;
    rig.app
        .delete_user_tool_server(&alice(), keyed.id().as_str())
        .await
        .unwrap();
    assert_eq!(
        rig.broker.calls().last().unwrap(),
        &Call::Revoke {
            id: keyed.id().clone(),
            // the definition was still there when the broker was asked
            url: Some(URL.to_owned()),
        }
    );
    assert!(rig.broker.sealed().is_empty());
    assert!(rig.stored(&alice(), keyed.id()).await.is_none());
    // a server that wants none has no credential to revoke
    let calls = rig.broker.calls().len();
    rig.app
        .delete_user_tool_server(&alice(), docs.id().as_str())
        .await
        .unwrap();
    assert_eq!(rig.broker.calls().len(), calls);
    // the second time it is not there
    let error = rig
        .app
        .delete_user_tool_server(&alice(), docs.id().as_str())
        .await
        .unwrap_err();
    assert!(is(&error, &UserToolRefusal::NotFound), "{error:?}");
}

#[tokio::test]
async fn a_failed_revoke_aborts_the_delete_and_the_server_stays() {
    let rig = Rig::full();
    let keyed = rig.add(&alice(), bearer("k", URL, SENTINEL)).await;
    rig.broker.brain().fail_revoke = true;
    let error = rig
        .app
        .delete_user_tool_server(&alice(), keyed.id().as_str())
        .await
        .unwrap_err();
    assert!(
        is(&error, &UserToolRefusal::CredentialsUnavailable),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Transient);
    assert!(rig.stored(&alice(), keyed.id()).await.is_some());
    assert_eq!(rig.broker.sealed(), vec![SENTINEL.to_owned()]);
    // it goes when the broker is back
    rig.broker.brain().fail_revoke = false;
    rig.app
        .delete_user_tool_server(&alice(), keyed.id().as_str())
        .await
        .unwrap();
}

// ----------------------------------------------------------------------------- the secret

#[tokio::test]
async fn no_error_and_no_debug_holds_the_secret_the_url_or_another_persons_data() {
    let rig = Rig::full();
    // the request types print neither the secret nor the URL
    let request = bearer(
        "GitHub",
        "https://mcp.example.com/path-with-a-token-TOK123/mcp",
        SENTINEL,
    );
    let text = format!("{request:?}");
    assert!(
        !text.contains(SENTINEL) && !text.contains("TOK123"),
        "{text}"
    );
    let text = format!(
        "{:?}",
        EditUserToolServer {
            url: Some("https://mcp.example.com/path-with-a-token-TOK123/mcp".into()),
            secret: Some(ToolSecret::new(SENTINEL)),
            ..edit()
        }
    );
    assert!(
        !text.contains(SENTINEL) && !text.contains("TOK123"),
        "{text}"
    );
    // and neither does what comes back
    let added = rig.try_add(&alice(), request).await.unwrap();
    let text = format!("{added:?}");
    assert!(
        !text.contains(SENTINEL) && !text.contains("TOK123"),
        "{text}"
    );
    // every failure of the broker, in every step, says one fixed thing
    rig.broker.brain().fail_store = true;
    let error = rig
        .try_add(&alice(), bearer("k2", URL, SENTINEL))
        .await
        .unwrap_err();
    let all = format!("{error} / {error:?} / {}", report(&error));
    assert!(!all.contains(SENTINEL), "{all}");
}
