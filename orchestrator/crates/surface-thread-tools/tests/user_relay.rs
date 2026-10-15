//! The relay of a person's own MCP servers (ADR 0046, decisions 4 and 5, slice 5a), over the
//! in-memory stack, `MemoryToolServers` for the person's servers and a broker that counts the
//! leases it lends: what `tools/list` shows of them (names, no annotations, a description cut at
//! 2 KiB), what `tools/call` sends (exactly the leased header and value), that **a lease is taken for
//! every list and every call and none is kept**, what happens when the credential cannot be had (the
//! server is left out, a call fails with a sentence and a failed step, never a silent skip), what a
//! 401 does (the definition becomes `needs_reauth`, and the picker says so), and that a server
//! detached, deleted or switched off after the agent listed it is not on the endpoint.
//!
//! The attach routes of the HTTP API are exercised here too, over the same stack: the codes of the
//! 422 a client acts on.
//!
//! The wire (the real client, a real server, the egress guard at the connection) and the sentinel are
//! `user_relay_wire.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig, Inbound, NewThread,
    Permission, Policy, RoleGrant, ToolServerInfo,
};
use orch_core::{
    AgentId, AgentStepData, AgentTarget, EventBody, StepPhase, StepState, ThreadId, ThreadState,
    UserId,
};
use orch_ports::memory::{
    MemoryPendingAuthorizations, MemoryStore, MemoryToolServer, MemoryToolServers,
    MemoryUserToolServers, MemoryWakeup, ScriptedAgent, SeqIds, ToolScript,
};
use orch_ports::{
    AgentEndpoint, CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy,
    EgressRefusal, FixedRegistry, IdGen, KeepsCredentials, NoArtifacts, NoBroker, NoModel,
    NoUserToolServers, OAuthStart, OAuthState, PortSet, RedirectUri, Role, ServerRevision,
    StoredServer, ToolDef, ToolSecret, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserTools,
};
use orch_surface_thread_tools::{
    MAX_OWN_DESCRIPTION_BYTES, NEEDS_SIGN_IN, RelayTools, ThreadToolsConfig, USER_SERVER_TIMEOUT,
};
use rmcp::ServiceError;
use serde_json::{Value, json};
use support::{ALICE, Client, call, connect, grant_token, keys, tool_names, try_call};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const WEBSEARCH: &str = "http://websearch.test/mcp";
const WEBSEARCH_BEARER: &str = "deployment-bearer-0001";

// ------------------------------------------------------------------------------ the broker

/// What the broker does at the next `lease` of a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// A bearer token, a different one for every lease.
    Bearer,
    /// A value in this header, a different one for every lease.
    Header(&'static str),
    NeedsReauth,
    /// The broker or its secret store is down.
    Unavailable,
    /// A refresh was refused by the egress guard.
    Egress,
    /// No such server (it was deleted under the lease).
    Unknown,
    /// A lease that ended a second ago.
    Expired,
}

#[derive(Default)]
struct Spy {
    leases: Vec<(UserId, UserToolServerId)>,
    behaviour: BTreeMap<UserToolServerId, Behaviour>,
    /// While the next lease is taken, another writer moves the definition under the relay: its
    /// revision changes and nothing else does (a person re-keyed the server).
    another_writer: Option<(UserId, UserToolServerId)>,
}

/// A broker that lends what a test says and counts. Every other call is refused.
#[derive(Clone)]
struct SpyBroker {
    spy: Arc<Mutex<Spy>>,
    servers: MemoryUserToolServers,
}

impl SpyBroker {
    fn over(servers: &MemoryUserToolServers) -> Self {
        SpyBroker {
            spy: Arc::default(),
            servers: servers.clone(),
        }
    }

    fn move_revision_during_the_next_lease(&self, owner: &UserId, id: &UserToolServerId) {
        self.spy.lock().unwrap().another_writer = Some((owner.clone(), id.clone()));
    }

    fn leases(&self) -> usize {
        self.spy.lock().unwrap().leases.len()
    }

    fn lent_to(&self) -> Vec<(UserId, UserToolServerId)> {
        self.spy.lock().unwrap().leases.clone()
    }

    fn behave(&self, id: &UserToolServerId, behaviour: Behaviour) {
        self.spy
            .lock()
            .unwrap()
            .behaviour
            .insert(id.clone(), behaviour);
    }
}

impl From<NoBroker> for SpyBroker {
    fn from(_: NoBroker) -> Self {
        SpyBroker::over(&MemoryUserToolServers::new())
    }
}

impl KeepsCredentials for SpyBroker {}

fn refused() -> CredentialError {
    CredentialError::unavailable("this broker only lends")
}

