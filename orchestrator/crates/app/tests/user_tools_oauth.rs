//! Authorizing a person's own `oauth2` MCP server in the application (ADR 0046, slice 4c): who may
//! begin, what is kept for the callback, what the callback does with a state, a code, an `iss` and an
//! `error`, and what is left when the server changes under an authorization.
//!
//! The broker here is a fake that follows the port's docs and records what it was asked; the real
//! one, against an authorization server, is exercised through HTTP in `orch-api`'s tests. The
//! pending authorizations are the in-memory store; time is a clock the test moves.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_app::{
    AddUserToolServer, AgentDirectory, App, AppConfig, AppError, AuthorizationOutcome,
    MAX_CALLBACK_CODE_BYTES, OAuthCallback, Permission, Policy, RoleGrant, UserToolRefusal,
};
use orch_core::{UserId, report};
use orch_ports::memory::{
    FixedClock, MemoryPendingAuthorizations, MemoryStore, MemoryUserToolServers, MemoryWakeup,
    ScriptedAgent, SeqIds,
};
use orch_ports::{
    Clock, CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy,
    EgressRefusal, FixedRegistry, KeepsCredentials, NoArtifacts, NoBroker, NoModel,
    NoUserToolServers, OAuthStart, OAuthState, PortSet, Principal, RedirectUri, RefuseAll, Role,
    ServerRevision, StoredServer, ToolSecret, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserTools,
};

const URL: &str = "https://mcp.example.com/mcp";
const MOVED_URL: &str = "https://moved.example.com/mcp";
const REDIRECT: &str = "https://chat.example.com/api/user-tool-servers/oauth/callback";
const ISSUER: &str = "https://auth.example.com";
/// Planted in the code and in the state: nothing the application says may carry them.
const CODE: &str = "SENTINEL-code-8e2f51a0";
const ISS_SENTINEL: &str = "https://SENTINEL-iss.example.com";
const FLOW_SECONDS: u64 = 600;

fn start_time() -> orch_core::Timestamp {
    orch_core::Timestamp::from_second(1_790_000_000).unwrap()
}

// ------------------------------------------------------------------------------ the doubles

/// What happens to the definition while the broker exchanges the code (after it was read by the
/// callback, before the callback writes): another request at the same moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Meanwhile {
    #[default]
    Nothing,
    /// A rename: the revision moves, what the credential was issued for does not.
    Rename,
    /// A new URL: the server is `added` again with no credential named.
    Move,
    /// The server is deleted.
    Delete,
    /// The broker marks the grant: a state write that moves the revision.
    Mark,
}

/// How the next `complete_oauth` ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Completes {
    #[default]
    Succeeds,
    /// Not a flow of this owner and server, or an `iss` that is refused.
    Unknown,
    /// The authorization server refused the code.
    NeedsReauth,
    /// The authorization server or the store could not be reached.
    Unavailable,
}

/// The definitions' store with the switches a test throws: a `replace` that fails, a `get` that fails.
#[derive(Clone, Default)]
struct Store {
    inner: MemoryUserToolServers,
    fail_replaces: Arc<Mutex<usize>>,
    fail_gets: Arc<AtomicBool>,
    replaces: Arc<Mutex<usize>>,
}

impl From<NoUserToolServers> for Store {
    fn from(_: NoUserToolServers) -> Self {
        let store = Store::default();
        store.inner.set_down(true);
        store
    }
}

impl UserToolServerStore for Store {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.inner.insert(owner, server).await
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        if self.fail_gets.load(Ordering::SeqCst) {
            return Err(UserToolServerStoreError::unavailable("scripted"));
        }
        self.inner.get(owner, id).await
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        self.inner.list(owner).await
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        *self.replaces.lock().unwrap() += 1;
        {
            let mut failing = self.fail_replaces.lock().unwrap();
            if *failing > 0 {
                *failing -= 1;
                return Err(UserToolServerStoreError::unavailable("scripted"));
            }
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
        self.inner.set_state(owner, id, state, expected).await
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        self.inner.delete(owner, id).await
    }
}

/// One `complete_oauth` the broker was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Completion {
    owner: UserId,
    server: UserToolServerId,
    state: String,
    code: String,
    iss: Option<String>,
}

