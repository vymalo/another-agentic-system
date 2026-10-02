//! A running orchestrator (router + dispatcher) on a real TCP port, and a client for its HTTP API
//! (the resource API and the AG-UI routes).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orch_api::{ApiConfig, SurfaceRoutes};
use orch_app::{App, Dispatcher, DispatcherConfig, NewThread};
use orch_core::{AgentId, AgentTarget, ThreadId, UserId};
use orch_ports::Ports;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::fake::EXTENSION_URI;
use crate::sse::SseClient;
use crate::wait::eventually;

/// Dispatcher timings that make lease expiry, retries and polling happen in milliseconds.
pub fn fast_dispatcher() -> DispatcherConfig {
    DispatcherConfig {
        concurrency: 8,
        lease: Duration::from_millis(800),
        heartbeat: Duration::from_millis(150),
        poll_interval: Duration::from_millis(50),
        max_attempts: 4,
        backoff_base: Duration::from_millis(30),
        backoff_max: Duration::from_millis(200),
        poll_min: Duration::from_millis(30),
        poll_max: Duration::from_millis(150),
        max_poll_failures: 10,
        max_cancel_attempts: 10,
        cancel_retry_delay: Duration::from_millis(50),
        verify_watch: Duration::from_millis(50),
        live_flush: Duration::from_millis(20),
        live_refresh: Duration::from_millis(150),
        live_max_bytes: 64 * 1024,
    }
}

/// One orchestrator process: the HTTP API and a dispatcher over a shared [`App`].
///
/// Dropping it, or [`TestInstance::kill`], aborts both tasks without any cleanup: that is what
/// a crashed process looks like (leases are not released), and the responses still streaming
/// are cut mid-body, so a client sees the connection break instead of a clean end.
pub struct TestInstance {
    /// `http://127.0.0.1:<port>`.
    pub base_url: String,
    server: JoinHandle<()>,
    /// `None` for an instance that only serves the API.
    dispatcher: Option<JoinHandle<()>>,
    shutdown: CancellationToken,
    /// Cancelled when the process "dies": streaming bodies break.
    crash: CancellationToken,
    /// The process's own view of the event log (see [`Chat::events`]).
    log: Arc<dyn LogTap>,
}

/// The event log seen from inside a test process. No HTTP route returns the log as the core
/// wrote it (the AG-UI surface returns its projection), and a test that pins what the core and
/// the A2A adapter wrote needs the events themselves; it reads them here, through the same
/// [`App`] the routes use.
#[async_trait]
trait LogTap: Send + Sync {
    /// Every event of `thread`, as the JSON of `orch_core::Event`.
    async fn events(&self, user: &str, thread: &str) -> Vec<Value>;
    /// Creates a thread through the application, without any surface: the log holds the message
    /// with no consumer-chosen ids. Returns its id.
    async fn create_thread(
        &self,
        user: &str,
        agent: &str,
        text: &str,
        release: Option<&str>,
    ) -> String;
    /// Posts a message to `thread` through the application, as a producer that is not AG-UI.
    /// Returns the `user_message` event.
    async fn post_message(&self, user: &str, thread: &str, text: &str) -> Value;
}

struct AppLog<P: Ports>(Arc<App<P>>);

#[async_trait]
impl<P: Ports> LogTap for AppLog<P> {
    async fn events(&self, user: &str, thread: &str) -> Vec<Value> {
        const PAGE: u32 = 500;
        let user = UserId::new(user);
        let id = thread.parse::<ThreadId>().unwrap();
        let mut all: Vec<Value> = Vec::new();
        loop {
            let after = all.last().map_or(0, |e| e["seq"].as_i64().unwrap());
            let page = self.0.list_events(&user, id, after, PAGE).await.unwrap();
            let done = page.len() < PAGE as usize;
            all.extend(page.iter().map(|e| serde_json::to_value(e).unwrap()));
            if done {
                return all;
            }
        }
    }