impl CredentialBroker for SpyBroker {
    async fn store(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
        _material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        Err(refused())
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
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        let (n, behaviour, another_writer) = {
            let mut spy = self.spy.lock().unwrap();
            spy.leases.push((owner.clone(), server.clone()));
            let n = spy.leases.len();
            let behaviour = spy
                .behaviour
                .get(server)
                .copied()
                .unwrap_or(Behaviour::Bearer);
            (n, behaviour, spy.another_writer.take())
        };
        if let Some((owner, id)) = another_writer {
            let now = self.servers.get(&owner, &id).await.unwrap().unwrap();
            self.servers
                .set_state(&owner, &id, now.server().state(), now.revision())
                .await
                .unwrap();
        }
        let in_five_minutes = Timestamp::now() + Duration::from_secs(300);
        match behaviour {
            Behaviour::Bearer => Ok(CredentialLease::bearer(
                ToolSecret::new(format!("lent-token-{n}")),
                in_five_minutes,
            )),
            Behaviour::Header(name) => Ok(CredentialLease::header(
                name,
                ToolSecret::new(format!("lent-key-{n}")),
                in_five_minutes,
            )
            .unwrap()),
            Behaviour::NeedsReauth => Err(CredentialError::NeedsReauth),
            Behaviour::Unavailable => Err(CredentialError::unavailable("the secret store is down")),
            Behaviour::Egress => Err(CredentialError::unavailable("a URL was refused")
                .with_source(EgressRefusal::PrivateAddress)),
            Behaviour::Unknown => Err(CredentialError::Unknown),
            Behaviour::Expired => Ok(CredentialLease::bearer(
                ToolSecret::new(format!("lent-token-{n}")),
                Timestamp::now() - Duration::from_secs(1),
            )),
        }
    }

    async fn revoke(
        &self,
        _owner: &UserId,
        _server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        Err(refused())
    }
}

// --------------------------------------------------------------------------------- the rig

/// The definitions' store of the rig: the in-memory one, or a store that refuses everything, which
/// is what a deployment with the feature off holds (`NoUserToolServers`).
#[derive(Clone)]
struct Servers {
    inner: MemoryUserToolServers,
    refuse: bool,
}

impl From<NoUserToolServers> for Servers {
    fn from(_: NoUserToolServers) -> Self {
        Servers {
            inner: MemoryUserToolServers::new(),
            refuse: true,
        }
    }
}

impl Servers {
    fn up(&self) -> Result<(), UserToolServerStoreError> {
        if self.refuse {
            return Err(UserToolServerStoreError::unavailable(
                "people's own MCP servers are off in this deployment",
            ));
        }
        Ok(())
    }
}

impl UserToolServerStore for Servers {
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
        self.inner.get(owner, id).await
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        self.up()?;
        self.inner.list(owner).await
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
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

/// What the application is built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Credentials are kept (`Full`).
    Full,
    /// Definitions are kept and credentials are not (`WithoutCredentials`).
    WithoutCredentials,
    /// People's own servers are off (`Off`): the definitions' store refuses even a read.
    Off,
}

type Bundle = UserTools<Servers, SpyBroker, MemoryPendingAuthorizations>;
type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    orch_ports::SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    orch_auth_header::HeaderAuth,
    NoArtifacts,
    Bundle,
>;

/// Everybody who signs in may use servers of their own here.
fn policy() -> Policy {
    let mut grant = RoleGrant::user();
    grant.permissions.insert(Permission::ToolsUserAdd);
    Policy::new(
        BTreeMap::from([(Role::new("user"), grant)]),
        Some(Role::new("user")),
    )
    .unwrap()
}

struct Rig {
    base: String,
    http: reqwest::Client,
    app: Arc<App<Stack>>,
    ids: SeqIds,
    servers: MemoryUserToolServers,
    broker: SpyBroker,
    memory: MemoryToolServers,
    store: MemoryStore,
    server: JoinHandle<()>,
    stop: CancellationToken,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop.cancel();
        self.server.abort();
    }
}

fn dispatcher_config() -> DispatcherConfig {
    DispatcherConfig {
        concurrency: 8,
        lease: Duration::from_millis(500),
        heartbeat: Duration::from_millis(100),
        poll_interval: Duration::from_millis(50),
        max_attempts: 5,
        backoff_base: Duration::from_millis(20),
        backoff_max: Duration::from_millis(200),
        poll_min: Duration::from_millis(20),
        poll_max: Duration::from_millis(100),
        max_poll_failures: 10,
        max_cancel_attempts: 10,
        cancel_retry_delay: Duration::from_millis(50),
        verify_watch: Duration::from_millis(50),
        live_flush: Duration::from_millis(20),
        live_refresh: Duration::from_millis(150),
        live_max_bytes: 64 * 1024,
    }
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

fn def(name: &str, description: Option<String>, annotations: Option<Value>) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        title: None,
        description,
        input_schema: json!({"type": "object"}).as_object().unwrap().clone(),
        output_schema: None,
        annotations,
    }
}

/// The deployment's server, with a tool that carries annotations: the control for what is dropped
/// from a person's.
fn websearch_upstream() -> MemoryToolServer {
    MemoryToolServer::new()
        .with_bearer(WEBSEARCH_BEARER)
        .with_tool(
            def(
                "lookup",
                Some("Looks a thing up.".to_owned()),
                Some(json!({"readOnlyHint": true})),
            ),
            ToolScript::Echo,
        )
}

impl Rig {
    async fn start() -> Rig {
        Self::start_with(EgressPolicy::new(), MemoryStore::new(), Mode::Full, true).await
    }

    /// An application over `store` built in `mode`, and a relay that has the client for people's
    /// servers or not.
    async fn start_with(
        egress: EgressPolicy,
        store: MemoryStore,
        mode: Mode,
        own_client: bool,
    ) -> Rig {
        let servers = MemoryUserToolServers::new();
        let broker = SpyBroker::over(&servers);
        let wrapped = Servers {
            inner: servers.clone(),
            refuse: false,
        };
        let bundle = match mode {
            Mode::Full => Bundle::full(
                egress,
                wrapped,
                broker.clone(),
                MemoryPendingAuthorizations::new(),
            ),
            Mode::WithoutCredentials => Bundle::without_credentials(egress, wrapped),
            Mode::Off => Bundle::off(),
        };
        Self::start_over(store, bundle, servers, broker, own_client).await
    }

