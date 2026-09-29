//! A running orchestrator (router + dispatcher) on a real TCP port, and a chat API client.

use std::sync::Arc;
use std::time::Duration;

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
/// a crashed process looks like (leases are not released).
pub struct TestInstance {
    /// `http://127.0.0.1:<port>`.
    pub base_url: String,
    server: JoinHandle<()>,
    dispatcher: JoinHandle<()>,
    shutdown: CancellationToken,
}

impl TestInstance {
    /// Starts serving `app` and dispatching its outbox as `owner`.
    pub async fn spawn<P: Ports>(
        app: Arc<App<P>>,
        api: ApiConfig,
        dispatcher: DispatcherConfig,
        owner: &str,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let dispatcher = tokio::spawn(
            Dispatcher::new(Arc::clone(&app), dispatcher, owner).run(shutdown.clone()),
        );
        let router = orch_api::router(app, api);
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        TestInstance {
            base_url: format!("http://{addr}"),
            server,
            dispatcher,
            shutdown,
        }
    }

    /// Simulates a crash: no graceful shutdown, no lease release.
    pub fn kill(&self) {
        self.server.abort();
        self.dispatcher.abort();
    }

    /// Stops the dispatcher gracefully (it releases its leases) and the server.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        self.server.abort();
        let _ = tokio::time::timeout(Duration::from_secs(10), &mut self.dispatcher).await;
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