#[derive(Default)]
struct Brain {
    counter: u64,
    /// The state of the flow each server has pending, and the redirect it began with.
    begun: HashMap<(UserId, UserToolServerId), (String, String)>,
    begins: Vec<(UserId, UserToolServerId, String)>,
    completions: Vec<Completion>,
    revokes: Vec<(UserId, UserToolServerId)>,
    begin_fails: Option<BeginFails>,
    completes: Completes,
    meanwhile: Meanwhile,
    version: u64,
    /// What `begin_oauth` says the authorization server is.
    issuer: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeginFails {
    Egress(EgressRefusal),
    Unreachable,
    NoSuchServer,
}

#[derive(Clone)]
struct Oauth {
    servers: Store,
    clock: FixedClock,
    brain: Arc<Mutex<Brain>>,
}

impl Oauth {
    fn new(servers: &Store, clock: &FixedClock) -> Self {
        Oauth {
            servers: servers.clone(),
            clock: clock.clone(),
            brain: Arc::default(),
        }
    }

    fn brain(&self) -> std::sync::MutexGuard<'_, Brain> {
        self.brain.lock().unwrap()
    }
}

impl From<NoBroker> for Oauth {
    fn from(_: NoBroker) -> Self {
        Oauth::new(&Store::default(), &FixedClock::new(start_time()))
    }
}

impl KeepsCredentials for Oauth {}

impl CredentialBroker for Oauth {
    async fn store(
        &self,
        _: &UserId,
        _: &UserToolServerId,
        _: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        Err(CredentialError::Unknown)
    }

    async fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        let mut brain = self.brain();
        brain
            .begins
            .push((owner.clone(), server.clone(), redirect.as_str().to_owned()));
        match brain.begin_fails {
            Some(BeginFails::Egress(refusal)) => {
                return Err(
                    CredentialError::unavailable("the egress guard refused a URL")
                        .with_source(refusal),
                );
            }
            Some(BeginFails::Unreachable) => {
                return Err(CredentialError::unavailable("the server did not answer")
                    .with_source(std::io::Error::other("SENTINEL-io-cause")));
            }
            Some(BeginFails::NoSuchServer) => return Err(CredentialError::Unknown),
            None => {}
        }
        brain.counter += 1;
        let state = format!("state-of-flow-{:08}-abcdef", brain.counter);
        brain.begun.insert(
            (owner.clone(), server.clone()),
            (state.clone(), redirect.to_string()),
        );
        let issuer = brain.issuer.clone().unwrap_or_else(|| ISSUER.to_owned());
        Ok(OAuthStart {
            authorize_url: format!("{issuer}/authorize?state={state}&code_challenge=abc"),
            state: OAuthState::new(&state).unwrap(),
            expires_at: self
                .clock
                .now()
                .checked_add(jiff::SignedDuration::from_secs(
                    i64::try_from(FLOW_SECONDS).unwrap(),
                ))
                .unwrap(),
            issuer,
        })
    }

    async fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
        iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        let (outcome, meanwhile) = {
            let mut brain = self.brain();
            brain.completions.push(Completion {
                owner: owner.clone(),
                server: server.clone(),
                state: state.as_str().to_owned(),
                code: code.expose().to_owned(),
                iss: iss.map(str::to_owned),
            });
            let is_the_flow = brain
                .begun
                .get(&(owner.clone(), server.clone()))
                .is_some_and(|(pending, _)| pending == state.as_str());
            let outcome = if is_the_flow {
                brain.completes
            } else {
                Completes::Unknown
            };
            (outcome, std::mem::take(&mut brain.meanwhile))
        };
        match outcome {
            Completes::Unknown => return Err(CredentialError::Unknown),
            Completes::NeedsReauth => return Err(CredentialError::NeedsReauth),
            Completes::Unavailable => {
                return Err(CredentialError::unavailable(
                    "the token endpoint did not answer",
                ));
            }
            Completes::Succeeds => {}
        }
        if let Some(now) = self.servers.inner.get(owner, server).await.unwrap() {
            match meanwhile {
                Meanwhile::Nothing => {}
                Meanwhile::Rename => {
                    let renamed = UserToolServer::from_stored(
                        server.clone(),
                        "Renamed meanwhile",
                        now.server().url(),
                        now.server().auth().clone(),
                        now.server().state(),
                        now.server().credential().cloned(),
                    )
                    .unwrap();
                    self.servers
                        .inner
                        .replace(owner, &renamed, now.revision())
                        .await
                        .unwrap();
                }
                Meanwhile::Move => {
                    let moved = UserToolServer::from_stored(
                        server.clone(),
                        now.server().name(),
                        MOVED_URL,
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
                Meanwhile::Delete => self.servers.inner.delete(owner, server).await.unwrap(),
                Meanwhile::Mark => {
                    self.servers
                        .inner
                        .set_state(
                            owner,
                            server,
                            UserToolServerState::NeedsReauth,
                            now.revision(),
                        )
                        .await
                        .unwrap();
                }
            }
        }
        let mut brain = self.brain();
        brain.version += 1;
        Ok(CredentialRef::new(&format!("cred-{server}-v{}", brain.version)).unwrap())
    }

    async fn lease(
        &self,
        _: &UserId,
        _: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        Err(CredentialError::Unknown)
    }

    async fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        self.brain().revokes.push((owner.clone(), server.clone()));
        Ok(())
    }
}