    async fn start_over(
        store: MemoryStore,
        bundle: Bundle,
        servers: MemoryUserToolServers,
        broker: SpyBroker,
        own_client: bool,
    ) -> Rig {
        let ids = SeqIds::default();
        let websearch = ToolServerInfo {
            timeout: Duration::from_secs(2),
            ..ToolServerInfo::new("websearch", "Web search")
        };
        let app = Arc::new(
            App::new(
                PortSet {
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: orch_ports::SystemClock,
                    ids: ids.clone(),
                    model: NoModel,
                    auth: orch_auth_header::HeaderAuth::new(),
                    registry: directory().fixed_registry(),
                    artifacts: NoArtifacts,
                    user_tools: bundle,
                },
                directory(),
                AppConfig {
                    policy: policy(),
                    tool_servers: vec![websearch.clone()],
                    stream_poll: Duration::from_millis(100),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let memory = MemoryToolServers::new();
        memory.add(WEBSEARCH, websearch_upstream());
        let endpoint =
            orch_ports::ToolServerEndpoint::new("websearch", WEBSEARCH, Duration::from_secs(2))
                .with_bearer(ToolSecret::new(WEBSEARCH_BEARER));
        let mut relay =
            RelayTools::new(Arc::clone(&app), memory.clone(), [(websearch, endpoint)]).unwrap();
        if own_client {
            relay = relay.with_user_servers(memory.clone());
        }
        let config = ThreadToolsConfig::new(keys(), ["127.0.0.1", "localhost"])
            .unwrap()
            .with_provider(relay);
        let stop = CancellationToken::new();
        tokio::spawn(
            Dispatcher::new(Arc::clone(&app), dispatcher_config(), "user-relay").run(stop.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            ApiConfig::default(),
            vec![orch_surface_thread_tools::routes(Arc::clone(&app), config)],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Rig {
            base: format!("http://{addr}"),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            ids,
            servers,
            broker,
            memory,
            store,
            server,
            stop,
        }
    }

    fn alice() -> UserId {
        UserId::new(ALICE)
    }

    /// The definitions `owner` has in `other`, and their servers upstream, here too: the same
    /// person's servers seen by another application over the same threads.
    async fn copy_definitions_of(&self, owner: &UserId, other: &Rig) {
        for stored in other.servers.list(owner).await.unwrap() {
            self.servers.insert(owner, stored.server()).await.unwrap();
            self.memory.add(
                stored.server().url().to_owned(),
                MemoryToolServer::new().with_standard_tools(),
            );
        }
    }

    /// A server of Alice's at `url`, in `state`, upstream with the standard tools.
    async fn server(
        &self,
        name: &str,
        url: &str,
        auth: UserAuth,
        state: UserToolServerState,
    ) -> UserToolServerId {
        self.server_for(&Self::alice(), name, url, auth, state)
            .await
    }

    async fn server_for(
        &self,
        owner: &UserId,
        name: &str,
        url: &str,
        auth: UserAuth,
        state: UserToolServerState,
    ) -> UserToolServerId {
        let id = UserToolServerId::generate();
        let target = EgressPolicy::new().allowing_http().check_url(url).unwrap();
        let mut server = UserToolServer::new(id.clone(), name, &target, auth)
            .unwrap()
            .with_state(state);
        if state == UserToolServerState::Connected && server.auth().needs_credential() {
            server = server.with_credential(CredentialRef::new("cred-1").unwrap());
        }
        self.servers.insert(owner, &server).await.unwrap();
        self.memory.add(
            server.url().to_owned(),
            MemoryToolServer::new().with_standard_tools(),
        );
        id
    }

    /// A `bearer` server of Alice's, connected.
    async fn bearer(&self, name: &str, url: &str) -> UserToolServerId {
        self.server(name, url, UserAuth::Bearer, UserToolServerState::Connected)
            .await
    }

    async fn working_thread(&self, tools: &[&UserToolServerId]) -> ThreadId {
        let id = ThreadId(self.ids.new_id());
        self.app
            .create_thread_as(
                &Self::alice(),
                id,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "slow work".to_owned(),
                },
                Inbound {
                    tools: tools.iter().map(|t| t.to_string()).collect(),
                    ..Inbound::default()
                },
            )
            .await
            .unwrap();
        orch_testsupport::eventually("the agent is working", || async {
            let thread = self.app.get_thread(&Self::alice(), id).await.unwrap();
            (thread.state == ThreadState::Working).then_some(())
        })
        .await;
        id
    }

    async fn client(&self, thread: ThreadId) -> Client {
        connect(
            &format!("{}/thread-tools/{thread}/mcp", self.base),
            &grant_token(thread, "plain"),
        )
        .await
    }

    async fn events(&self, thread: ThreadId) -> Vec<orch_core::Event> {
        self.app
            .list_events(&Self::alice(), thread, 0, 500)
            .await
            .unwrap()
    }

    async fn steps(&self, thread: ThreadId) -> Vec<AgentStepData> {
        self.events(thread)
            .await
            .into_iter()
            .filter_map(|e| match e.body {
                EventBody::AgentStep(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    async fn state_of(&self, id: &UserToolServerId) -> UserToolServerState {
        self.servers
            .get(&Self::alice(), id)
            .await
            .unwrap()
            .unwrap()
            .server()
            .state()
    }

    /// What the person's server was asked, by url: `(method, bearer, headers)`.
    fn seen(&self, url: &str) -> Vec<orch_ports::testkit::tool_server::SeenRequest> {
        self.memory.seen(url)
    }

    async fn http_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("X-Auth-Request-Email", ALICE);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn protocol_code(error: &ServiceError) -> i32 {
    match error {
        ServiceError::McpError(data) => data.code.0,
        other => panic!("not a JSON-RPC error: {other}"),
    }
}

const TRACKER: &str = "https://tracker.example.test/mcp";
const NOTES: &str = "https://notes.example.test/mcp";

// ---- tools/list -------------------------------------------------------------------------------

#[tokio::test]
async fn a_persons_tools_are_named_id_double_underscore_tool_with_no_annotations_and_a_short_description()
 {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    // the stranger's server says a lot: long words, hints that a model might act on, an icon-less title
    r.memory.add(
        TRACKER,
        MemoryToolServer::new()
            .with_tool(
                def(
                    "search",
                    Some("é".repeat(5000)),
                    Some(json!({"readOnlyHint": true, "destructiveHint": false})),
                ),
                ToolScript::Echo,
            )
            .with_tool(def("_hidden", None, None), ToolScript::Echo)
            .with_tool(
                def("brief", Some("short".to_owned()), None),
                ToolScript::Echo,
            ),
    );
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        names,
        [
            "get_ui_catalog".to_owned(),
            "turn_output".to_owned(),
            format!("{id}__search"),
            format!("{id}__brief"),
        ],
        "`_hidden` (a leading `_`) is left out"
    );
    let search = tools
        .iter()
        .find(|t| t.name == format!("{id}__search"))
        .unwrap();
    assert!(search.annotations.is_none(), "{:?}", search.annotations);
    let description = search.description.as_deref().unwrap();
    assert!(
        description.len() <= MAX_OWN_DESCRIPTION_BYTES,
        "{}",
        description.len()
    );
    assert_eq!(MAX_OWN_DESCRIPTION_BYTES, 2048);
    assert!(
        description.len() >= MAX_OWN_DESCRIPTION_BYTES - 1,
        "cut at the limit, on a character boundary: {}",
        description.len()
    );
    assert!(description.chars().all(|c| c == 'é'));
    assert_eq!(search.title.as_deref(), Some("Tracker: search"));
    assert!(search.icons.is_none(), "a person supplies no icon");
    assert_eq!(
        serde_json::to_value(search.meta.as_ref().unwrap()).unwrap(),
        json!({"thread-tools/v1": {"reportsStep": true, "timeoutSecs": USER_SERVER_TIMEOUT.as_secs() + 5}}),
    );
    let brief = tools
        .iter()
        .find(|t| t.name == format!("{id}__brief"))
        .unwrap();
    assert_eq!(brief.description.as_deref(), Some("short"));
}

#[tokio::test]
async fn the_deployments_servers_keep_their_annotations_and_come_first() {
    // the control: what is dropped from a person's server is kept from the deployment's
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    r.app
        .set_tools(
            &Rig::alice(),
            thread,
            vec![id.to_string(), "websearch".to_owned()],
        )
        .await
        .unwrap();
    let tools = r.client(thread).await.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names[2], "websearch__lookup", "{names:?}");
    assert!(names[3].starts_with(&format!("{id}__")), "{names:?}");
    let lookup = tools
        .iter()
        .find(|t| t.name == "websearch__lookup")
        .unwrap();
    assert!(
        lookup.annotations.is_some(),
        "the deployment's server is believed"
    );
}

#[tokio::test]
async fn every_list_takes_a_fresh_lease_and_the_server_sees_each_one() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert_eq!(r.broker.leases(), 0, "attaching asked nobody");

    client.list_all_tools().await.unwrap();
    client.list_all_tools().await.unwrap();
    assert_eq!(r.broker.leases(), 2, "a lease for each list");
    let seen = r.seen(TRACKER);
    let bearers: Vec<_> = seen.iter().map(|s| s.bearer.clone().unwrap()).collect();
    assert_eq!(
        bearers,
        ["lent-token-1", "lent-token-2"],
        "a different token each time"
    );
    assert!(seen.iter().all(|s| s.method == "tools/list"));
    assert!(
        r.broker
            .lent_to()
            .iter()
            .all(|(owner, server)| owner == &Rig::alice() && server == &id)
    );
}

// ---- tools/call -------------------------------------------------------------------------------

#[tokio::test]
async fn a_call_sends_exactly_the_leased_bearer_and_is_one_step_with_the_generic_icon() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;

    let out = call(&client, &format!("{id}__echo"), json!({"text": "rust"})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value, json!({"text": "rust"}));

    let seen = r.seen(TRACKER);
    let called = seen.iter().find(|s| s.method == "tools/call").unwrap();
    assert_eq!(called.bearer.as_deref(), Some("lent-token-1"));
    assert!(called.headers.iter().all(|(name, _)| name != "x-api-key"));
    assert_eq!(
        called.name.as_deref(),
        Some("echo"),
        "the server's own name for the tool"
    );
    assert_eq!(r.broker.leases(), 1);

    // one step: its start and its end, the generic icon of a person's server, input and output
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    let (start, end) = (&steps[0], &steps[1]);
    assert_eq!(start.id, end.id);
    assert_eq!(
        (start.phase, start.state, end.phase, end.state),
        (
            StepPhase::Start,
            StepState::Running,
            StepPhase::End,
            StepState::Completed
        )
    );
    assert_eq!(
        start.icon.as_deref(),
        Some(format!("mcp-server:{id}").as_str())
    );
    assert_eq!(start.label, "Tracker \u{b7} echo");
    assert_eq!(start.input.as_ref().unwrap()["text"], "rust");
    assert_eq!(end.output.as_ref().unwrap().text, "{\"text\":\"rust\"}");
}

#[tokio::test]
async fn an_api_key_goes_in_the_header_the_person_named_and_nowhere_else() {
    let r = Rig::start().await;
    let id = r
        .server(
            "Search",
            NOTES,
            UserAuth::api_key("X-Api-Key").unwrap(),
            UserToolServerState::Connected,
        )
        .await;
    r.broker.behave(&id, Behaviour::Header("X-Api-Key"));
    let thread = r.working_thread(&[&id]).await;
    let out = call(&r.client(thread).await, &format!("{id}__echo"), json!({})).await;
    assert!(!out.is_error, "{out:?}");
    let seen = r.seen(NOTES);
    let called = seen.iter().find(|s| s.method == "tools/call").unwrap();
    assert_eq!(
        called.bearer, None,
        "no bearer: the key is not an Authorization"
    );
    assert_eq!(
        called.headers,
        [("x-api-key".to_owned(), "lent-key-1".to_owned())],
        "exactly the header and the value that were lent"
    );
}

#[tokio::test]
async fn a_lease_is_taken_for_every_call_and_none_is_kept() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    for n in 0..3 {
        let out = call(&client, &format!("{id}__echo"), json!({"n": n})).await;
        assert!(!out.is_error);
    }
    assert_eq!(r.broker.leases(), 3);
    let bearers: Vec<_> = r
        .seen(TRACKER)
        .iter()
        .filter(|s| s.method == "tools/call")
        .map(|s| s.bearer.clone().unwrap())
        .collect();
    assert_eq!(bearers, ["lent-token-1", "lent-token-2", "lent-token-3"]);
    // a list in between takes its own
    client.list_all_tools().await.unwrap();
    assert_eq!(r.broker.leases(), 4);
}

#[tokio::test]
async fn a_server_that_wants_no_credential_is_connected_to_with_none_and_the_broker_is_not_asked() {
    let r = Rig::start().await;
    let id = r
        .server(
            "Open",
            NOTES,
            UserAuth::None,
            UserToolServerState::Connected,
        )
        .await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    client.list_all_tools().await.unwrap();
    let out = call(&client, &format!("{id}__echo"), json!({"a": 1})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(r.broker.leases(), 0);
    assert!(
        r.seen(NOTES)
            .iter()
            .all(|s| s.bearer.is_none() && s.headers.is_empty())
    );
}

// ---- a credential that cannot be had ----------------------------------------------------------

#[tokio::test]
async fn a_credential_that_needs_the_person_leaves_the_server_out_and_fails_a_call_with_the_sentence()
 {
    let r = Rig::start().await;
    let gone = r.bearer("Tracker", TRACKER).await;
    let fine = r.bearer("Notes", NOTES).await;
    r.broker.behave(&gone, Behaviour::NeedsReauth);
    let thread = r.working_thread(&[&gone, &fine]).await;
    let client = r.client(thread).await;

    // left out of the list, the other server is listed
    let names = tool_names(&client).await;
    assert!(
        names.iter().any(|n| n.starts_with(&format!("{fine}__"))),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.starts_with(&format!("{gone}__"))),
        "{names:?}"
    );

    // a call that was attached fails as a failed step, with the sentence, never a silent skip
    let out = call(&client, &format!("{gone}__echo"), json!({"text": "x"})).await;
    assert!(out.is_error);
    assert_eq!(out.text, NEEDS_SIGN_IN);
    assert_eq!(
        NEEDS_SIGN_IN,
        "the MCP server needs the person to sign in again"
    );
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].state, StepState::Running);
    assert_eq!(steps[1].state, StepState::Failed);
    assert_eq!(steps[1].detail.as_deref(), Some(NEEDS_SIGN_IN));
    assert_eq!(
        steps[1].icon.as_deref(),
        Some(format!("mcp-server:{gone}").as_str())
    );
    assert!(
        r.seen(TRACKER).is_empty(),
        "the server was never connected to"
    );
}

#[tokio::test]
async fn a_credential_the_broker_says_ended_marks_the_server_and_the_picker_says_so() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.broker.behave(&id, Behaviour::NeedsReauth);
    let thread = r.working_thread(&[&id]).await;
    assert_eq!(r.state_of(&id).await, UserToolServerState::Connected);
    let (status, picker) = r
        .http_json(reqwest::Method::GET, "/api/tool-servers", None)
        .await;
    assert_eq!(status, 200);
    let own = picker
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap();
    assert_eq!(own["status"], "ready", "until something learns it ended");

    r.client(thread).await.list_all_tools().await.unwrap();
    assert_eq!(r.state_of(&id).await, UserToolServerState::NeedsReauth);
    let (_, picker) = r
        .http_json(reqwest::Method::GET, "/api/tool-servers", None)
        .await;
    let own = picker
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap();
    assert_eq!(
        (own["status"].as_str(), own["reason"].as_str()),
        (Some("needs_auth"), Some("reauthorize"))
    );
    let (_, mine) = r
        .http_json(reqwest::Method::GET, "/api/user-tool-servers", None)
        .await;
    let row = &mine["servers"][0];
    assert_eq!(
        (
            row["state"].as_str(),
            row["status"].as_str(),
            row["reason"].as_str()
        ),
        (
            Some("needs_reauth"),
            Some("needs_auth"),
            Some("reauthorize")
        )
    );
}

#[tokio::test]
async fn a_broker_that_is_down_leaves_the_server_out_and_fails_a_call_without_blaming_the_person() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.broker.behave(&id, Behaviour::Unavailable);
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the credential for the MCP server 'Tracker' could not be had now; the call did not run"
    );
    assert_ne!(
        out.text, NEEDS_SIGN_IN,
        "an outage is not a call to sign in again"
    );
    let steps = r.steps(thread).await;
    assert_eq!(steps.last().unwrap().state, StepState::Failed);
    assert_eq!(
        r.state_of(&id).await,
        UserToolServerState::Connected,
        "an outage marks nothing"
    );
    assert!(r.seen(TRACKER).is_empty());
}