    async fn create_thread(
        &self,
        user: &str,
        agent: &str,
        text: &str,
        release: Option<&str>,
    ) -> String {
        let thread = self
            .0
            .create_thread(
                &UserId::new(user),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new(agent),
                        release: release.map(str::to_owned),
                    },
                    text: text.to_owned(),
                },
            )
            .await
            .unwrap();
        thread.id.to_string()
    }

    async fn post_message(&self, user: &str, thread: &str, text: &str) -> Value {
        let event = self
            .0
            .post_message(
                &UserId::new(user),
                thread.parse::<ThreadId>().unwrap(),
                text.to_owned(),
            )
            .await
            .unwrap();
        serde_json::to_value(event).unwrap()
    }
}

/// Breaks a response body when `crash` is cancelled: the body ends with an I/O error, which
/// hyper turns into an aborted connection (no terminating chunk), as a dead process would.
async fn break_on_crash(
    crash: CancellationToken,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (parts, body) = next.run(request).await.into_parts();
    let data = body.into_data_stream();
    let stream = futures::stream::unfold((data, false), move |(mut data, dead)| {
        let crash = crash.clone();
        async move {
            if dead {
                return None;
            }
            tokio::select! {
                () = crash.cancelled() => Some((
                    Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "the process died",
                    )),
                    (data, true),
                )),
                next = data.next() => {
                    next.map(|chunk| (chunk.map_err(std::io::Error::other), (data, false)))
                }
            }
        }
    });
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

impl TestInstance {
    /// Starts serving `app` and dispatching its outbox as `owner`.
    pub async fn spawn<P: Ports>(
        app: Arc<App<P>>,
        api: ApiConfig,
        dispatcher: DispatcherConfig,
        owner: &str,
    ) -> Self {
        Self::spawn_with(app, api, Some(dispatcher), owner).await
    }

    /// Like [`TestInstance::spawn`]; `dispatcher: None` serves the API only, like a replica
    /// whose dispatcher is not running (its outbox rows are left to the other replicas).
    pub async fn spawn_with<P: Ports>(
        app: Arc<App<P>>,
        api: ApiConfig,
        dispatcher: Option<DispatcherConfig>,
        owner: &str,
    ) -> Self {
        Self::spawn_with_surfaces(app, api, dispatcher, owner, Vec::new()).await
    }

    /// Like [`TestInstance::spawn_with`], and `extra` surfaces (built by the caller over the same
    /// `app`, for example the MCP server) are mounted beside the AG-UI routes.
    pub async fn spawn_with_surfaces<P: Ports>(
        app: Arc<App<P>>,
        api: ApiConfig,
        dispatcher: Option<DispatcherConfig>,
        owner: &str,
        extra: Vec<SurfaceRoutes>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        Self::spawn_on(listener, app, api, dispatcher, owner, extra).await
    }