// ------------------------------------------------------------------------------------ the rig

type Bundle = UserTools<Store, Oauth, MemoryPendingAuthorizations>;
type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    FixedClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    RefuseAll,
    NoArtifacts,
    Bundle,
>;

fn who(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

fn alice() -> Principal {
    who("alice@example.com", &["user", "maker"])
}
fn bob() -> Principal {
    who("bob@example.com", &["user", "maker"])
}
fn carol() -> Principal {
    who("carol@example.com", &["user"])
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

struct Rig {
    app: App<Stack>,
    servers: Store,
    broker: Oauth,
    pending: MemoryPendingAuthorizations,
    clock: FixedClock,
}

fn app_over(bundle: Bundle, clock: &FixedClock, redirect: Option<&str>) -> App<Stack> {
    App::new(
        PortSet {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agents: ScriptedAgent::new(),
            clock: clock.clone(),
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
            user_tool_redirect: redirect.map(|r| RedirectUri::parse(r).unwrap()),
            ..AppConfig::default()
        },
    )
    .unwrap()
}

impl Rig {
    fn full() -> Rig {
        Self::with_redirect(Some(REDIRECT))
    }

    fn with_redirect(redirect: Option<&str>) -> Rig {
        let clock = FixedClock::new(start_time());
        let servers = Store::default();
        let broker = Oauth::new(&servers, &clock);
        let pending = MemoryPendingAuthorizations::new();
        let bundle = Bundle::full(
            EgressPolicy::new(),
            servers.clone(),
            broker.clone(),
            pending.clone(),
        );
        Rig {
            app: app_over(bundle, &clock, redirect),
            servers,
            broker,
            pending,
            clock,
        }
    }

    /// An `oauth2` server of `who`'s, added through the application: `added`, no credential.
    async fn add_oauth(&self, who: &Principal) -> UserToolServerId {
        let added = self
            .app
            .add_user_tool_server(
                who,
                AddUserToolServer {
                    name: "GitHub".to_owned(),
                    url: URL.to_owned(),
                    auth: UserAuth::oauth2(vec!["repo".to_owned()], None).unwrap(),
                    secret: None,
                },
            )
            .await
            .unwrap();
        added.id().clone()
    }

    async fn stored(&self, who: &Principal, id: &UserToolServerId) -> Option<StoredServer> {
        self.servers.inner.get(&who.user, id).await.unwrap()
    }

    /// The state of the flow the broker began last for the server.
    fn state_of(&self, who: &Principal, id: &UserToolServerId) -> String {
        self.broker
            .brain()
            .begun
            .get(&(who.user.clone(), id.clone()))
            .expect("a flow was begun")
            .0
            .clone()
    }

    /// The callback of the flow the server has pending, with a good code.
    fn callback(&self, who: &Principal, id: &UserToolServerId) -> OAuthCallback {
        OAuthCallback {
            state: Some(self.state_of(who, id)),
            code: Some(ToolSecret::new(CODE)),
            iss: Some(ISSUER.to_owned()),
            error: false,
        }
    }

    fn completions(&self) -> Vec<Completion> {
        self.broker.brain().completions.clone()
    }

    fn revokes(&self) -> usize {
        self.broker.brain().revokes.len()
    }
}

// ------------------------------------------------------------------------------------ authorize

#[tokio::test]
async fn an_authorization_begins_at_the_broker_with_the_configured_redirect_and_keeps_the_state() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    let before = rig.stored(&alice, &id).await.unwrap();

    let started = rig
        .app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();

    assert_eq!(
        rig.broker.brain().begins,
        vec![(alice.user.clone(), id.clone(), REDIRECT.to_owned())],
        "the broker is given the owner, the server and the callback of this deployment"
    );
    assert!(started.authorize_url().starts_with(ISSUER));
    assert_eq!(started.issuer(), ISSUER);
    assert_eq!(
        started.expires_at(),
        start_time()
            .checked_add(jiff::SignedDuration::from_secs(
                i64::try_from(FLOW_SECONDS).unwrap()
            ))
            .unwrap()
    );
    // the state is kept for the callback, and only its digest
    let state = OAuthState::new(&rig.state_of(&alice, &id)).unwrap();
    assert_eq!(rig.pending.len(), 1);
    // and nothing of the definition was written: the server is as it was
    let after = rig.stored(&alice, &id).await.unwrap();
    assert_eq!(after, before);
    assert_eq!(after.server().state(), UserToolServerState::Added);
    let _ = state;
}

#[tokio::test]
async fn what_an_authorization_prints_hides_its_url() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    let started = rig
        .app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let shown = format!("{started:?}");
    assert!(
        !shown.contains("state-of-flow") && !shown.contains("code_challenge"),
        "{shown}"
    );
    assert!(shown.contains(ISSUER), "the issuer is public: {shown}");
}