#[tokio::test]
async fn a_refusal_of_the_egress_guard_is_not_reachable_from_here_in_the_same_words_for_every_rule()
{
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.broker.behave(&id, Behaviour::Egress);
    let thread = r.working_thread(&[&id]).await;
    let out = call(&r.client(thread).await, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the MCP server 'Tracker' is not reachable from here; the call did not run"
    );
    assert!(!out.text.contains("private") && !out.text.contains("address"));
    assert!(r.seen(TRACKER).is_empty());
}

#[tokio::test]
async fn a_server_the_egress_lists_refuse_today_is_not_connected_to_and_no_lease_is_taken() {
    // the definition was admitted when it was added; the operator's lists say otherwise now
    let open = Rig::start().await;
    let id = open.bearer("Tracker", TRACKER).await;
    let thread = open.working_thread(&[&id]).await;
    let r = Rig::start_with(
        EgressPolicy::new().with_deny("tracker.example.test".parse().unwrap()),
        open.store.clone(),
        Mode::Full,
        true,
    )
    .await;
    r.copy_definitions_of(&Rig::alice(), &open).await;
    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the MCP server 'Tracker' is not reachable from here; the call did not run"
    );
    assert_eq!(
        r.broker.leases(),
        0,
        "a token is not refreshed for a server that is not connected to"
    );
    assert!(r.seen(TRACKER).is_empty());
}

