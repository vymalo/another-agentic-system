//! Test harness: the real router on a real TCP port over the in-memory stack, with the AG-UI
//! surface and the chat API mounted (the default `ORCH_SURFACES`).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_agui_proto::testkit::assert_json_conforms;
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_auth_header::HeaderAuth;
use orch_core::{AgentId, UserId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
    sample_releases,
};
use orch_ports::{AgentEndpoint, CompositeRegistry, FixedRegistry, PortSet, SystemClock};
use orch_ports::{AuthError, Authenticator, Credentials, Principal};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The deployment's own agents in front of a registry the tests change (ADR 0022).
pub type Ports = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    orch_ports::NoModel,
    CompositeRegistry<FixedRegistry, MemoryRegistry>,
    TestAuth,
    MemoryArtifacts,
>;

/// The identity of the harness's requests, and the OAuth client a request claims to come from
/// (ADR 0087, decision 1).
///
/// `HeaderAuth` reads `X-Auth-Request-Email` and ignores the bearer; here a **`Bearer` token names
/// the client** the request comes from, which no real credential does: the production JWT adapter
/// reads that from `azp`/`client_id`, and only a test can set one through this harness. A request
/// with no bearer names no client (`None`), exactly as the proxy header does.
#[derive(Debug, Clone, Default)]
pub struct TestAuth {
    inner: HeaderAuth,
}

impl TestAuth {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_dev_user(user: UserId) -> Self {
        TestAuth {
            inner: HeaderAuth::new().with_dev_user(user),
        }
    }
}

/// The bearer value that names **no client**: ADR 0087's token-with-no-client sentinel, `Some("")`,
/// which a deployment that lists signing clients treats as a signing client's (fail closed). No real
/// credential spells it; the JWT adapter reads it from a token that carries neither `azp` nor
/// `client_id`.
pub const NO_CLIENT: &str = "test:no-client";

impl Authenticator for TestAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        let mut principal = self.inner.authenticate(credentials).await?;
        principal.client_id = match credentials.bearer {
            Some(token) if token == NO_CLIENT => Some(String::new()),
            Some(token) => Some(token.to_owned()),
            None => None,
        };
        Ok(principal)
    }
}

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";
/// A person as an identity provider's `sub` names them.
pub const SUB_USER: &str = "3b1d7e52-6a40-4c18-9f0e-a1c2d3e4f501";
pub const T: Duration = Duration::from_secs(10);

/// A thread id a consumer would mint.
pub fn new_thread_id() -> String {
    uuid_like(0)
}

/// A distinct UUID-shaped id per call.
pub fn uuid_like(salt: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed) + salt;
    format!("00000000-0000-7000-8000-{n:012x}")
}

pub struct Resp {
    pub status: u16,
    pub content_type: String,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    /// The problem, asserting it is one with this status.
    pub fn problem(&self, status: u16) -> Value {
        assert_eq!(
            self.status,
            status,
            "{}",
            String::from_utf8_lossy(&self.body)
        );
        assert!(
            self.content_type.starts_with("application/problem+json"),
            "not a problem: {:?}",
            self.content_type
        );
        let p = self.json();
        assert_eq!(p["status"], status);
        assert!(p["detail"].is_string(), "{p}");
        p
    }
}

/// One AG-UI frame as it came over the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub id: Option<i64>,
    pub event: Value,
}

impl Frame {
    pub fn kind(&self) -> &str {
        self.event["type"].as_str().unwrap()
    }
}

/// A streaming AG-UI response.
pub struct Stream {
    stream: BoxStream<'static, reqwest::Result<bytes::Bytes>>,
    buf: String,
    ended: bool,
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    /// Comment lines (`: keepalive`) seen so far.
    pub comments: usize,
    /// The `id:` of each block that had no `data:`, in order: where the log ends, said by a
    /// stream whose last events have no frame.
    pub heads: Vec<i64>,
}

impl Stream {
    pub fn new(resp: reqwest::Response) -> Self {
        Stream {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            stream: resp.bytes_stream().boxed(),
            buf: String::new(),
            ended: false,
            comments: 0,
            heads: Vec::new(),
        }
    }