#[tokio::test]
async fn a_server_that_is_already_connected_can_be_authorized_again() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let first = rig.callback(&alice, &id);
    assert_eq!(
        rig.app.finish_user_tool_authorization(first).await,
        AuthorizationOutcome::Connected
    );
    let v1 = rig
        .stored(&alice, &id)
        .await
        .unwrap()
        .server()
        .credential()
        .cloned();

    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let second = rig.callback(&alice, &id);
    assert_eq!(
        rig.app.finish_user_tool_authorization(second).await,
        AuthorizationOutcome::Connected
    );
    let stored = rig.stored(&alice, &id).await.unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert_ne!(
        stored.server().credential().cloned(),
        v1,
        "the new grant is named"
    );
}

#[tokio::test]
async fn authorizing_is_refused_for_what_it_cannot_be_done_for_and_calls_nothing() {
    let rig = Rig::full();
    let (alice, bob, carol) = (alice(), bob(), carol());
    let id = rig.add_oauth(&alice).await;
    let bearer = rig
        .app
        .add_user_tool_server(
            &alice,
            AddUserToolServer {
                name: "Key".to_owned(),
                url: URL.to_owned(),
                auth: UserAuth::None,
                secret: None,
            },
        )
        .await
        .unwrap();

    let calls = |rig: &Rig| rig.broker.brain().begins.len();
    // a role without the permission
    let denied = rig
        .app
        .authorize_user_tool_server(&carol, id.as_str())
        .await
        .unwrap_err();
    assert!(matches!(denied, AppError::Forbidden { .. }), "{denied:?}");
    // another person's server is not there; neither is one that never was, or a malformed id
    for (who, id) in [
        (&bob, id.as_str()),
        (&alice, "u-0000000000"),
        (&alice, "not-an-id"),
        // her own, but of a kind with no authorization
        (&alice, bearer.id().as_str()),
    ] {
        let error = rig
            .app
            .authorize_user_tool_server(who, id)
            .await
            .unwrap_err();
        assert!(
            matches!(error, AppError::UserTool(UserToolRefusal::NotFound)),
            "{id}: {error:?}"
        );
    }
    assert_eq!(calls(&rig), 0, "the broker was not asked");
    assert!(rig.pending.is_empty());
}