#[tokio::test]
async fn a_server_the_broker_does_not_know_is_left_out_and_a_call_says_it_is_no_longer_there() {
    // deleted between the read of the definition and the lease
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.broker.behave(&id, Behaviour::Unknown);
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the MCP server 'Tracker' is no longer there; the call did not run"
    );
    assert_eq!(
        r.steps(thread).await.last().unwrap().state,
        StepState::Failed
    );
    assert_eq!(
        r.state_of(&id).await,
        UserToolServerState::Connected,
        "nothing is marked"
    );
    assert!(r.seen(TRACKER).is_empty());
}

#[tokio::test]
async fn a_lease_that_has_ended_is_not_used() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.broker.behave(&id, Behaviour::Expired);
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert!(out.text.contains("could not be had now"), "{}", out.text);
    assert!(
        r.seen(TRACKER).is_empty(),
        "the ended credential was never sent"
    );
}

#[tokio::test]
async fn a_server_that_is_not_connected_is_not_offered_and_the_broker_is_not_asked() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    // the person changed the URL meanwhile: the server is `added` again, with no credential
    let stored = r.servers.get(&Rig::alice(), &id).await.unwrap().unwrap();
    r.servers
        .set_state(
            &Rig::alice(),
            &id,
            UserToolServerState::Added,
            stored.revision(),
        )
        .await
        .unwrap();
    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert_eq!(out.text, NEEDS_SIGN_IN);
    assert_eq!(r.broker.leases(), 0);
    assert!(r.seen(TRACKER).is_empty());
}