    /// The next frame; `None` at the end of the stream or on timeout (see `ended`).
    pub async fn next(&mut self, within: Duration) -> Option<Frame> {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            if let Some(pos) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..pos + 2).collect();
                let (mut id, mut data) = (None, Vec::new());
                for line in block.lines() {
                    if line.is_empty() {
                        continue;
                    } else if line.starts_with(':') {
                        self.comments += 1;
                    } else if let Some(v) = line.strip_prefix("id:") {
                        id = Some(v.trim().parse::<i64>().unwrap());
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push(v.strip_prefix(' ').unwrap_or(v).to_owned());
                    } else {
                        panic!("a line an AG-UI stream does not carry: {line:?}");
                    }
                }
                if data.is_empty() {
                    self.heads.extend(id);
                    continue;
                }
                let event: Value = serde_json::from_str(&data.join("\n")).unwrap();
                // Every event that leaves the surface is a schema Event.
                assert_json_conforms(&event);
                return Some(Frame { id, event });
            }
            match tokio::time::timeout_at(deadline, self.stream.next()).await {
                Err(_) => return None,
                Ok(Some(Ok(chunk))) => self.buf.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(Some(Err(_)) | None) => {
                    self.ended = true;
                    return None;
                }
            }
        }
    }

    /// Whether the server closed the stream.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Frames up to and including the first one `stop` accepts; panics if the stream ends or
    /// stays silent for `T` first. The stream stays usable.
    pub async fn until(&mut self, stop: impl Fn(&Frame) -> bool) -> Vec<Frame> {
        let mut out = Vec::new();
        loop {
            let Some(frame) = self.next(T).await else {
                panic!(
                    "the stream ended or stalled (ended: {}); got {out:?}",
                    self.ended
                );
            };
            let done = stop(&frame);
            out.push(frame);
            if done {
                return out;
            }
        }
    }

    /// Frames up to and including the next terminal event of a run.
    pub async fn through_run(&mut self) -> Vec<Frame> {
        self.until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
            .await
    }

    /// Whether nothing arrives for `quiet` (comments do not count).
    pub async fn is_quiet_for(&mut self, quiet: Duration) -> bool {
        self.next(quiet).await.is_none() && !self.ended
    }

    /// Everything that arrives until the stream is quiet for 400 ms (a connect stream stays open
    /// and idle between runs).
    pub async fn until_quiet(mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(frame) = self.next(Duration::from_millis(400)).await {
            out.push(frame);
        }
        out
    }

    /// Everything up to the end of the stream (a run's response ends after its terminal event).
    pub async fn all(mut self) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(frame) = self.next(T).await {
            out.push(frame);
        }
        assert!(self.ended, "the stream did not end; got {out:?}");
        out
    }
}

/// `type` (and the most telling member) of each frame.
pub fn kinds(frames: &[Frame]) -> Vec<&str> {
    frames.iter().map(Frame::kind).collect()
}

pub struct Harness {
    pub base: String,
    pub client: reqwest::Client,
    pub agent: ScriptedAgent,
    pub store: MemoryStore,
    /// The registry behind the static agents: empty until a test lists an agent in it, and
    /// `set_down` makes it unreachable.
    pub registry: MemoryRegistry,
    pub app: Arc<App<Ports>>,
    server: JoinHandle<()>,
    dispatcher: JoinHandle<()>,
    token: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.token.cancel();
        self.server.abort();
        self.dispatcher.abort();
    }
}