#[tokio::test]
async fn authorizing_is_a_404_when_the_feature_is_off_and_a_422_when_nothing_can_be_kept() {
    // off
    let clock = FixedClock::new(start_time());
    let off = app_over(Bundle::off(), &clock, Some(REDIRECT));
    let error = off
        .authorize_user_tool_server(&alice(), "u-0123456789")
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::UserTool(UserToolRefusal::Off)),
        "{error:?}"
    );

    // enabled with no secrets section
    let without = app_over(
        Bundle::without_credentials(EgressPolicy::new(), Store::default()),
        &clock,
        Some(REDIRECT),
    );
    let error = without
        .authorize_user_tool_server(&alice(), "u-0123456789")
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::UserTool(UserToolRefusal::NoCredentials)),
        "{error:?}"
    );

    // keeps credentials, and has nowhere for the browser to come back to
    let rig = Rig::with_redirect(None);
    let id = rig.add_oauth(&alice()).await;
    let error = rig
        .app
        .authorize_user_tool_server(&alice(), id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            AppError::UserTool(UserToolRefusal::OAuthNotConfigured)
        ),
        "{error:?}"
    );
    assert!(rig.broker.brain().begins.is_empty());
}

#[tokio::test]
async fn a_refusal_of_the_egress_guard_is_told_as_its_one_public_sentence() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    for refusal in [
        EgressRefusal::PrivateAddress,
        EgressRefusal::NoAddress,
        EgressRefusal::Denied,
        EgressRefusal::NotAllowed,
    ] {
        rig.broker.brain().begin_fails = Some(BeginFails::Egress(refusal));
        let error = rig
            .app
            .authorize_user_tool_server(&alice, id.as_str())
            .await
            .unwrap_err();
        let AppError::UserTool(UserToolRefusal::EgressRefused { reason }) = &error else {
            panic!("{refusal:?}: {error:?}");
        };
        assert_eq!(reason, &refusal.public_reason());
        assert_eq!(error.to_string(), refusal.public_reason());
    }
    // the precise rule is never the answer: private and unresolvable are one sentence
    assert_eq!(
        EgressRefusal::PrivateAddress.public_reason(),
        EgressRefusal::NoAddress.public_reason()
    );
    assert!(
        rig.pending.is_empty(),
        "nothing was kept for a flow that did not begin"
    );
}

#[tokio::test]
async fn other_failures_of_the_broker_are_one_sentence_that_names_no_cause() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;

    rig.broker.brain().begin_fails = Some(BeginFails::Unreachable);
    let error = rig
        .app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            AppError::UserTool(UserToolRefusal::AuthorizationUnavailable)
        ),
        "{error:?}"
    );
    assert!(
        !report(&error).contains("SENTINEL-io-cause"),
        "the cause is not the answer"
    );

    // the server was deleted under the request
    rig.broker.brain().begin_fails = Some(BeginFails::NoSuchServer);
    let error = rig
        .app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::UserTool(UserToolRefusal::NotFound)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_state_that_cannot_be_kept_is_a_storage_failure_and_no_flow_is_offered() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.pending.set_down(true);
    let error = rig
        .app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Store(_)), "{error:?}");
    assert_eq!(
        orch_core::Classify::class(&error),
        orch_core::ErrorClass::Transient
    );
}

// ------------------------------------------------------------------------------------- the callback

#[tokio::test]
async fn the_callback_connects_the_server_with_what_the_browser_brought() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let state = rig.state_of(&alice, &id);

    let outcome = rig
        .app
        .finish_user_tool_authorization(rig.callback(&alice, &id))
        .await;

    assert_eq!(outcome, AuthorizationOutcome::Connected);
    assert_eq!(
        rig.completions(),
        vec![Completion {
            owner: alice.user.clone(),
            server: id.clone(),
            state,
            code: CODE.to_owned(),
            iss: Some(ISSUER.to_owned()),
        }],
        "the broker is given the person the state found, and the `iss` as it came"
    );
    let stored = rig.stored(&alice, &id).await.unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::Connected);
    assert!(
        stored.server().credential().is_some(),
        "the reference the broker gave is named"
    );
    assert_eq!(stored.server().name(), "GitHub");
    assert!(rig.pending.is_empty(), "the state was taken");
    assert_eq!(rig.revokes(), 0);
}

#[tokio::test]
async fn the_callback_passes_the_iss_as_it_came_and_none_when_there_was_none() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    for iss in [None, Some(String::new()), Some(ISS_SENTINEL.to_owned())] {
        rig.app
            .authorize_user_tool_server(&alice, id.as_str())
            .await
            .unwrap();
        let mut callback = rig.callback(&alice, &id);
        callback.iss = iss.clone();
        rig.app.finish_user_tool_authorization(callback).await;
        assert_eq!(rig.completions().last().unwrap().iss, iss);
    }
}