// ---- a 401 from the server --------------------------------------------------------------------

#[tokio::test]
async fn a_401_from_the_server_marks_the_definition_and_fails_the_call_and_the_next_asks_nobody() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    // the server wants another token than the one the broker lends: it answers 401
    r.memory.add(
        TRACKER,
        MemoryToolServer::new()
            .with_standard_tools()
            .with_bearer("another-token"),
    );
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;

    let out = call(&client, &format!("{id}__echo"), json!({"text": "x"})).await;
    assert!(out.is_error);
    assert_eq!(out.text, NEEDS_SIGN_IN);
    let steps = r.steps(thread).await;
    assert_eq!(steps.last().unwrap().state, StepState::Failed);
    assert_eq!(steps.last().unwrap().detail.as_deref(), Some(NEEDS_SIGN_IN));
    assert_eq!(r.state_of(&id).await, UserToolServerState::NeedsReauth);
    assert_eq!(r.broker.leases(), 1);

    // the picker says what to do
    let (_, picker) = r
        .http_json(reqwest::Method::GET, "/api/tool-servers", None)
        .await;
    let own = picker
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap();
    assert_eq!(
        (own["status"].as_str(), own["reason"].as_str()),
        (Some("needs_auth"), Some("reauthorize"))
    );

    // from now on the server is not offered, a call fails at once, and no token is lent
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let again = call(&client, &format!("{id}__echo"), json!({})).await;
    assert_eq!(again.text, NEEDS_SIGN_IN);
    assert_eq!(
        r.broker.leases(),
        1,
        "the credential is not lent again until the person acts"
    );

    // and the person cannot attach it to another thread
    let other = r.working_thread(&[]).await;
    let (status, body) = r
        .http_json(
            reqwest::Method::PUT,
            &format!("/api/threads/{other}/tools"),
            Some(json!({"servers": [id.to_string()]})),
        )
        .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "tool_server_needs_auth");
}