/// Millisecond timings for the dispatcher. The lease is long on purpose: a worker that is alive keeps its
/// row however slow the machine is for a moment (a lease that lapses under it hands the row to a new
/// claim, which reads the task back from a snapshot, and the log it leaves has no `working` before the
/// artifact); no test here kills a dispatcher and waits for its lease to lapse.
pub fn fast_dispatcher() -> DispatcherConfig {
    DispatcherConfig {
        concurrency: 8,
        order: orch_ports::ClaimOrder::Fair,
        lease: Duration::from_secs(60),
        heartbeat: Duration::from_millis(100),
        poll_interval: Duration::from_millis(50),
        max_attempts: 5,
        backoff_base: Duration::from_millis(20),
        backoff_max: Duration::from_millis(200),
        poll_min: Duration::from_millis(20),
        poll_max: Duration::from_millis(100),
        max_poll_failures: 10,
        max_cancel_attempts: 10,
        own_agent_listing_attempts: 5,
        own_agent_probe: Duration::from_millis(200),
        own_agent_wait_delay: Duration::from_millis(100),
        cancel_retry_delay: Duration::from_millis(50),
        verify_watch: Duration::from_millis(50),
        live_flush: Duration::from_millis(20),
        live_refresh: Duration::from_millis(150),
        live_max_bytes: 64 * 1024,
    }
}

/// The configured public origin of a signing deployment: the shared vectors' `aud`.
pub const PUBLIC_ORIGIN: &str = "https://api.test.invalid";

impl Harness {
    pub async fn start() -> Self {
        Self::start_as(None).await
    }

    /// With a development user: the identity of a request without the header.
    pub async fn start_as(dev_user: Option<&str>) -> Self {
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        let auth = match dev_user {
            Some(user) => TestAuth::with_dev_user(UserId::new(user)),
            None => TestAuth::new(),
        };
        Self::start_with(api, auth).await
    }

    pub async fn start_with(api: ApiConfig, auth: TestAuth) -> Self {
        Self::start_configured(api, auth, orch_app::Policy::default()).await
    }