#[tokio::test]
async fn a_state_is_used_once() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let callback = rig.callback(&alice, &id);
    let again = rig.callback(&alice, &id);
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::Connected
    );
    assert_eq!(
        rig.app.finish_user_tool_authorization(again).await,
        AuthorizationOutcome::InvalidLink
    );
    assert_eq!(rig.completions().len(), 1, "the code was exchanged once");
}

#[tokio::test]
async fn a_state_that_is_not_pending_is_an_invalid_link_and_asks_the_broker_nothing() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    for state in [
        None,
        Some(String::new()),
        // not a state at all: too short, a character a state does not have
        Some("short".to_owned()),
        Some("has a space in it, so it is not a state".to_owned()),
        Some("x".repeat(500)),
        // well formed and never put
        Some("state-of-flow-99999999-abcdef".to_owned()),
    ] {
        let callback = OAuthCallback {
            state,
            code: Some(ToolSecret::new(CODE)),
            iss: None,
            error: false,
        };
        assert_eq!(
            rig.app.finish_user_tool_authorization(callback).await,
            AuthorizationOutcome::InvalidLink
        );
    }
    assert!(rig.completions().is_empty());
    // and the pending state was not harmed by the others
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::Connected
    );
}

#[tokio::test]
async fn an_ended_state_is_an_invalid_link() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let callback = rig.callback(&alice, &id);
    rig.clock.advance(Duration::from_secs(FLOW_SECONDS));
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::InvalidLink,
        "valid until the instant it ends"
    );
    assert!(rig.completions().is_empty());
    assert_eq!(
        rig.stored(&alice, &id).await.unwrap().server().state(),
        UserToolServerState::Added
    );
}

#[tokio::test]
async fn a_newer_authorization_of_the_server_ends_the_older_state() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let older = rig.callback(&alice, &id);
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    assert_eq!(rig.pending.len(), 1, "one pending authorization per server");
    assert_eq!(
        rig.app.finish_user_tool_authorization(older).await,
        AuthorizationOutcome::InvalidLink
    );
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::Connected
    );
}

#[tokio::test]
async fn a_redirect_with_an_error_spends_the_state_and_calls_nothing() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let mut declined = rig.callback(&alice, &id);
    declined.error = true;
    // even if it carries a code, an error wins
    assert_eq!(
        rig.app.finish_user_tool_authorization(declined).await,
        AuthorizationOutcome::NotCompleted
    );
    assert!(
        rig.completions().is_empty(),
        "no exchange for a redirect that said no"
    );
    assert_eq!(
        rig.stored(&alice, &id).await.unwrap().server().state(),
        UserToolServerState::Added,
        "the server stays as it was"
    );
    // the state is spent: the same redirect again is nothing
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::InvalidLink
    );
}

#[tokio::test]
async fn a_redirect_with_no_usable_code_spends_the_state_and_calls_nothing() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    let too_long = "c".repeat(MAX_CALLBACK_CODE_BYTES + 1);
    for code in [None, Some(String::new()), Some(too_long)] {
        rig.app
            .authorize_user_tool_server(&alice, id.as_str())
            .await
            .unwrap();
        let mut callback = rig.callback(&alice, &id);
        callback.code = code.map(ToolSecret::new);
        assert_eq!(
            rig.app.finish_user_tool_authorization(callback).await,
            AuthorizationOutcome::NotCompleted
        );
    }
    assert!(rig.completions().is_empty());
    // a code of exactly the limit is a code
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let mut callback = rig.callback(&alice, &id);
    callback.code = Some(ToolSecret::new("c".repeat(MAX_CALLBACK_CODE_BYTES)));
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::Connected
    );
}