#[tokio::test]
async fn a_401_at_the_listing_marks_the_definition_too() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.memory.add(
        TRACKER,
        MemoryToolServer::new()
            .with_standard_tools()
            .with_bearer("another-token"),
    );
    let thread = r.working_thread(&[&id]).await;
    let names = tool_names(&r.client(thread).await).await;
    assert_eq!(names, ["get_ui_catalog", "turn_output"]);
    assert_eq!(r.state_of(&id).await, UserToolServerState::NeedsReauth);
}

#[tokio::test]
async fn a_401_does_not_overwrite_a_definition_that_changed_since_it_was_read() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    r.memory.add(
        TRACKER,
        MemoryToolServer::new()
            .with_standard_tools()
            .with_bearer("another-token"),
    );
    let thread = r.working_thread(&[&id]).await;
    // while the relay leases, the person re-keys the server: its revision moves
    r.broker
        .move_revision_during_the_next_lease(&Rig::alice(), &id);
    let out = call(&r.client(thread).await, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    assert_eq!(out.text, NEEDS_SIGN_IN, "the agent is told all the same");
    // the compare-and-swap on the revision read lost: the state is the person's to write
    assert_eq!(r.state_of(&id).await, UserToolServerState::Connected);
}

// ---- what is still on the endpoint at the call ---------------------------------------------------