    /// Like [`TestInstance::spawn_with_surfaces`], serving on `listener`, which the caller bound
    /// beforehand: a test that has to tell something its own address before the instance exists
    /// (the base URL of the thread tools, which the A2A adapter puts in every grant) binds first.
    pub async fn spawn_on<P: Ports>(
        listener: tokio::net::TcpListener,
        app: Arc<App<P>>,
        api: ApiConfig,
        dispatcher: Option<DispatcherConfig>,
        owner: &str,
        extra: Vec<SurfaceRoutes>,
    ) -> Self {
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let dispatcher = dispatcher.map(|config| {
            tokio::spawn(Dispatcher::new(Arc::clone(&app), config, owner).run(shutdown.clone()))
        });
        let keepalive = api.sse_keepalive;
        let crash = CancellationToken::new();
        let log: Arc<dyn LogTap> = Arc::new(AppLog(Arc::clone(&app)));
        let mut surfaces = vec![orch_surface_agui::routes(Arc::clone(&app), keepalive)];
        surfaces.extend(extra);
        let router =
            orch_api::router_with_surfaces(app, api, surfaces).layer(axum::middleware::from_fn({
                let crash = crash.clone();
                move |request, next| break_on_crash(crash.clone(), request, next)
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        TestInstance {
            base_url: format!("http://{addr}"),
            server,
            dispatcher,
            shutdown,
            crash,
            log,
        }
    }

    /// A client for this instance acting as `user`. Unlike [`Chat::new`], it can also read the
    /// event log ([`Chat::events`]) and write to it without a surface ([`Chat::seed_thread`]).
    pub fn chat(&self, user: &str) -> Chat {
        Chat {
            log: Some(Arc::clone(&self.log)),
            ..Chat::new(&self.base_url, user)
        }
    }

    /// Simulates a crash: no graceful shutdown, no lease release, and the streams that were open
    /// break.
    pub fn kill(&self) {
        self.crash.cancel();
        self.server.abort();
        if let Some(dispatcher) = &self.dispatcher {
            dispatcher.abort();
        }
    }

    /// Stops the dispatcher gracefully (it releases its leases) and the server.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        self.crash.cancel();
        self.server.abort();
        if let Some(dispatcher) = &mut self.dispatcher {
            let _ = tokio::time::timeout(Duration::from_secs(10), dispatcher).await;
        }
    }
}

impl Drop for TestInstance {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A client acting as one user: the resource API, and the AG-UI routes.
///
/// Threads are created and continued the way a real consumer does, through
/// `POST /agui/agents/{agentId}`. The event log as the core wrote it has no HTTP route; a client
/// obtained from [`TestInstance::chat`] reads it in-process ([`Chat::events`]).
#[derive(Clone)]
pub struct Chat {
    base: String,
    user: Option<String>,
    http: reqwest::Client,
    log: Option<Arc<dyn LogTap>>,
}

impl Chat {
    /// A client for `base_url` sending `X-Auth-Request-Email: user`.
    pub fn new(base_url: &str, user: &str) -> Self {
        Chat {
            base: base_url.to_owned(),
            user: Some(user.to_owned()),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            log: None,
        }
    }

    /// The same client for another user.
    pub fn as_user(&self, user: &str) -> Self {
        Chat {
            user: Some(user.to_owned()),
            ..self.clone()
        }
    }

    /// The same client without any identity header.
    pub fn anonymous(&self) -> Self {
        Chat {
            user: None,
            ..self.clone()
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.user {
            Some(u) => req.header("X-Auth-Request-Email", u),
            None => req,
        }
    }

    /// `GET path` as `(status, body)`; the body is `Null` when empty.
    pub async fn get(&self, path: &str) -> (u16, Value) {
        Self::finish(self.request(reqwest::Method::GET, path)).await
    }

    /// `POST path` with an optional JSON body, as `(status, body)`.
    pub async fn post(&self, path: &str, body: Option<Value>) -> (u16, Value) {
        let req = self.request(reqwest::Method::POST, path);
        Self::finish(match body {
            Some(b) => req.json(&b),
            None => req,
        })
        .await
    }

    /// `PATCH /api/threads/{id}` with `{"title": title}`: renames the thread, as `(status, body)`.
    pub async fn rename(&self, id: &str, title: &str) -> (u16, Value) {
        Self::finish(
            self.request(reqwest::Method::PATCH, &format!("/api/threads/{id}"))
                .json(&serde_json::json!({ "title": title })),
        )
        .await
    }

    /// `PATCH /api/threads/{id}` with `{"description": description}`: writes (or, when empty,
    /// clears) the thread's description, as `(status, body)`.
    pub async fn describe(&self, id: &str, description: &str) -> (u16, Value) {
        Self::finish(
            self.request(reqwest::Method::PATCH, &format!("/api/threads/{id}"))
                .json(&serde_json::json!({ "description": description })),
        )
        .await
    }

    /// `PUT /api/threads/{id}/tools` with `{"servers": servers}`: sets the MCP servers attached to
    /// the thread (ADR 0024), as `(status, body)`.
    pub async fn put_tools(&self, id: &str, servers: &[&str]) -> (u16, Value) {
        Self::finish(
            self.request(reqwest::Method::PUT, &format!("/api/threads/{id}/tools"))
                .json(&serde_json::json!({ "servers": servers })),
        )
        .await
    }

    /// `GET /api/tool-servers`: the servers a person may attach, as `(status, body)`.
    pub async fn tool_servers(&self) -> (u16, Value) {
        self.get("/api/tool-servers").await
    }

    async fn finish(req: reqwest::RequestBuilder) -> (u16, Value) {
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Creates a thread the way an AG-UI consumer does and returns its id: a run of `text` on
    /// `agent` under a fresh UUIDv7 thread id, with `release` in `forwardedProps`. Returns once
    /// the run route has accepted it (the thread exists, and its first message is in the log); the
    /// run's stream is dropped unread, since the log does not depend on anybody reading it. Panics
    /// unless the answer is 200.
    pub async fn create_thread(&self, agent: &str, text: &str, release: Option<&str>) -> String {
        let (status, body) = self.try_create_thread(agent, text, release).await;
        assert_eq!(status, 200, "create thread failed: {body}");
        body["threadId"].as_str().unwrap().to_owned()
    }

    /// [`Chat::create_thread`], returning the raw answer: `(200, {"threadId": …})` when the run
    /// was accepted, the status and the problem otherwise.
    pub async fn try_create_thread(
        &self,
        agent: &str,
        text: &str,
        release: Option<&str>,
    ) -> (u16, Value) {
        let thread = Uuid::now_v7().to_string();
        let extra = match release {
            Some(r) => json!({"forwardedProps": {EXTENSION_URI: {"release": r}}}),
            None => json!({}),
        };
        let body = Self::agui_input(&thread, "run-1", &[("msg-1", text)], extra);
        let response = self.agui_post(agent, &body).await;
        let status = response.status().as_u16();
        if status == 200 {
            return (status, json!({ "threadId": thread }));
        }
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// [`Chat::try_create_thread`] with MCP servers to attach (`forwardedProps["vymalo.tools"]`,
    /// ADR 0024): `(200, {"threadId": …})` when the run was accepted, the status and the problem
    /// otherwise.
    pub async fn try_create_thread_with_tools(
        &self,
        agent: &str,
        text: &str,
        tools: &[&str],
    ) -> (u16, Value) {
        let thread = Uuid::now_v7().to_string();
        let body = Self::agui_input(
            &thread,
            "run-1",
            &[("msg-1", text)],
            json!({"forwardedProps": {"vymalo.tools": tools}}),
        );
        let response = self.agui_post(agent, &body).await;
        let status = response.status().as_u16();
        if status == 200 {
            return (status, json!({ "threadId": thread }));
        }
        let bytes = response.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Sends a follow-up message to `thread` (which `agent` runs) as a new AG-UI run: the way a
    /// consumer answers a question or adds to a thread. Returns the run's stream; panics unless
    /// the answer is 200.
    pub async fn follow_up(&self, thread: &str, agent: &str, text: &str) -> SseClient {
        let n = Uuid::now_v7();
        let body = Self::agui_input(
            thread,
            &format!("run-{n}"),
            &[(&format!("msg-{n}"), text)],
            json!({}),
        );
        self.agui_run(agent, &body).await
    }

    /// Creates a thread through the application, with no surface in between: its first message
    /// carries no consumer-chosen ids, as an input from a producer that is not AG-UI would.
    /// Needs a client from [`TestInstance::chat`].
    pub async fn seed_thread(&self, agent: &str, text: &str, release: Option<&str>) -> String {
        self.tap()
            .create_thread(self.user_name(), agent, text, release)
            .await
    }

    /// Posts `text` to `thread` through the application, with no surface in between; returns the
    /// `user_message` event. Needs a client from [`TestInstance::chat`].
    pub async fn seed_message(&self, thread: &str, text: &str) -> Value {
        self.tap()
            .post_message(self.user_name(), thread, text)
            .await
    }

    fn tap(&self) -> &Arc<dyn LogTap> {
        self.log
            .as_ref()
            .expect("this client cannot reach the event log: build it with TestInstance::chat")
    }

    fn user_name(&self) -> &str {
        self.user
            .as_deref()
            .expect("the anonymous client has no user to act as")
    }

    /// `GET /api/threads/{id}`.
    pub async fn thread(&self, id: &str) -> Value {
        let (status, body) = self.get(&format!("/api/threads/{id}")).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    /// The thread's state string.
    pub async fn state(&self, id: &str) -> String {
        self.thread(id).await["state"].as_str().unwrap().to_owned()
    }

    /// Waits until the thread is in `want`.
    pub async fn wait_state(&self, id: &str, want: &str) {
        let mut last = String::new();
        let outcome = tokio::time::timeout(crate::DEFAULT_TIMEOUT, async {
            loop {
                last = self.state(id).await;
                if last == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            outcome.is_ok(),
            "timed out waiting for thread state {want}; last seen {last}"
        );
    }

    /// Waits until the thread has at least `n` events and returns them all.
    pub async fn wait_events(&self, id: &str, n: usize) -> Vec<Value> {
        eventually(&format!("{n} events on thread {id}"), || async {
            let ev = self.events(id).await;
            (ev.len() >= n).then_some(ev)
        })
        .await
    }

    /// All events of the thread, as the core wrote them (the JSON of `orch_core::Event`), read
    /// in-process. Needs a client from [`TestInstance::chat`]; no HTTP route returns the log.
    pub async fn events(&self, id: &str) -> Vec<Value> {
        self.tap().events(self.user_name(), id).await
    }

    /// `POST /cancel`; returns the status.
    pub async fn cancel(&self, id: &str) -> u16 {
        self.post(&format!("/api/threads/{id}/cancel"), None)
            .await
            .0
    }

    /// `POST /agui/agents/{agent}` with `body` as JSON: the raw answer, for a test that looks at
    /// the status, the headers or the problem.
    pub async fn agui_post(&self, agent: &str, body: &Value) -> reqwest::Response {
        self.request(reqwest::Method::POST, &format!("/agui/agents/{agent}"))
            .header("Accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    /// Starts an AG-UI run and returns its stream; panics unless the answer is 200.
    pub async fn agui_run(&self, agent: &str, body: &Value) -> SseClient {
        let resp = self.agui_post(agent, body).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "the run was refused: {}",
            resp.text().await.unwrap()
        );
        SseClient::from_response(resp)
    }

    /// A `RunAgentInput` for `thread`: `run` is the run id, `messages` the transcript the
    /// consumer holds, `extra` merged over the top (`resume`, `forwardedProps`, …).
    pub fn agui_input(thread: &str, run: &str, messages: &[(&str, &str)], extra: Value) -> Value {
        let messages: Vec<Value> = messages
            .iter()
            .map(|(id, text)| json!({"id": id, "role": "user", "content": text}))
            .collect();
        let mut body = json!({"threadId": thread, "runId": run, "messages": messages});
        if let Some(extra) = extra.as_object() {
            for (k, v) in extra {
                body[k] = v.clone();
            }
        }
        body
    }

    /// `GET /agui/threads/{id}/connect`: the raw answer, for a test that looks at the status, the
    /// headers or the problem. `last_event_id` is sent verbatim; `query` is appended to the URL
    /// (`"mode=run"`).
    pub async fn agui_connect_raw(
        &self,
        id: &str,
        last_event_id: Option<&str>,
        query: Option<&str>,
    ) -> reqwest::Response {
        let path = match query {
            Some(q) => format!("/agui/threads/{id}/connect?{q}"),
            None => format!("/agui/threads/{id}/connect"),
        };
        let mut req = self
            .request(reqwest::Method::GET, &path)
            .header("Accept", "text/event-stream");
        if let Some(n) = last_event_id {
            req = req.header("Last-Event-ID", n);
        }
        req.send().await.unwrap()
    }

    /// Connects to a thread's AG-UI stream (replaying after `last_event_id`, and only the active
    /// run's remainder with `mode_run`); panics unless the answer is 200.
    pub async fn agui_connect(
        &self,
        id: &str,
        last_event_id: Option<i64>,
        mode_run: bool,
    ) -> SseClient {
        let resp = self
            .agui_connect_raw(
                id,
                last_event_id.map(|n| n.to_string()).as_deref(),
                mode_run.then_some("mode=run"),
            )
            .await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "the connect was refused: {}",
            resp.text().await.unwrap()
        );
        SseClient::from_response(resp)
    }

    /// `GET /agui/agents/{agent}/capabilities` as `(status, body)`.
    pub async fn agui_capabilities(&self, agent: &str) -> (u16, Value) {
        self.get(&format!("/agui/agents/{agent}/capabilities"))
            .await
    }
}

/// `kind` (with the status or state for the kinds that have one) of each event.
pub fn shape(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| {
            let kind = e["kind"].as_str().unwrap();
            match kind {
                "agent_status" => format!("{kind}:{}", e["data"]["status"].as_str().unwrap()),
                "thread_state" => format!("{kind}:{}", e["data"]["state"].as_str().unwrap()),
                _ => kind.to_owned(),
            }
        })
        .collect()
}