#[tokio::test]
async fn what_the_broker_says_decides_the_page_and_the_state_is_spent_every_time() {
    for (completes, want) in [
        (Completes::Unknown, AuthorizationOutcome::InvalidLink),
        (Completes::NeedsReauth, AuthorizationOutcome::NotCompleted),
        (Completes::Unavailable, AuthorizationOutcome::Unavailable),
    ] {
        let rig = Rig::full();
        let alice = alice();
        let id = rig.add_oauth(&alice).await;
        rig.app
            .authorize_user_tool_server(&alice, id.as_str())
            .await
            .unwrap();
        rig.broker.brain().completes = completes;
        let callback = rig.callback(&alice, &id);
        let again = rig.callback(&alice, &id);
        assert_eq!(
            rig.app.finish_user_tool_authorization(callback).await,
            want,
            "{completes:?}"
        );
        let stored = rig.stored(&alice, &id).await.unwrap();
        assert_eq!(
            stored.server().state(),
            UserToolServerState::Added,
            "{completes:?}"
        );
        assert!(stored.server().credential().is_none(), "{completes:?}");
        assert_eq!(rig.revokes(), 0, "{completes:?}: the broker kept nothing");
        // spent: a second try of the same redirect never reaches the broker
        rig.broker.brain().completes = Completes::Succeeds;
        assert_eq!(
            rig.app.finish_user_tool_authorization(again).await,
            AuthorizationOutcome::InvalidLink,
            "{completes:?}"
        );
        assert_eq!(rig.completions().len(), 1, "{completes:?}");
    }
}

#[tokio::test]
async fn a_server_that_is_gone_or_changed_kind_before_the_callback_is_an_invalid_link() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let callback = rig.callback(&alice, &id);
    rig.servers.inner.delete(&alice.user, &id).await.unwrap();
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::InvalidLink
    );

    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let callback = rig.callback(&alice, &id);
    let now = rig.stored(&alice, &id).await.unwrap();
    let changed = UserToolServer::from_stored(
        id.clone(),
        "GitHub",
        URL,
        UserAuth::None,
        UserToolServerState::Connected,
        None,
    )
    .unwrap();
    rig.servers
        .inner
        .replace(&alice.user, &changed, now.revision())
        .await
        .unwrap();
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::InvalidLink
    );
    assert!(rig.completions().is_empty(), "the code was sent to nobody");
}

#[tokio::test]
async fn a_rename_or_a_mark_while_the_code_is_exchanged_does_not_lose_the_authorization() {
    for meanwhile in [Meanwhile::Rename, Meanwhile::Mark] {
        let rig = Rig::full();
        let alice = alice();
        let id = rig.add_oauth(&alice).await;
        rig.app
            .authorize_user_tool_server(&alice, id.as_str())
            .await
            .unwrap();
        rig.broker.brain().meanwhile = meanwhile;
        assert_eq!(
            rig.app
                .finish_user_tool_authorization(rig.callback(&alice, &id))
                .await,
            AuthorizationOutcome::Connected,
            "{meanwhile:?}"
        );
        let stored = rig.stored(&alice, &id).await.unwrap();
        assert_eq!(
            stored.server().state(),
            UserToolServerState::Connected,
            "{meanwhile:?}"
        );
        assert!(stored.server().credential().is_some(), "{meanwhile:?}");
        if meanwhile == Meanwhile::Rename {
            assert_eq!(
                stored.server().name(),
                "Renamed meanwhile",
                "the other writer's change stands"
            );
        }
        assert_eq!(rig.revokes(), 0, "{meanwhile:?}");
    }
}

#[tokio::test]
async fn a_server_moved_while_the_code_is_exchanged_gets_no_credential_and_the_grant_is_revoked() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    rig.broker.brain().meanwhile = Meanwhile::Move;

    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::NotCompleted
    );

    let stored = rig.stored(&alice, &id).await.unwrap();
    assert_eq!(stored.server().url(), MOVED_URL, "the edit stands");
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(
        stored.server().credential().is_none(),
        "nothing names the grant for the old URL"
    );
    assert_eq!(
        rig.broker.brain().revokes,
        vec![(alice.user.clone(), id.clone())],
        "the grant that fits no definition was revoked"
    );
}

#[tokio::test]
async fn a_server_deleted_while_the_code_is_exchanged_has_its_grant_revoked() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    rig.broker.brain().meanwhile = Meanwhile::Delete;
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::NotCompleted
    );
    assert!(rig.stored(&alice, &id).await.is_none());
    assert_eq!(rig.revokes(), 1);
}

#[tokio::test]
async fn a_definition_that_cannot_be_written_leaves_no_unnamed_grant() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    *rig.servers.fail_replaces.lock().unwrap() = 1;
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::Unavailable
    );
    let stored = rig.stored(&alice, &id).await.unwrap();
    assert_eq!(stored.server().state(), UserToolServerState::Added);
    assert!(stored.server().credential().is_none());
    assert_eq!(
        rig.revokes(),
        1,
        "the broker kept a grant that no definition names: forgotten"
    );
}