#[tokio::test]
async fn a_server_detached_after_the_list_is_not_on_the_endpoint_at_the_call() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert!(tool_names(&client).await.contains(&format!("{id}__echo")));
    let leases = r.broker.leases();

    r.app
        .set_tools(&Rig::alice(), thread, Vec::new())
        .await
        .unwrap();
    let err = try_call(&client, &format!("{id}__echo"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert_eq!(
        r.broker.leases(),
        leases,
        "no lease for a call that is refused"
    );
    assert!(r.steps(thread).await.is_empty());
    assert!(r.seen(TRACKER).iter().all(|s| s.method == "tools/list"));
}

#[tokio::test]
async fn a_server_deleted_after_the_list_is_not_on_the_endpoint_at_the_call() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    assert!(tool_names(&client).await.contains(&format!("{id}__echo")));
    r.servers.delete(&Rig::alice(), &id).await.unwrap();
    let leases = r.broker.leases();

    let err = try_call(&client, &format!("{id}__echo"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert_eq!(r.broker.leases(), leases);
    assert!(r.steps(thread).await.is_empty());
    // and it is not listed either
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
}

#[tokio::test]
async fn a_server_that_is_not_the_owners_is_not_served_even_when_its_id_is_attached_to_the_thread()
{
    // Alice attached her server; it was deleted, and Bob made a server under the very same id (the
    // owner is part of a definition's key). The thread still names the id; Bob's credential is not
    // lent for Alice's thread.
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    r.servers.delete(&Rig::alice(), &id).await.unwrap();
    let bob = UserId::new("bob@example.com");
    let target = EgressPolicy::new().check_url(TRACKER).unwrap();
    let bobs = UserToolServer::new(id.clone(), "Bob's", &target, UserAuth::Bearer)
        .unwrap()
        .with_state(UserToolServerState::Connected)
        .with_credential(CredentialRef::new("cred-bob").unwrap());
    r.servers.insert(&bob, &bobs).await.unwrap();

    let client = r.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let err = try_call(&client, &format!("{id}__echo"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert_eq!(
        r.broker.leases(),
        0,
        "Bob's credential is not lent for Alice's thread"
    );
    assert!(r.seen(TRACKER).is_empty());
}

#[tokio::test]
async fn people_s_servers_are_not_served_where_the_feature_is_off_or_the_relay_has_no_client_for_them()
 {
    // the thread was made when the feature was on
    let on = Rig::start().await;
    let id = on.bearer("Tracker", TRACKER).await;
    let thread = on.working_thread(&[&id]).await;
    assert!(
        tool_names(&on.client(thread).await)
            .await
            .contains(&format!("{id}__echo"))
    );

    // the same threads under an application where the feature is off
    let off = Rig::start_with(EgressPolicy::new(), on.store.clone(), Mode::Off, true).await;
    off.copy_definitions_of(&Rig::alice(), &on).await;
    let client = off.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let err = try_call(&client, &format!("{id}__echo"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert!(off.seen(TRACKER).is_empty());
    assert_eq!(off.broker.leases(), 0);

    // a relay built without the client for them serves none either
    let bare = Rig::start_with(EgressPolicy::new(), on.store.clone(), Mode::Full, false).await;
    bare.copy_definitions_of(&Rig::alice(), &on).await;
    let client = bare.client(thread).await;
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let err = try_call(&client, &format!("{id}__echo"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert_eq!(bare.broker.leases(), 0);
}

#[tokio::test]
async fn a_credential_kind_is_not_served_where_the_deployment_keeps_none() {
    // the threads were made when credentials were kept
    let on = Rig::start().await;
    let open = on
        .server(
            "Open",
            NOTES,
            UserAuth::None,
            UserToolServerState::Connected,
        )
        .await;
    let keyed = on.bearer("Keyed", TRACKER).await;
    let thread = on.working_thread(&[&open, &keyed]).await;
    // the same threads where none are kept (`without_credentials`): a `none` server works, a `bearer`
    // one does not
    let r = Rig::start_with(
        EgressPolicy::new(),
        on.store.clone(),
        Mode::WithoutCredentials,
        true,
    )
    .await;
    r.copy_definitions_of(&Rig::alice(), &on).await;
    let client = r.client(thread).await;
    let names = tool_names(&client).await;
    assert!(names.contains(&format!("{open}__echo")), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with(&format!("{keyed}__"))));
    let out = call(&client, &format!("{keyed}__echo"), json!({})).await;
    assert!(out.is_error);
    assert!(out.text.contains("could not be had now"), "{}", out.text);
    assert_eq!(r.broker.leases(), 0);
    assert!(r.seen(TRACKER).is_empty());
    let ok = call(&client, &format!("{open}__echo"), json!({"a": 1})).await;
    assert!(!ok.is_error, "{ok:?}");
}

// ---- a call that is given up ------------------------------------------------------------------

#[tokio::test]
async fn a_call_the_agent_drops_ends_its_step_as_canceled() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    let tool = format!("{id}__slow");
    let call = tokio::spawn({
        let tool = tool.clone();
        async move { try_call(&client, &tool, json!({})).await.map(|_| ()) }
    });
    orch_testsupport::eventually("the call to start", || async {
        (r.steps(thread).await.len() == 1).then_some(())
    })
    .await;
    call.abort();
    let _ = call.await;
    orch_testsupport::eventually("the step to be canceled", || async {
        let steps = r.steps(thread).await;
        (steps.len() == 2 && steps[1].state == StepState::Canceled).then_some(())
    })
    .await;
}

// ---- attaching over HTTP --------------------------------------------------------------------------

#[tokio::test]
async fn attaching_over_http_answers_the_codes_a_client_acts_on() {
    let r = Rig::start().await;
    let ready = r.bearer("Tracker", TRACKER).await;
    let waiting = r
        .server(
            "Waiting",
            NOTES,
            UserAuth::Bearer,
            UserToolServerState::Added,
        )
        .await;
    let bob = UserId::new("bob@example.com");
    let bobs = r
        .server_for(
            &bob,
            "Bob's",
            TRACKER,
            UserAuth::Bearer,
            UserToolServerState::Connected,
        )
        .await;
    let thread = r.working_thread(&[]).await;
    let path = format!("/api/threads/{thread}/tools");
    let put = |servers: Vec<String>| {
        r.http_json(
            reqwest::Method::PUT,
            &path,
            Some(json!({"servers": servers})),
        )
    };

    let (status, body) = put(vec![ready.to_string()]).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["servers"], json!([ready.to_string()]));

    let (status, body) = put(vec![ready.to_string(), waiting.to_string()]).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["code"], "tool_server_needs_auth");
    let detail = body["detail"].as_str().unwrap();
    assert!(
        detail.contains(waiting.as_str()) && detail.contains("no key or authorization yet"),
        "{detail}"
    );

    // another person's server, and an id nobody has: the same 422 with no code
    let nobody = UserToolServerId::generate();
    let (status, theirs) = put(vec![bobs.to_string()]).await;
    let (status_none, none) = put(vec![nobody.to_string()]).await;
    assert_eq!((status, status_none), (422, 422));
    assert!(
        theirs.get("code").is_none() && none.get("code").is_none(),
        "{theirs} {none}"
    );
    assert_eq!(
        theirs["detail"]
            .as_str()
            .unwrap()
            .replace(bobs.as_str(), "ID"),
        none["detail"]
            .as_str()
            .unwrap()
            .replace(nobody.as_str(), "ID")
    );

    // the set is what the last good call made it
    let kept = r.app.get_thread(&Rig::alice(), thread).await.unwrap();
    assert_eq!(kept.job.tools, [ready.to_string()]);
}

// ---- cleanliness ----------------------------------------------------------------------------------

#[tokio::test]
async fn nothing_that_was_lent_is_in_the_log_the_steps_or_the_replies() {
    let r = Rig::start().await;
    let id = r.bearer("Tracker", TRACKER).await;
    let thread = r.working_thread(&[&id]).await;
    let client = r.client(thread).await;
    let mut said = String::new();
    for n in 0..3 {
        let out = call(&client, &format!("{id}__echo"), json!({"n": n})).await;
        said.push_str(&out.text);
    }
    let tools = client.list_all_tools().await.unwrap();
    said.push_str(&serde_json::to_string(&tools).unwrap());
    let log = serde_json::to_string(&r.events(thread).await).unwrap();
    for text in [&said, &log] {
        assert!(!text.contains("lent-token"), "{text}");
        assert!(!text.contains("Bearer"), "{text}");
    }
}