    /// A deployment whose public origin is [`PUBLIC_ORIGIN`] and that lists `clients` as signing clients (ADR 0087, decision 1): a request whose
    /// token names one of them must carry `vymalo.promptSignature` in its last user message's metadata.
    /// The resource API gets the same list, as the binary gives it.
    pub async fn start_signing(clients: &[&str]) -> Self {
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            signing_clients: clients.iter().map(|c| (*c).to_owned()).collect(),
            ..ApiConfig::default()
        };
        Self::start_full(
            api,
            TestAuth::new(),
            orch_app::Policy::default(),
            orch_app::SharingSettings::default(),
            None,
            orch_surface_agui::Intake {
                signing_clients: clients
                    .iter()
                    .map(|c| (*c).to_owned())
                    .collect::<Vec<String>>()
                    .into(),
                public_origin: Some(PUBLIC_ORIGIN.into()),
            },
        )
        .await
    }

    /// With the roles `policy` gives: `Policy::deny_all()` grants nobody anything.
    pub async fn start_with_policy(policy: orch_app::Policy) -> Self {
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        Self::start_configured(api, TestAuth::new(), policy).await
    }

    async fn start_configured(api: ApiConfig, auth: TestAuth, policy: orch_app::Policy) -> Self {
        Self::start_sharing(api, auth, policy, orch_app::SharingSettings::default()).await
    }

    /// With a deployment that shares threads (ADR 0040): `sharing` is its cap and secret, `api` the
    /// limits of the public routes.
    pub async fn start_sharing(
        api: ApiConfig,
        auth: TestAuth,
        policy: orch_app::Policy,
        sharing: orch_app::SharingSettings,
    ) -> Self {
        Self::start_full(
            api,
            auth,
            policy,
            sharing,
            None,
            orch_surface_agui::Intake::default(),
        )
        .await
    }

    /// With a deployment that makes the person's own `coder` agent: `provisioning` is its
    /// provisioner (ADR 0103). A request with no identity header is [`SUB_USER`]: only a person
    /// who is an account id has an agent made (ADR 0108), and the identity header is an e-mail.
    pub async fn start_provisioned(provisioning: orch_app::Provisioning) -> Self {
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        Self::start_full(
            api,
            TestAuth::with_dev_user(UserId::new(SUB_USER)),
            orch_app::Policy::default(),
            orch_app::SharingSettings::default(),
            Some(provisioning),
            orch_surface_agui::Intake::default(),
        )
        .await
    }

    async fn start_full(
        api: ApiConfig,
        auth: TestAuth,
        policy: orch_app::Policy,
        sharing: orch_app::SharingSettings,
        provisioning: Option<orch_app::Provisioning>,
        intake: orch_surface_agui::Intake,
    ) -> Self {
        let store = MemoryStore::new();
        let agent = ScriptedAgent::new().with_releases("coder", sample_releases());
        let entry = |id: &str, name: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: name.to_owned(),
        };
        let mut directory =
            AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")]);
        // a signing deployment also has a former name of `plain` (ADR 0049)
        if intake.public_origin.is_some() {
            directory =
                directory.with_aliases([(AgentId::new("old-plain"), AgentId::new("plain"))]);
        }
        let registry = MemoryRegistry::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    prompt_signer: orch_ports::NoPromptSigner,
                    artifacts: MemoryArtifacts::new(),
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: agent.clone(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: orch_ports::NoModel,
                    auth,
                    registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
                    user_tools: orch_ports::NoUserTools::off(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    policy,
                    sharing,
                    provisioning,
                    // the servers a run may attach to the thread it creates (ADR 0024); `repos` is
                    // the coder's only
                    tool_servers: vec![
                        orch_app::ToolServerInfo::new("websearch", "Web search"),
                        orch_app::ToolServerInfo {
                            agents: Some(vec![AgentId::new("coder")]),
                            ..orch_app::ToolServerInfo::new("repos", "Repositories")
                        },
                    ],
                    ..AppConfig::default()
                },
            )
            .expect("a valid gate"),
        );
        let token = CancellationToken::new();
        let dispatcher = tokio::spawn(
            Dispatcher::new(Arc::clone(&app), fast_dispatcher(), "test-dispatcher")
                .run(token.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let keepalive = api.sse_keepalive;
        let lifetime = api.sse_lifetime;
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![orch_surface_agui::routes(
                Arc::clone(&app),
                keepalive,
                lifetime,
                intake,
            )],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            agent,
            store,
            registry,
            app,
            server,
            dispatcher,
            token,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// `POST /agui/agents/{agent}` as `user`, JSON body, streaming accepted.
    pub async fn post(&self, agent: &str, user: Option<&str>, body: &Value) -> reqwest::Response {
        let mut req = self
            .client
            .post(self.url(&format!("/agui/agents/{agent}")))
            .header("Accept", "text/event-stream")
            .json(body);
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        req.send().await.unwrap()
    }

    /// `POST /agui/agents/{agent}` as `user` of the OAuth client `client` (ADR 0087): the bearer
    /// names the client to [`TestAuth`], and the identity header names the person.
    pub async fn post_as_client(
        &self,
        agent: &str,
        user: &str,
        client: &str,
        body: &Value,
    ) -> reqwest::Response {
        self.client
            .post(self.url(&format!("/agui/agents/{agent}")))
            .header("Accept", "text/event-stream")
            .header("X-Auth-Request-Email", user)
            .header("Authorization", format!("Bearer {client}"))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    /// `POST {path}` on the resource API as `user` of the OAuth client `client`.
    pub async fn api_post_as_client(
        &self,
        path: &str,
        user: &str,
        client: &str,
        body: &Value,
    ) -> Resp {
        resp_of(
            self.client
                .post(self.url(path))
                .header("X-Auth-Request-Email", user)
                .header("Authorization", format!("Bearer {client}"))
                .json(body)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    /// A run of the client `client` that must be accepted: its stream.
    pub async fn run_as_client(
        &self,
        agent: &str,
        user: &str,
        client: &str,
        body: &Value,
    ) -> Stream {
        let resp = self.post_as_client(agent, user, client, body).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "refused: {}",
            resp.text().await.unwrap()
        );
        Stream::new(resp)
    }

    /// A run of the client `client` that must be refused before the stream: its problem.
    pub async fn refused_as_client(
        &self,
        agent: &str,
        user: &str,
        client: &str,
        body: &Value,
    ) -> Resp {
        resp_of(self.post_as_client(agent, user, client, body).await).await
    }

    /// A run that must be accepted: its stream.
    pub async fn run(&self, agent: &str, user: &str, body: &Value) -> Stream {
        let resp = self.post(agent, Some(user), body).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "refused: {}",
            resp.text().await.unwrap()
        );
        Stream::new(resp)
    }

    /// A run that must be refused before the stream: its problem.
    pub async fn refused(&self, agent: &str, user: Option<&str>, body: &Value) -> Resp {
        resp_of(self.post(agent, user, body).await).await
    }

    pub async fn get(&self, path: &str, user: Option<&str>) -> Resp {
        let mut req = self.client.get(self.url(path));
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        resp_of(req.send().await.unwrap()).await
    }

    /// `GET /agui/threads/{thread}/connect`, unchecked.
    pub async fn connect_raw(
        &self,
        thread: &str,
        user: Option<&str>,
        last_event_id: Option<&str>,
        query: Option<&str>,
    ) -> reqwest::Response {
        let path = match query {
            Some(q) => format!("/agui/threads/{thread}/connect?{q}"),
            None => format!("/agui/threads/{thread}/connect"),
        };
        let mut req = self
            .client
            .get(self.url(&path))
            .header("Accept", "text/event-stream");
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-ID", id);
        }
        req.send().await.unwrap()
    }

    /// `GET /agui/threads/{thread}/history[?query]`, unchecked.
    pub async fn history(
        &self,
        thread: &str,
        user: Option<&str>,
        query: &str,
        accept: Option<&str>,
    ) -> Resp {
        let path = format!("/agui/threads/{thread}/history{}", query_of(query));
        self.history_at(&path, user, accept).await
    }

    /// `GET /agui/shared/{token}/history` (signed in) or `/agui/public/shared/{token}/history`
    /// (`public`), unchecked.
    pub async fn history_shared(
        &self,
        token: &str,
        public: bool,
        user: Option<&str>,
        query: &str,
    ) -> Resp {
        let kind = if public { "public/shared" } else { "shared" };
        let path = format!("/agui/{kind}/{token}/history{}", query_of(query));
        self.history_at(&path, user, None).await
    }

    async fn history_at(&self, path: &str, user: Option<&str>, accept: Option<&str>) -> Resp {
        let mut req = self.client.get(self.url(path));
        if let Some(accept) = accept {
            req = req.header("Accept", accept);
        }
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        resp_of(req.send().await.unwrap()).await
    }

    /// A connect that must be accepted: its stream.
    pub async fn connect(&self, thread: &str, user: &str, last_event_id: Option<i64>) -> Stream {
        self.connect_with(thread, user, last_event_id, None).await
    }

    /// `?mode=run`.
    pub async fn connect_run(
        &self,
        thread: &str,
        user: &str,
        last_event_id: Option<i64>,
    ) -> Stream {
        self.connect_with(thread, user, last_event_id, Some("mode=run"))
            .await
    }

    async fn connect_with(
        &self,
        thread: &str,
        user: &str,
        last_event_id: Option<i64>,
        query: Option<&str>,
    ) -> Stream {
        let resp = self
            .connect_raw(
                thread,
                Some(user),
                last_event_id.map(|n| n.to_string()).as_deref(),
                query,
            )
            .await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "refused: {}",
            resp.text().await.unwrap()
        );
        Stream::new(resp)
    }

    pub async fn post_empty(&self, path: &str, user: &str) -> Resp {
        resp_of(
            self.client
                .post(self.url(path))
                .header("X-Auth-Request-Email", user)
                .send()
                .await
                .unwrap(),
        )
        .await
    }

    pub async fn thread(&self, user: &str, id: &str) -> Value {
        let r = self.get(&format!("/api/threads/{id}"), Some(user)).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        r.json()
    }

    pub async fn state(&self, user: &str, id: &str) -> String {
        self.thread(user, id).await["state"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    pub async fn wait_state(&self, user: &str, id: &str, want: &str) {
        let deadline = tokio::time::Instant::now() + T;
        loop {
            let got = self.state(user, id).await;
            if got == want {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {want}, still {got}"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The thread's events, through the store (the chat API's routes are not mounted here).
    pub async fn events(&self, user: &str, id: &str) -> Vec<Value> {
        let id = id.parse().unwrap();
        self.app
            .list_events(&UserId::new(user), id, 0, 500)
            .await
            .unwrap()
            .iter()
            .map(|e| serde_json::to_value(e).unwrap())
            .collect()
    }
}

/// `?query`, or nothing for an empty one.
fn query_of(query: &str) -> String {
    if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    }
}

pub async fn resp_of(resp: reqwest::Response) -> Resp {
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let headers = resp.headers().clone();
    Resp {
        status,
        content_type,
        headers,
        body: resp.bytes().await.unwrap().to_vec(),
    }
}

/// A `RunAgentInput` body.
pub fn input(thread: &str, run: &str, messages: &[(&str, &str)]) -> Value {
    let messages: Vec<Value> = messages
        .iter()
        .map(|(id, text)| json!({"id": id, "role": "user", "content": text}))
        .collect();
    json!({"threadId": thread, "runId": run, "messages": messages})
}

/// `input` with members of `extra` merged over the top.
pub fn input_with(thread: &str, run: &str, messages: &[(&str, &str)], extra: Value) -> Value {
    let mut body = input(thread, run, messages);
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    body
}

/// The secret of the sharing tests: 32 bytes.
pub const SHARING_SECRET: &str = "0123456789abcdef0123456789abcdef";

/// A deployment's sharing settings of `mode`, with the test secret.
pub fn sharing(mode: orch_app::SharingMode) -> orch_app::SharingSettings {
    orch_app::SharingSettings::new(
        mode,
        Some(
            orch_app::ShareKeys::new(secrecy::SecretString::from(SHARING_SECRET.to_owned()), None)
                .unwrap(),
        ),
        orch_app::PublicView::default(),
    )
    .unwrap()
}

impl Harness {
    /// A deployment that shares up to `mode`, with the default limits of the public routes.
    pub async fn start_shared(mode: orch_app::SharingMode) -> Self {
        Self::start_shared_with(mode, orch_api::PublicLimits::default()).await
    }

    /// As [`start_shared`](Self::start_shared), with these limits on the public routes.
    pub async fn start_shared_with(
        mode: orch_app::SharingMode,
        limits: orch_api::PublicLimits,
    ) -> Self {
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            public_limits: Some(limits),
            ..ApiConfig::default()
        };
        Self::start_sharing(
            api,
            TestAuth::new(),
            orch_app::Policy::default(),
            sharing(mode),
        )
        .await
    }

    /// Shares `thread` (Alice's) at `level` and returns the token of the link.
    pub async fn share(&self, thread: &str, level: orch_core::ShareLevel) -> String {
        let view = self
            .app
            .share_thread(&UserId::new(ALICE), thread.parse().unwrap(), level)
            .await
            .unwrap();
        view.url.unwrap().trim_start_matches("/s/").to_owned()
    }

    /// `GET /agui/shared/{token}/connect` (signed in) or `/agui/public/shared/{token}/connect`
    /// (`user: None` and `public`), unchecked.
    pub async fn connect_shared_raw(
        &self,
        token: &str,
        public: bool,
        user: Option<&str>,
        last_event_id: Option<&str>,
        accept: &str,
    ) -> reqwest::Response {
        let path = if public {
            format!("/agui/public/shared/{token}/connect")
        } else {
            format!("/agui/shared/{token}/connect")
        };
        let mut req = self.client.get(self.url(&path)).header("Accept", accept);
        if let Some(u) = user {
            req = req.header("X-Auth-Request-Email", u);
        }
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-ID", id);
        }
        req.send().await.unwrap()
    }

    /// A shared connect that must be accepted: its stream.
    pub async fn connect_shared(&self, token: &str, public: bool, user: Option<&str>) -> Stream {
        let resp = self
            .connect_shared_raw(token, public, user, None, "text/event-stream")
            .await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "refused: {}",
            resp.text().await.unwrap()
        );
        Stream::new(resp)
    }
}