#[tokio::test]
async fn a_store_that_cannot_be_read_for_a_callback_is_unavailable() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    rig.servers.fail_gets.store(true, Ordering::SeqCst);
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&alice, &id))
            .await,
        AuthorizationOutcome::Unavailable
    );
    assert!(
        rig.completions().is_empty(),
        "the code was not exchanged for a server nobody could read"
    );
}

#[tokio::test]
async fn pending_authorizations_that_are_down_leave_the_state_for_the_next_try() {
    let rig = Rig::full();
    let alice = alice();
    let id = rig.add_oauth(&alice).await;
    rig.app
        .authorize_user_tool_server(&alice, id.as_str())
        .await
        .unwrap();
    let callback = rig.callback(&alice, &id);
    let again = rig.callback(&alice, &id);
    rig.pending.set_down(true);
    assert_eq!(
        rig.app.finish_user_tool_authorization(callback).await,
        AuthorizationOutcome::Unavailable
    );
    rig.pending.set_down(false);
    assert_eq!(
        rig.app.finish_user_tool_authorization(again).await,
        AuthorizationOutcome::Connected,
        "a state that was not read is still pending"
    );
}

#[tokio::test]
async fn a_callback_is_nothing_where_nothing_can_be_pending() {
    let clock = FixedClock::new(start_time());
    let callback = || OAuthCallback {
        state: Some("state-of-flow-00000001-abcdef".to_owned()),
        code: Some(ToolSecret::new(CODE)),
        iss: None,
        error: false,
    };
    let off = app_over(Bundle::off(), &clock, Some(REDIRECT));
    assert_eq!(
        off.finish_user_tool_authorization(callback()).await,
        AuthorizationOutcome::InvalidLink
    );
    let without = app_over(
        Bundle::without_credentials(EgressPolicy::new(), Store::default()),
        &clock,
        Some(REDIRECT),
    );
    assert_eq!(
        without.finish_user_tool_authorization(callback()).await,
        AuthorizationOutcome::InvalidLink
    );
}

#[tokio::test]
async fn two_people_with_the_same_server_id_are_kept_apart_by_the_state() {
    let rig = Rig::full();
    let (alice, bob) = (alice(), bob());
    let a = rig.add_oauth(&alice).await;
    let b = rig.add_oauth(&bob).await;
    rig.app
        .authorize_user_tool_server(&alice, a.as_str())
        .await
        .unwrap();
    rig.app
        .authorize_user_tool_server(&bob, b.as_str())
        .await
        .unwrap();
    // bob's redirect finishes bob's server, not alice's
    assert_eq!(
        rig.app
            .finish_user_tool_authorization(rig.callback(&bob, &b))
            .await,
        AuthorizationOutcome::Connected
    );
    assert_eq!(
        rig.stored(&alice, &a).await.unwrap().server().state(),
        UserToolServerState::Added
    );
    assert_eq!(rig.completions()[0].owner, bob.user);
}

#[test]
fn what_a_callback_prints_is_which_parts_it_had_and_no_text() {
    let callback = OAuthCallback {
        state: Some("SENTINEL-state-0123456789".to_owned()),
        code: Some(ToolSecret::new(CODE)),
        iss: Some(ISS_SENTINEL.to_owned()),
        error: true,
    };
    let shown = format!("{callback:?}");
    for hidden in ["SENTINEL", CODE] {
        assert!(!shown.contains(hidden), "{shown}");
    }
    assert!(shown.contains("error: true"), "{shown}");
}

#[test]
fn the_outcomes_have_fixed_words_and_only_one_is_connected() {
    use AuthorizationOutcome::{Connected, InvalidLink, NotCompleted, Unavailable};
    assert_eq!(
        [Connected, InvalidLink, NotCompleted, Unavailable].map(AuthorizationOutcome::as_str),
        ["connected", "invalid_link", "not_completed", "unavailable"]
    );
    assert!(Connected.is_connected());
    assert!(
        ![InvalidLink, NotCompleted, Unavailable]
            .iter()
            .any(|o| o.is_connected())
    );
}
