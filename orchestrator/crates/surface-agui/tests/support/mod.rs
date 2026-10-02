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
    MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds, sample_releases,
};
use orch_ports::{AgentEndpoint, CompositeRegistry, FixedRegistry, PortSet, SystemClock};
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
    orch_auth_header::HeaderAuth,
>;

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";
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

pub fn fast_dispatcher() -> DispatcherConfig {
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
            Some(user) => HeaderAuth::new().with_dev_user(UserId::new(user)),
            None => HeaderAuth::new(),
        };
        Self::start_with(api, auth).await
    }

    pub async fn start_with(api: ApiConfig, auth: HeaderAuth) -> Self {
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
        let directory = AgentDirectory::new(vec![entry("coder", "Coder"), entry("plain", "Plain")]);
        let registry = MemoryRegistry::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: agent.clone(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: orch_ports::NoModel,
                    auth,
                    registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
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
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![orch_surface_agui::routes(Arc::clone(&app), keepalive)],
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
