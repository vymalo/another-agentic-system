//! A running orchestrator (router + dispatcher) on a real TCP port, and a chat API client.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_api::ApiConfig;
use orch_app::{App, Dispatcher, DispatcherConfig};
use orch_ports::Ports;
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let dispatcher = dispatcher.map(|config| {
            tokio::spawn(Dispatcher::new(Arc::clone(&app), config, owner).run(shutdown.clone()))
        });
        let keepalive = api.sse_keepalive;
        let crash = CancellationToken::new();
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![
                orch_surface_agui::routes(Arc::clone(&app), keepalive),
                orch_surface_chat_api::routes(app, keepalive),
            ],
        )
        .layer(axum::middleware::from_fn({
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

/// A chat API client acting as one user.
#[derive(Clone)]
pub struct Chat {
    base: String,
    user: Option<String>,
    http: reqwest::Client,
}

impl Chat {
    /// A client for `base_url` sending `X-Auth-Request-Email: user`.
    pub fn new(base_url: &str, user: &str) -> Self {
        Chat {
            base: base_url.to_owned(),
            user: Some(user.to_owned()),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
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

    async fn finish(req: reqwest::RequestBuilder) -> (u16, Value) {
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Creates a thread and returns its id; panics unless the API answers 201.
    pub async fn create_thread(&self, agent: &str, text: &str, release: Option<&str>) -> String {
        let (status, body) = self.try_create_thread(agent, text, release).await;
        assert_eq!(status, 201, "create thread failed: {body}");
        body["id"].as_str().unwrap().to_owned()
    }

    /// Creates a thread, returning the raw answer.
    pub async fn try_create_thread(
        &self,
        agent: &str,
        text: &str,
        release: Option<&str>,
    ) -> (u16, Value) {
        let mut target = json!({ "agentId": agent });
        if let Some(r) = release {
            target["release"] = json!(r);
        }
        self.post(
            "/api/threads",
            Some(json!({ "target": target, "text": text })),
        )
        .await
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

    /// All events of the thread (`GET /events`, paged transparently).
    pub async fn events(&self, id: &str) -> Vec<Value> {
        let mut all: Vec<Value> = Vec::new();
        loop {
            let after = all.last().map_or(0, |e| e["seq"].as_i64().unwrap());
            let (status, body) = self
                .get(&format!("/api/threads/{id}/events?after={after}&limit=500"))
                .await;
            assert_eq!(status, 200, "{body}");
            let page = body.as_array().unwrap().clone();
            let done = page.len() < 500;
            all.extend(page);
            if done {
                return all;
            }
        }
    }

    /// Follow-up message; returns `(status, body)`.
    pub async fn post_message(&self, id: &str, text: &str) -> (u16, Value) {
        self.post(
            &format!("/api/threads/{id}/messages"),
            Some(json!({ "text": text })),
        )
        .await
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

    /// Opens the SSE stream, optionally resuming after `Last-Event-ID`.
    pub async fn stream(&self, id: &str, last_event_id: Option<i64>) -> SseClient {
        let mut req = self.request(reqwest::Method::GET, &format!("/api/threads/{id}/stream"));
        if let Some(n) = last_event_id {
            req = req.header("Last-Event-ID", n.to_string());
        }
        SseClient::from_response(req.send().await.unwrap())
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
